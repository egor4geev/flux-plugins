#!/usr/bin/env python3
"""Flux Tiles: the generator of this full-color set of file icons.

Every icon is one of Flux's own file-type glyphs (the bundled set `flux.icons`, made by Flux's
`scripts/gen-icons.py`) drawn in white — or near-black where white wouldn't read — on a rounded tile
of the file type's color. The set covers exactly the files Flux Icons covers: this script reads Flux
Icons' set file and gives each of its entries a tile.

    python3 src/gen-tiles.py [<flux>/plugins/icons]

The argument is Flux's `plugins/icons` folder (by default the `flux` repository next to this one);
the script writes `icons/*.svg` and `icon-themes/tiles.toml` of this plugin. Rules for the result
(full-color icons, `tint`): Flux's `docs/icon-themes.md`. Needs Python 3.11+ (`tomllib`).
"""
import os
import re
import sys
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent.parent
DEFAULT_FLUX_ICONS = HERE.parent.parent.parent / "flux" / "plugins" / "icons"

WHITE = "#ffffff"
INK = "#1d1f24"

# The tile of each glyph: (background, glyph color). Colors of the languages' own brands where they
# have one, otherwise the hue Flux Icons gives the file type, saturated enough to carry a white glyph
# on light and dark themes alike.
TILES = {
    "rust": ("#d9542b", WHITE),
    "typescript": ("#3178c6", WHITE),
    "javascript": ("#efcf3a", INK),
    "react": ("#1592c4", WHITE),
    "json": ("#e5a52e", INK),
    "markdown": ("#5a63e0", WHITE),
    "readme": ("#239c8f", WHITE),
    "toml": ("#7a808c", WHITE),
    "yaml": ("#cf4277", WHITE),
    "python": ("#3670a8", "#ffd84a"),
    "go": ("#0596c4", WHITE),
    "shell": ("#2c8f55", WHITE),
    "swift": ("#ec5236", WHITE),
    "c": ("#5466c6", WHITE),
    "html": ("#e04f26", WHITE),
    "css": ("#7b4bd0", WHITE),
    "lock": ("#6b7180", WHITE),
    "git": ("#e8502f", WHITE),
    "docker": ("#1f8ee0", WHITE),
    "license": ("#e3b02a", INK),
    "package": ("#d9542b", WHITE),
    "config": ("#6d9f2f", WHITE),
    "image": ("#9a5ae6", WHITE),
    "archive": ("#b2741c", WHITE),
    "text": ("#7d8592", WHITE),
    "code": ("#64708a", WHITE),
    "file": ("#7d8592", WHITE),
}

# A glyph Flux Icons draws in several colors (the color tells the kind): a tile per color.
BY_COLOR = {
    ("package", "red"): ("package-npm", "#cb3837", WHITE),
    ("code", "red"): ("code-red", "#c9352e", WHITE),
    ("code", "orange"): ("code-orange", "#e0701c", WHITE),
    ("code", "violet"): ("code-violet", "#7d55e6", WHITE),
    ("code", "indigo"): ("code-indigo", "#5c62c9", WHITE),
    ("code", "blue"): ("code-blue", "#2f73c2", WHITE),
    ("code", "amber"): ("code-amber", "#d68a12", WHITE),
    ("code", "green"): ("code-green", "#2f9a68", WHITE),
}

# Tiles: 14 px with a 1 px margin, as Flux's glyphs leave about that much; the glyph inside at 60 %.
TILE = '<rect x="1" y="1" width="14" height="14" rx="3.5" fill="{bg}"/>'
SCALE = 0.6
OFFSET = (16 - 16 * SCALE) / 2
# Strokes get thicker before the glyph is scaled down, so they stay crisp at 16 px.
STROKE_WIDTH = "2.1"

# Flux's knockout glyph of TypeScript and JavaScript is a rounded square with letters cut out: on a
# tile, the square is the tile, so only the letters are drawn.
SQUARE = '<rect x="1.5" y="1.5" width="13" height="13" rx="2.75"/>'


def recolor_outside_masks(content, color):
    """Black → `color`, except inside `<mask>`: a mask's black hides, it isn't a color."""
    parts = re.split(r"(<mask\b.*?</mask>)", content, flags=re.S)
    return "".join(part if part.startswith("<mask") else part.replace("#000", color) for part in parts)


