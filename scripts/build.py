#!/usr/bin/env python3
"""Builds the Flux plugin catalog: grammars, plugins with code, packages and the index.

For every folder of `plugins/` (a plugin, named by its id):

1. its tree-sitter grammars (`grammars.toml`) are fetched at the pinned revision and compiled to
   WebAssembly with the tree-sitter CLI (`tree-sitter build --wasm`), into the paths the manifest's
   `[[grammars]] wasm` names;
2. a plugin with code (`Cargo.toml`) is built for `wasm32-wasip2`;
3. the folder is packed into `dist/<id>-<version>.tar.gz` — deterministically: sorted, no times,
   no owners — without its sources and build files;
4. `dist/index.json` lists every plugin: its manifest, its package's URL, SHA-256 and size, its
   icon, README, languages, themes and icon sets (the format Flux reads, `flux_plugin::catalog`).

Usage:
    python3 scripts/build.py                          # everything, packages on GitHub Releases
    python3 scripts/build.py --base-url file://$PWD/dist   # a local index for Flux
    python3 scripts/build.py --check                  # validate (and build grammars), no packages
    python3 scripts/build.py --only flux.rust --force # one plugin, grammars rebuilt

Needs Python 3.11+, git, and for grammars the tree-sitter CLI 0.27.1 (`TREE_SITTER` or PATH);
for plugins with code, cargo with the `wasm32-wasip2` target.
"""

from __future__ import annotations

import argparse
import datetime
import gzip
import hashlib
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PLUGINS = ROOT / "plugins"
DIST = ROOT / "dist"
BUILD = ROOT / ".build"

# The repository the packages are published in (GitHub Releases, a tag per plugin version).
DEFAULT_REPO = "egor4geev/flux-plugins"
# The plugin API this catalog is for (Flux runs plugins of this API only).
API_VERSION = "0.2"
INDEX_FORMAT = 1

ID = re.compile(r"^[a-z0-9]+([.-][a-z0-9]+)*$")
# Files of a plugin's folder that are not part of its package: build inputs and outputs.
EXCLUDED_TOP = {"grammars.toml", "Cargo.toml", "Cargo.lock", "src", "target", "build.rs"}


class BuildError(Exception):
    pass


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--base-url", help="where the packages are downloaded from (default: GitHub Releases)")
    parser.add_argument("--repo", default=DEFAULT_REPO, help="GitHub repository of the releases")
    parser.add_argument("--only", nargs="*", help="build only these plugin ids")
    parser.add_argument("--check", action="store_true", help="validate and build, but write no packages")
    parser.add_argument("--force", action="store_true", help="rebuild grammars that are already built")
    parser.add_argument("--skip-grammars", action="store_true", help="use the grammars already built")
    parser.add_argument("--date", help="the publication date of every plugin (YYYY-MM-DD)")
    args = parser.parse_args()

    folders = sorted(path for path in PLUGINS.iterdir() if (path / "flux-plugin.toml").is_file()) if PLUGINS.is_dir() else []
    if args.only:
        wanted = set(args.only)
        folders = [folder for folder in folders if folder.name in wanted]
        missing = wanted - {folder.name for folder in folders}
        if missing:
            print(f"error: no plugin folder for {', '.join(sorted(missing))}", file=sys.stderr)
            return 1
    if not folders:
        print("error: no plugins in plugins/", file=sys.stderr)
        return 1

    entries = []
    failures = []
    for folder in folders:
        try:
            entry = build_plugin(folder, args)
            if entry is not None:
                entries.append(entry)
            print(f"ok    {folder.name}")
        except BuildError as err:
            failures.append((folder.name, str(err)))
            print(f"FAIL  {folder.name}: {err}", file=sys.stderr)

    if failures:
        print(f"\n{len(failures)} plugin(s) failed", file=sys.stderr)
        return 1
    if args.check:
        print(f"\n{len(folders)} plugin(s) checked")
        return 0

    entries.sort(key=lambda entry: entry["name"].lower())
    if args.only and (DIST / "index.json").is_file():
        # Keep the other plugins of an earlier full build.
        previous = json.loads((DIST / "index.json").read_text())
        built = {entry["id"] for entry in entries}
        entries = [entry for entry in previous.get("plugins", []) if entry["id"] not in built] + entries
        entries.sort(key=lambda entry: entry["name"].lower())
    index = {
        "format": INDEX_FORMAT,
        "generated": datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
        "plugins": entries,
    }
    DIST.mkdir(exist_ok=True)
    (DIST / "index.json").write_text(json.dumps(index, indent=2, ensure_ascii=False) + "\n")
    print(f"\n{len(entries)} plugin(s) → {DIST / 'index.json'}")
    return 0


