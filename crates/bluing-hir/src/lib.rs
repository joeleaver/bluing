//! The module tree, HIR and name resolution of the Bluing compiler.
//!
//! [`analyze`] loads every crate of a [`CrateGraph`] through a
//! [`SourceProvider`], lowers all items to the [`Hir`], and resolves imports,
//! visibilities and every path in item signatures.

mod crate_graph;
mod files;
pub mod hir;
mod ids;
mod lower;
mod name;
pub mod resolve;
mod vfs;

pub use bluing_base::{Diagnostic, FileId};
pub use crate_graph::{CrateData, CrateGraph, CrateKind};
pub use files::{FileData, Files};
pub use hir::Hir;
pub use ids::*;
pub use lower::{EXTENSION, MissingCrateRoot};
pub use name::Name;
pub use vfs::{Layered, MemoryProvider, SourceProvider, VfsPath};

pub use resolve::Resolutions;

/// The result of analyzing a compilation.
pub struct Analysis {
    pub hir: Hir,
    pub files: Files,
    pub resolutions: Resolutions,
    /// Every diagnostic, ordered by file and position.
    pub diagnostics: Vec<Diagnostic>,
}

/// Loads, lowers and resolves every crate of `graph`.
pub fn analyze(
    graph: &CrateGraph,
    provider: &dyn SourceProvider,
) -> Result<Analysis, MissingCrateRoot> {
    let collected = lower::collect(graph, provider)?;
    let (resolutions, resolve_diagnostics) =
        resolve::resolve(&collected.hir, graph, collected.crate_roots);
    let mut diagnostics = collected.diagnostics;
    diagnostics.extend(resolve_diagnostics);
    diagnostics.sort_by_key(|d| (d.file, d.range.start(), d.range.end()));
    Ok(Analysis {
        hir: collected.hir,
        files: collected.files,
        resolutions,
        diagnostics,
    })
}
