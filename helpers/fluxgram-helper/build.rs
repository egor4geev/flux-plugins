//! Links TDLib in statically: the static libraries of the pinned `tdlib-rs` release for the target
//! (TDLib 1.8.61 with OpenSSL and zlib), downloaded and checked against their SHA-256 into
//! `target/tdlib/<platform>/` once — or taken from `TDLIB_DIR`, a folder with `lib/` (a TDLib of
//! one's own build: the same libraries).
//!
//! The download needs `curl` and `tar` (both come with macOS and Windows 10+).
//!
//! The renderers of animated stickers (`fluxgram.render`) are built from their sources in `native/`:
//! rlottie (TGS) and libvpx's VP9 decoder (WEBM) — only the files compiled, portable C and C++ with
//! no assembly or SIMD of their own, so the one list builds for every target (README.md → Building).

use std::env;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use sha2::{Digest, Sha256};

/// The release of `tdlib-rs` with TDLib's prebuilt libraries: `tdlib-<version>-<platform>.zip`.
const RELEASE: &str = "https://github.com/FedericoBruzzone/tdlib-rs/releases/download/v1.4.0";
const TDLIB_VERSION: &str = "1.8.61";

/// The archives this program is built from, by platform, with their SHA-256 (pinned: a release's
/// file could be replaced).
const ARCHIVES: &[(&str, &str)] = &[
    (
        "macos-aarch64",
        "6600d1fbc6ebf7a3df669b9ae84d48265a28f3c7456a2b64cfd0327aceb88620",
    ),
    (
        "macos-x86_64",
        "2e96b32a8b92d86629b1f116f1e40fd2eed9e7373584cfd947304b90a07013a6",
    ),
    (
        "windows-x86_64",
        "20f226518578b040028556bb862668f6802d16d3a5726df3dccb867dca4e9de1",
    ),
    (
        "windows-aarch64",
        "ff37a0edc69c738d5b91bafea53a88983f97be4844b107f8b938f8fd4ff57559",
    ),
];

/// TDLib's libraries, the JSON interface first; OpenSSL and zlib come after (their names differ
/// by platform). Static archives: the linkers take what's used, in any order.
const TDLIB_LIBS: &[&str] = &[
    "tdjson_static",
    "tdjson_private",
    "tdclient",
    "tdcore",
    "tdapi",
    "tdmtproto",
    "tdnet",
    "tddb",
    "tdsqlite",
    "tde2e",
    "tdactor",
    "tdutils",
];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=TDLIB_DIR");

    let os = env::var("CARGO_CFG_TARGET_OS").unwrap();
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap();
    let platform = match (os.as_str(), arch.as_str()) {
        ("macos", "aarch64") => "macos-aarch64",
        ("macos", "x86_64") => "macos-x86_64",
        ("windows", "x86_64") => "windows-x86_64",
        ("windows", "aarch64") => "windows-aarch64",
        _ => panic!(
            "fluxgram-helper is built for macOS and Windows (aarch64, x86_64), not {os} {arch}"
        ),
    };

    build_native(&os);

    let lib = match env::var_os("TDLIB_DIR") {
        Some(dir) => PathBuf::from(dir).join("lib"),
        None => fetch(platform),
    };

    if os == "macos" {
        link_static(&lib, TDLIB_LIBS.iter().map(|name| format!("lib{name}.a")));
        link_static(
            &lib,
            ["libssl.a", "libcrypto.a", "libz.a"].map(String::from),
        );
        // TDLib is C++.
        println!("cargo:rustc-link-lib=c++");
    } else {
        link_static(&lib, TDLIB_LIBS.iter().map(|name| format!("{name}.lib")));
        link_static(
            &lib,
            ["libssl.lib", "libcrypto.lib", "zlib.lib"].map(String::from),
        );
        // What TDLib and OpenSSL take from Windows (TDLib's own CMake files list the same).
        for system in [
            "ws2_32", "mswsock", "crypt32", "normaliz", "psapi", "shell32", "advapi32", "user32",
            "bcrypt",
        ] {
            println!("cargo:rustc-link-lib=dylib={system}");
        }
        // TDLib's libraries are built for the C runtime in DLLs (/MD), OpenSSL's and zlib's for the
        // static one (/MT): one runtime in the program, the DLLs', as Rust's default.
        println!("cargo:rustc-link-arg=/NODEFAULTLIB:libcmt.lib");
    }
}

