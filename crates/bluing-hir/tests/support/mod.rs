//! Shared helpers: build a compilation of a script crate plus the embedded
//! standard library from in-memory files.

#![allow(dead_code)] // Each test binary uses a different subset.

use bluing_base::Code;
use bluing_hir::{
    Analysis, CrateData, CrateGraph, CrateKind, MemoryProvider, Name, VfsPath, analyze,
};

/// The directory script files live under in tests.
pub const ROOT_DIR: &str = "/script";

/// Analyzes a script crate whose files are `(path relative to the crate
/// directory, text)`; the first file is the crate root.
pub fn analyze_files(files: &[(&str, &str)]) -> Analysis {
    analyze_with(files, false)
}

pub fn analyze_with(files: &[(&str, &str)], allow_unsafe: bool) -> Analysis {
    let mut provider = MemoryProvider::new();
    for (path, text) in bluing_std::files() {
        provider.insert(VfsPath::new(format!("{}/{path}", bluing_std::DIR)), *text);
    }
    for (path, text) in files {
        provider.insert(VfsPath::new(format!("{ROOT_DIR}/{path}")), *text);
    }
    let mut graph = CrateGraph::new();
    let std = graph.add(CrateData {
        name: Name::new("std"),
        root: VfsPath::new(bluing_std::ROOT),
        kind: CrateKind::Library,
        deps: Vec::new(),
    });
    graph.add(CrateData {
        name: Name::new("script"),
        root: VfsPath::new(format!("{ROOT_DIR}/{}", files[0].0)),
        kind: CrateKind::Script { allow_unsafe },
        deps: vec![std],
    });
    analyze(&graph, &provider).expect("crate roots exist")
}

/// The codes of every diagnostic, in order.
pub fn codes(analysis: &Analysis) -> Vec<Code> {
    analysis.diagnostics.iter().map(|d| d.code).collect()
}

/// Every diagnostic rendered as `path:line:col: code: message`, for failure
/// messages.
pub fn render(analysis: &Analysis) -> String {
    analysis
        .diagnostics
        .iter()
        .map(|d| {
            let file = analysis.files.get(d.file);
            let pos = bluing_base::LineIndex::new(&file.text).line_col(d.range.start());
            format!(
                "{}:{}:{}: {}: {}\n",
                file.path, pos.line, pos.col, d.code, d.message
            )
        })
        .collect()
}
