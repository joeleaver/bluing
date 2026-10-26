//! The Bluing compiler driver. [`check`] runs every phase on a script crate:
//! loading and parsing, name resolution, body lowering, signature types, and
//! type checking of bodies.
//!
//! The standard library is embedded; hosts only supply the script's files.

use bluing_base::Diagnostic;
use bluing_hir::{CrateData, CrateGraph, CrateKind, Files, Layered, Name};

pub use bluing_hir::{MemoryProvider, MissingCrateRoot, SourceProvider, VfsPath};

#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Let the script use `unsafe`.
    pub allow_unsafe: bool,
}

/// The outcome of checking a script crate.
pub struct Checked {
    pub files: Files,
    /// Every diagnostic, ordered by file and position.
    pub diagnostics: Vec<Diagnostic>,
}

/// Checks the script crate rooted at `root`, reading its files from
/// `provider`.
pub fn check(
    root: VfsPath,
    provider: &dyn SourceProvider,
    options: &Options,
) -> Result<Checked, MissingCrateRoot> {
    let std_files = std_provider();
    let provider = Layered(vec![&std_files, provider]);
    let mut graph = CrateGraph::new();
    let std = graph.add(CrateData {
        name: Name::new("std"),
        root: VfsPath::new(bluing_std::ROOT),
        kind: CrateKind::Library,
        deps: Vec::new(),
    });
    graph.add(CrateData {
        name: Name::new("script"),
        root,
        kind: CrateKind::Script {
            allow_unsafe: options.allow_unsafe,
        },
        deps: vec![std],
    });
    let analysis = bluing_hir::analyze(&graph, &provider)?;
    let mut types = bluing_ty::collect(&analysis, &graph);
    bluing_typeck::typeck(&mut types, &analysis.bodies);
    let mut diagnostics = analysis.diagnostics.clone();
    diagnostics.extend(types.diagnostics);
    diagnostics.sort_by_key(|d| (d.file, d.range.start(), d.range.end()));
    Ok(Checked {
        files: analysis.files,
        diagnostics,
    })
}

/// The standard library's files.
fn std_provider() -> MemoryProvider {
    let mut files = MemoryProvider::new();
    for (path, text) in bluing_std::files() {
        files.insert(VfsPath::new(format!("{}/{path}", bluing_std::DIR)), *text);
    }
    files
}
