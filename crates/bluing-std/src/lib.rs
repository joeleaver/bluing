//! The sources of Bluing's standard library, embedded in the compiler.
//!
//! The library is written in Bluing and compiled in library mode; see
//! DESIGN.md, "Files, crates and the standard library".

/// The virtual directory the library's files live under.
pub const DIR: &str = "builtin:/std";

/// The crate root, under [`DIR`].
pub const ROOT: &str = "builtin:/std/lib.blu";

/// Every file of the library as `(path relative to DIR, contents)`.
pub fn files() -> &'static [(&'static str, &'static str)] {
    include!(concat!(env!("OUT_DIR"), "/files.rs"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn root_is_embedded() {
        let root = super::ROOT
            .strip_prefix(super::DIR)
            .unwrap()
            .trim_start_matches('/');
        assert!(super::files().iter().any(|(path, _)| *path == root));
    }
}