/// rlottie, libvpx's VP9 decoder and the program's door to it (`native/vp9.c`), as static
/// libraries: every source file in their folders is compiled (they hold only what is).
fn build_native(os: &str) {
    let native = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("native");
    println!("cargo:rerun-if-changed=native");
    let msvc = env::var("CARGO_CFG_TARGET_ENV").is_ok_and(|target_env| target_env == "msvc");

    // rlottie as its CMake builds it: C++14, no exceptions or RTTI where the compiler allows, the
    // options of its `config.h` off (native/rlottie/config.h).
    let rlottie = native.join("rlottie");
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .std("c++14")
        .warnings(false)
        .cargo_warnings(false)
        .define("NDEBUG", None)
        .include(&rlottie)
        .include(rlottie.join("inc"))
        // Its C API's functions are defined, not imported from a DLL.
        .define("RLOTTIE_BUILD", None)
        .files(sources(&rlottie, "cpp"));
    for dir in [
        "src/lottie",
        "src/lottie/zip",
        "src/vector",
        "src/vector/freetype",
        "src/vector/stb",
        "src/binding/c",
    ] {
        build.include(rlottie.join(dir));
    }
    // Apple's clang says `__ARM_NEON__` on arm64, and rlottie takes its NEON drawing — 32-bit ARM
    // assembly — then: its portable code instead, as on every other target.
    if msvc {
        build
            .flag("/EHsc")
            .flag("/U__ARM_NEON__")
            .define("_USE_MATH_DEFINES", None)
            .define("NOMINMAX", None)
            .define("_CRT_SECURE_NO_WARNINGS", None);
    } else {
        build
            .flag("-fno-exceptions")
            .flag("-fno-rtti")
            .flag("-U__ARM_NEON__");
    }
    build.compile("rlottie");

    // libvpx's VP9 decoder in portable C: its configure's headers for the generic target are
    // kept with the sources (native/libvpx/vpx_config.h and the *_rtcd.h).
    let libvpx = native.join("libvpx");
    let mut build = cc::Build::new();
    build
        .warnings(false)
        .cargo_warnings(false)
        .define("NDEBUG", None)
        .include(&libvpx)
        .files(sources(&libvpx, "c"))
        .file(native.join("vp9.c"));
    if msvc {
        build.define("_CRT_SECURE_NO_WARNINGS", None);
    } else {
        build.flag("-std=gnu11");
    }
    build.compile("vpx");

    if os == "macos" {
        // rlottie is C++ (TDLib's line below says so too).
        println!("cargo:rustc-link-lib=c++");
    } else {
        // rlottie resolves a picture's path with PathCanonicalizeW.
        println!("cargo:rustc-link-lib=dylib=shlwapi");
    }
}

/// The files with the extension under `dir`, in a stable order.
fn sources(dir: &Path, extension: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut dirs = vec![dir.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|ext| ext == extension) {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Passes the archives by their full paths: a system library of the same name (macOS has a libz,
/// for one) can't be taken in their place.
fn link_static(lib: &Path, files: impl IntoIterator<Item = String>) {
    for file in files {
        let path = lib.join(&file);
        assert!(
            path.is_file(),
            "TDLib's library {} is missing",
            path.display()
        );
        println!("cargo:rustc-link-arg={}", path.display());
    }
}

/// The `lib/` folder of the platform's archive: downloaded and checked the first time, then taken
/// from `target/tdlib/<platform>/`.
fn fetch(platform: &str) -> PathBuf {
    let sha256 = ARCHIVES
        .iter()
        .find(|(name, _)| *name == platform)
        .unwrap()
        .1;
    let dir = target_dir().join("tdlib").join(platform);
    let lib = dir.join("tdlib").join("lib");
    // A marker with the archive's checksum: written last, so a half-unpacked folder isn't taken.
    let marker = dir.join(".unpacked");
    if fs::read_to_string(&marker).is_ok_and(|text| text.trim() == sha256) {
        return lib;
    }

    if dir.exists() {
        fs::remove_dir_all(&dir).unwrap();
    }
    fs::create_dir_all(&dir).unwrap();
    let name = format!("tdlib-{TDLIB_VERSION}-{platform}.zip");
    let zip = dir.join(&name);
    let url = format!("{RELEASE}/{name}");
    eprintln!("downloading TDLib {TDLIB_VERSION} ({platform}): {url}");
    let status = Command::new("curl")
        .args(["-fsSL", "--retry", "3", "-o"])
        .arg(&zip)
        .arg(&url)
        .status()
        .expect("curl is needed to download TDLib");
    assert!(status.success(), "couldn't download {url}");

    let actual = file_sha256(&zip);
    assert!(
        actual == sha256,
        "{name}: SHA-256 {actual}, expected {sha256} — the release's file changed"
    );

    // Only the libraries: the archive's DLLs and headers aren't used (the program declares the
    // four functions of td_json_client.h itself).
    let status = Command::new(tar())
        .arg("-xf")
        .arg(&zip)
        .arg("-C")
        .arg(&dir)
        .arg("tdlib/lib")
        .status()
        .expect("tar is needed to unpack TDLib");
    assert!(status.success(), "couldn't unpack {}", zip.display());
    fs::remove_file(&zip).unwrap();
    fs::write(&marker, sha256).unwrap();
    lib
}

/// Cargo's target folder: `CARGO_TARGET_DIR` when it's set, else `target/` next to Cargo.toml.
fn target_dir() -> PathBuf {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    match env::var_os("CARGO_TARGET_DIR").map(PathBuf::from) {
        Some(dir) if dir.is_absolute() => dir,
        Some(dir) => manifest.join(dir),
        None => manifest.join("target"),
    }
}

/// Windows' own bsdtar unpacks zip archives; the GNU tar of Git's tools, first on some PATHs,
/// doesn't.
fn tar() -> PathBuf {
    if cfg!(windows) {
        let root = env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        PathBuf::from(root).join("System32").join("tar.exe")
    } else {
        PathBuf::from("tar")
    }
}

fn file_sha256(path: &Path) -> String {
    let mut file = fs::File::open(path).unwrap();
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 20];
    loop {
        let n = file.read(&mut buf).unwrap();
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}
