//! The module tree, HIR and name resolution of the Bluing compiler.
//!
//! [`analyze`] loads every crate of a [`CrateGraph`] through a
//! [`SourceProvider`], lowers all items to the [`Hir`], and resolves imports,
//! visibilities and every path in item signatures.

pub mod body;
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

pub use body::{Body, BodyOwner};
pub use resolve::Resolutions;

use std::collections::BTreeMap;

/// The result of analyzing a compilation.
pub struct Analysis {
    pub hir: Hir,
    pub files: Files,
    pub resolutions: Resolutions,
    pub bodies: BTreeMap<BodyOwner, Body>,
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
    let (bodies, body_diagnostics) =
        body::lower_bodies(&collected.hir, graph, &resolutions, &collected.files);
    let mut diagnostics = collected.diagnostics;
    diagnostics.extend(resolve_diagnostics);
    diagnostics.extend(body_diagnostics);
    diagnostics.sort_by_key(|d| (d.file, d.range.start(), d.range.end()));
    Ok(Analysis {
        hir: collected.hir,
        files: collected.files,
        resolutions,
        bodies,
        diagnostics,
    })
}