def build_plugin(folder: Path, args) -> dict | None:
    manifest_text = (folder / "flux-plugin.toml").read_text()
    try:
        manifest = tomllib.loads(manifest_text)
    except tomllib.TOMLDecodeError as err:
        raise BuildError(f"flux-plugin.toml: {err}") from err
    check_manifest(folder, manifest)
    if not args.skip_grammars:
        build_grammars(folder, manifest, args.force)
    if (folder / "Cargo.toml").is_file():
        build_code(folder, manifest)
    check_files(folder, manifest)
    if args.check:
        return None

    package = pack(folder, manifest)
    file_name = f"{manifest['id']}-{manifest['version']}.tar.gz"
    DIST.mkdir(exist_ok=True)
    (DIST / file_name).write_bytes(package)
    if args.base_url:
        download = f"{args.base_url.rstrip('/')}/{file_name}"
    else:
        download = f"https://github.com/{args.repo}/releases/download/{manifest['id']}-v{manifest['version']}/{file_name}"
    return index_entry(folder, manifest, manifest_text, package, download, args.date)


# --- Checks ---

def check_manifest(folder: Path, manifest: dict) -> None:
    for key in ("id", "name", "version", "api"):
        if not isinstance(manifest.get(key), str) or not manifest[key].strip():
            raise BuildError(f"flux-plugin.toml: `{key}` is missing")
    plugin_id = manifest["id"]
    if not ID.match(plugin_id):
        raise BuildError(f"id {plugin_id!r}: lowercase letters and digits, words joined by '.' or '-'")
    if folder.name != plugin_id:
        raise BuildError(f"the folder is {folder.name!r}, but the plugin's id is {plugin_id!r}: name the folder by the id")
    if manifest["api"] != API_VERSION:
        raise BuildError(f"api {manifest['api']!r}: the catalog takes plugins for API {API_VERSION}")
    if not re.match(r"^\d+\.\d+\.\d+([-+].*)?$", manifest["version"]):
        raise BuildError(f"version {manifest['version']!r}: MAJOR.MINOR.PATCH")
    grammars = {grammar.get("id") for grammar in manifest.get("grammars", [])}
    for language in manifest.get("languages", []):
        if language.get("grammar") not in grammars:
            raise BuildError(f"language {language.get('id')!r}: its grammar isn't in [[grammars]]")
    for grammar in manifest.get("grammars", []):
        if "builtin" in grammar:
            raise BuildError(f"grammar {grammar.get('id')!r}: catalog plugins bring their grammars (`wasm`), not Flux's built-in ones")


def named_files(manifest: dict) -> list[str]:
    """The files of the folder the manifest names."""
    files = []
    if manifest.get("icon"):
        files.append(manifest["icon"])
    if manifest.get("wasm"):
        files.append(manifest["wasm"])
    for language in manifest.get("languages", []):
        highlights = language.get("highlights", [])
        files.extend([highlights] if isinstance(highlights, str) else highlights)
        if language.get("icon"):
            files.append(language["icon"])
    for grammar in manifest.get("grammars", []):
        if grammar.get("wasm"):
            files.append(grammar["wasm"])
    for window in manifest.get("tool-windows", []):
        if window.get("icon"):
            files.append(window["icon"])
    files.extend(theme["file"] for theme in manifest.get("themes", []))
    files.extend(icons["file"] for icons in manifest.get("icon-themes", []))
    return files


