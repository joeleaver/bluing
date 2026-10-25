//! Virtual file paths, and the trait hosts implement to supply source text.
//!
//! The compiler never touches a real file system: the browser host serves
//! files from memory and the desktop host from disk, both through
//! [`SourceProvider`].

use std::{collections::BTreeMap, fmt};

/// A `/`-separated path. Hosts choose the roots (an absolute OS path,
/// `builtin:/std/lib.blu`, ...); the compiler only splits and joins on `/`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VfsPath(String);

impl VfsPath {
    pub fn new(path: impl Into<String>) -> Self {
        VfsPath(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The directory containing this path, or `None` if it has no `/`.
    pub fn parent(&self) -> Option<VfsPath> {
        self.0.rsplit_once('/').map(|(dir, _)| VfsPath::new(dir))
    }

    /// The last component.
    pub fn file_name(&self) -> &str {
        self.0.rsplit_once('/').map_or(&self.0, |(_, name)| name)
    }

    /// The last component without its extension.
    pub fn file_stem(&self) -> &str {
        let name = self.file_name();
        name.rsplit_once('.').map_or(name, |(stem, _)| stem)
    }

    /// This path with `component` appended.
    pub fn join(&self, component: &str) -> VfsPath {
        VfsPath(format!("{}/{component}", self.0))
    }
}

impl fmt::Display for VfsPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Supplies the text of source files.
pub trait SourceProvider {
    /// The file's text, or `None` if there is no such file.
    fn read(&self, path: &VfsPath) -> Option<String>;
}

/// Files held in memory.
#[derive(Clone, Debug, Default)]
pub struct MemoryProvider {
    files: BTreeMap<VfsPath, String>,
}

impl MemoryProvider {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, path: VfsPath, text: impl Into<String>) {
        self.files.insert(path, text.into());
    }
}

impl SourceProvider for MemoryProvider {
    fn read(&self, path: &VfsPath) -> Option<String> {
        self.files.get(path).cloned()
    }
}

/// Tries each provider in order and returns the first file found.
pub struct Layered<'a>(pub Vec<&'a dyn SourceProvider>);

impl SourceProvider for Layered<'_> {
    fn read(&self, path: &VfsPath) -> Option<String> {
        self.0.iter().find_map(|provider| provider.read(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_joins() {
        let path = VfsPath::new("builtin:/std/option.blu");
        assert_eq!(path.parent(), Some(VfsPath::new("builtin:/std")));
        assert_eq!(path.file_name(), "option.blu");
        assert_eq!(path.file_stem(), "option");
        assert_eq!(VfsPath::new("/a").join("b.blu"), VfsPath::new("/a/b.blu"));
    }
}