def glyph_body(svg, color):
    """The glyph of a Flux icon in `color`, for a `<g>` of the tile."""
    match = re.match(r'\s*<svg\b([^>]*)>(.*)</svg>\s*$', svg, flags=re.S)
    if not match:
        raise ValueError("not an SVG")
    attrs, content = match.group(1), match.group(2).strip()
    group = []
    for name in ("fill", "stroke", "stroke-linecap", "stroke-linejoin"):
        value = re.search(rf'\b{name}="([^"]*)"', attrs)
        if value:
            group.append(f'{name}="{value.group(1).replace("#000", color)}"')
    if re.search(r'\bstroke="#000"', attrs):
        group.append(f'stroke-width="{STROKE_WIDTH}"')
    attributes = " ".join(group)
    return f'<g {attributes}>' if attributes else "<g>", recolor_outside_masks(content, color)


def letters_on_square(svg, color):
    """The letters cut out of Flux's rounded square (TypeScript, JavaScript), in `color`: on a tile
    the square is the tile, so the letters keep their place and size."""
    cut = re.search(r'<mask\b.*?<rect[^>]*/>(<g\b[^>]*>.*?</g>)</mask>', svg, flags=re.S)
    return cut.group(1).replace('stroke="#000"', f'stroke="{color}"')


def tile_svg(glyph_svg, bg, fg):
    head = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">' + TILE.format(bg=bg)
    if SQUARE in glyph_svg:
        return head + letters_on_square(glyph_svg, fg) + "</svg>\n"
    group, body = glyph_body(glyph_svg, fg)
    return (
        head
        + f'<g transform="translate({OFFSET:g} {OFFSET:g}) scale({SCALE:g})">{group}{body}</g></g>'
        + "</svg>\n"
    )


def main():
    flux_icons = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_FLUX_ICONS
    with open(flux_icons / "icon-themes" / "flux.toml", "rb") as f:
        flux = tomllib.load(f)
    out_icons = HERE / "icons"
    out_icons.mkdir(exist_ok=True)
    for stale in out_icons.glob("*.svg"):
        if stale.name != "tiles.svg":
            stale.unlink()

    made = {}

    def tile_for(entry):
        """The tile file for a Flux Icons entry: made once per glyph and color."""
        glyph = Path(entry["icon"]).stem
        color = entry.get("color")
        name, bg, fg = BY_COLOR.get((glyph, color)) or (glyph, *TILES[glyph])
        if name not in made:
            svg = (flux_icons / "icons" / f"{glyph}.svg").read_text()
            (out_icons / f"{name}.svg").write_text(tile_svg(svg, bg, fg))
            made[name] = bg
        return name

    def entry_line(key, entry):
        name = tile_for(entry)
        return f'"{key}" = {{ icon = "icons/{name}.svg", tint = "{made[name]}" }}'

    lines = [
        "# Flux Tiles: generated by `src/gen-tiles.py` from Flux Icons — edit the script, not this file.",
        "# Full-color icons (no `color`): white glyphs on tiles of the file type's color; `tint` is",
        "# the tile's color for what goes along with the file (the status bar's badge).",
        'name = "Flux Tiles"',
        "",
        "[defaults]",
    ]
    # Plain files get a gray tile with the document glyph; folders stay Flux's (a tree of colored
    # folders would shout).
    file_svg = (flux_icons.parent.parent / "crates" / "flux-app" / "assets" / "icons" / "file.svg")
    if not file_svg.exists():
        raise SystemExit(f"{file_svg}: Flux's plain file icon is needed for the default tile")
    (out_icons / "file.svg").write_text(tile_svg(file_svg.read_text(), *TILES["file"]))
    made["file"] = TILES["file"][0]
    lines.append(f'file = {{ icon = "icons/file.svg", tint = "{made["file"]}" }}')
    for section in ("file-names", "name-prefixes", "extensions"):
        lines += ["", f"[{section}]"]
        for key, entry in flux.get(section, {}).items():
            lines.append(entry_line(key, entry))
    (HERE / "icon-themes").mkdir(exist_ok=True)
    (HERE / "icon-themes" / "tiles.toml").write_text("\n".join(lines) + "\n")
    # The plugin's own icon in the Marketplace: monochrome, as plugin icons are.
    print(f"{len(made)} tiles, {sum(len(flux.get(s, {})) for s in ('file-names', 'name-prefixes', 'extensions'))} entries")


if __name__ == "__main__":
    main()