def check_files(folder: Path, manifest: dict) -> None:
    for relative in named_files(manifest):
        path = folder / relative
        if ".." in Path(relative).parts or Path(relative).is_absolute():
            raise BuildError(f"{relative}: a path inside the plugin's folder")
        if not path.is_file():
            raise BuildError(f"{relative} is missing")
        if relative.endswith(".wasm") and path.read_bytes()[:4] != b"\0asm":
            raise BuildError(f"{relative} is not WebAssembly")


# --- Grammars ---

def tree_sitter() -> str:
    tool = os.environ.get("TREE_SITTER") or shutil.which("tree-sitter")
    if not tool:
        raise BuildError("the tree-sitter CLI 0.27.1 is needed: set TREE_SITTER or put it on PATH")
    return tool


def build_grammars(folder: Path, manifest: dict, force: bool) -> None:
    sources_file = folder / "grammars.toml"
    if not sources_file.is_file():
        return
    sources = tomllib.loads(sources_file.read_text()).get("grammars", [])
    outputs = {grammar["id"]: grammar.get("wasm") for grammar in manifest.get("grammars", [])}
    for source in sources:
        grammar_id = source.get("id")
        output = outputs.get(grammar_id)
        if not output:
            raise BuildError(f"grammars.toml: {grammar_id!r} isn't a `wasm` grammar of the manifest")
        target = folder / output
        if target.is_file() and not force:
            continue
        checkout = fetch_source(source)
        grammar_dir = (checkout / source.get("path", ".")).resolve()
        if not (grammar_dir / "src" / "parser.c").is_file():
            raise BuildError(f"grammar {grammar_id!r}: no src/parser.c in {source.get('path', '.')}")
        target.parent.mkdir(parents=True, exist_ok=True)
        run([tree_sitter(), "build", "--wasm", "-o", str(target), str(grammar_dir)], f"grammar {grammar_id!r}")


def fetch_source(source: dict) -> Path:
    repo, rev = source.get("repo"), source.get("rev")
    if not repo or not rev:
        raise BuildError(f"grammars.toml: {source.get('id')!r} needs `repo` and `rev`")
    name = re.sub(r"[^A-Za-z0-9._-]", "_", f"{repo.rstrip('/').split('/')[-1]}@{rev}")
    checkout = BUILD / "sources" / name
    if (checkout / ".fetched").is_file():
        return checkout
    shutil.rmtree(checkout, ignore_errors=True)
    checkout.mkdir(parents=True)
    run(["git", "init", "--quiet"], "git init", cwd=checkout)
    run(["git", "remote", "add", "origin", repo], "git remote", cwd=checkout)
    run(["git", "fetch", "--quiet", "--depth", "1", "origin", rev], f"fetch {repo}@{rev}", cwd=checkout)
    run(["git", "checkout", "--quiet", "FETCH_HEAD"], "git checkout", cwd=checkout)
    (checkout / ".fetched").write_text(f"{repo} {rev}\n")
    return checkout


# --- Plugins with code ---

def build_code(folder: Path, manifest: dict) -> None:
    if not manifest.get("wasm"):
        raise BuildError("there is a Cargo.toml, but the manifest names no `wasm` component")
    run(["cargo", "build", "--release", "--target", "wasm32-wasip2"], "cargo build", cwd=folder)


def run(command: list[str], what: str, cwd: Path | None = None) -> None:
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True)
    if result.returncode != 0:
        tail = "\n".join((result.stderr or result.stdout).strip().splitlines()[-12:])
        raise BuildError(f"{what} failed:\n{tail}")


# --- Packages ---

def package_files(folder: Path, manifest: dict) -> list[Path]:
    """The files of the package: everything but sources, build files and hidden files; of
    `target/` only the component."""
    component = manifest.get("wasm")
    files = []
    for path in sorted(folder.rglob("*")):
        relative = path.relative_to(folder)
        if not path.is_file() or any(part.startswith(".") for part in relative.parts):
            continue
        if relative.parts[0] in EXCLUDED_TOP and relative.as_posix() != component:
            continue
        files.append(relative)
    return files


