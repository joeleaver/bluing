use la_arena::Arena;

use crate::{CrateId, Name, VfsPath};

/// The crates of one compilation and their dependencies.
#[derive(Debug, Default)]
pub struct CrateGraph {
    crates: Arena<CrateData>,
}

#[derive(Debug)]
pub struct CrateData {
    pub name: Name,
    pub root: VfsPath,
    pub kind: CrateKind,
    /// The crates this one can name; their names form its extern prelude.
    pub deps: Vec<CrateId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CrateKind {
    /// A script package. Gets the standard prelude of its `std` dependency.
    Script { allow_unsafe: bool },
    /// A library compiled in library mode (Bluing's `std`): `unsafe` and
    /// `#[lang]` attributes allowed, no implicit prelude.
    Library,
}

impl CrateGraph {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&mut self, data: CrateData) -> CrateId {
        self.crates.alloc(data)
    }

    pub fn crates(&self) -> impl Iterator<Item = (CrateId, &CrateData)> {
        self.crates.iter()
    }
}

impl std::ops::Index<CrateId> for CrateGraph {
    type Output = CrateData;

    fn index(&self, id: CrateId) -> &CrateData {
        &self.crates[id]
    }
}
