//! Embeds every file under `library/` so the compiler carries the standard
//! library's sources with it, in the browser and on the desktop alike.

use std::{env, fs, path::Path};

fn main() {
    let library = Path::new(env!("CARGO_MANIFEST_DIR")).join("library");
    println!("cargo::rerun-if-changed={}", library.display());
    let mut files = Vec::new();
    collect(&library, &library, &mut files);
    files.sort();
    let mut out = String::from("&[\n");
    for (relative, absolute) in &files {
        println!("cargo::rerun-if-changed={absolute}");
        out.push_str(&format!(
            "    ({relative:?}, include_str!({absolute:?})),\n"
        ));
    }
    out.push_str("]\n");
    let dest = Path::new(&env::var("OUT_DIR").expect("cargo sets OUT_DIR")).join("files.rs");
    fs::write(dest, out).expect("write the embedded file table");
}

/// Collects `(path relative to the library root with `/` separators,
/// absolute path)` for every file under `dir`.
fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let entries = fs::read_dir(dir).unwrap_or_else(|err| panic!("read {}: {err}", dir.display()));
    for entry in entries {
        let path = entry.expect("read a directory entry").path();
        println!("cargo::rerun-if-changed={}", path.display());
        if path.is_dir() {
            collect(root, &path, out);
        } else {
            let relative = path.strip_prefix(root).expect("path is under the root");
            let relative: Vec<_> = relative.iter().map(|part| part.to_string_lossy()).collect();
            out.push((relative.join("/"), path.to_string_lossy().into_owned()));
        }
    }
}