def pack(folder: Path, manifest: dict) -> bytes:
    """A deterministic `.tar.gz` of the plugin: the same files give the same bytes."""
    raw = io.BytesIO()
    with tarfile.open(fileobj=raw, mode="w", format=tarfile.PAX_FORMAT) as tar:
        for relative in package_files(folder, manifest):
            data = (folder / relative).read_bytes()
            info = tarfile.TarInfo(relative.as_posix())
            info.size = len(data)
            info.mode = 0o644
            info.mtime = 0
            info.uid = info.gid = 0
            info.uname = info.gname = ""
            tar.addfile(info, io.BytesIO(data))
    out = io.BytesIO()
    with gzip.GzipFile(fileobj=out, mode="wb", mtime=0, compresslevel=9) as gz:
        gz.write(raw.getvalue())
    return out.getvalue()


# --- The index ---

def categories(manifest: dict) -> list[str]:
    found = []
    if manifest.get("languages") or manifest.get("language-servers"):
        found.append("languages")
    if manifest.get("themes"):
        found.append("themes")
    if manifest.get("icon-themes"):
        found.append("icons")
    if manifest.get("wasm") or not found:
        found.append("tools")
    return found


def read_toml(folder: Path, relative: str) -> dict:
    try:
        return tomllib.loads((folder / relative).read_text())
    except (OSError, tomllib.TOMLDecodeError) as err:
        raise BuildError(f"{relative}: {err}") from err


def publication_date(folder: Path, fixed: str | None) -> str:
    if fixed:
        return fixed
    result = subprocess.run(["git", "log", "-1", "--format=%cs", "--", str(folder)], cwd=ROOT, capture_output=True, text=True)
    date = result.stdout.strip()
    return date or datetime.date.today().isoformat()


def index_entry(folder: Path, manifest: dict, manifest_text: str, package: bytes, download: str, date: str | None) -> dict:
    icon_svg = None
    if manifest.get("icon") and (folder / manifest["icon"]).is_file():
        icon_svg = (folder / manifest["icon"]).read_text()
    readme = (folder / "README.md").read_text() if (folder / "README.md").is_file() else None
    languages = [
        {
            "id": language["id"],
            "name": language.get("name", language["id"]),
            "extensions": [extension.lower() for extension in language.get("extensions", [])],
            "file_names": language.get("file-names", []),
        }
        for language in manifest.get("languages", [])
    ]
    themes = []
    for theme in manifest.get("themes", []):
        data = read_toml(folder, theme["file"])
        if not isinstance(data.get("name"), str):
            raise BuildError(f"{theme['file']}: the theme has no `name`")
        themes.append({"name": data["name"], "appearance": data.get("appearance", "dark")})
    icon_themes = []
    for icons in manifest.get("icon-themes", []):
        data = read_toml(folder, icons["file"])
        if not isinstance(data.get("name"), str):
            raise BuildError(f"{icons['file']}: the icon set has no `name`")
        icon_themes.append(data["name"])
    locales = {}
    for path in sorted((folder / "locales").glob("*.toml")) if (folder / "locales").is_dir() else []:
        try:
            table = tomllib.loads(path.read_text())
        except tomllib.TOMLDecodeError as err:
            raise BuildError(f"locales/{path.name}: {err}") from err
        strings = {key: value for key, value in table.items() if isinstance(value, str)}
        if strings:
            locales[path.stem] = strings
    return {
        "id": manifest["id"],
        "name": manifest["name"],
        "version": manifest["version"],
        "api": manifest["api"],
        "description": manifest.get("description", ""),
        "authors": manifest.get("authors", []),
        "repository": manifest.get("repository"),
        "categories": categories(manifest),
        "manifest": manifest_text,
        "download": download,
        "sha256": hashlib.sha256(package).hexdigest(),
        "size": len(package),
        "icon_svg": icon_svg,
        "readme": readme,
        "languages": languages,
        "themes": themes,
        "icon_themes": icon_themes,
        "updated": publication_date(folder, date),
        "locales": locales,
    }


if __name__ == "__main__":
    sys.exit(main())
