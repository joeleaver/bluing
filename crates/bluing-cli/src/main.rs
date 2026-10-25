//! `blu`, the Bluing command-line tool.

use std::{fs, path::Path, process::ExitCode};

use bluing_base::LineIndex;
use bluing_hir::{
    CrateData, CrateGraph, CrateKind, Layered, MemoryProvider, Name, SourceProvider, VfsPath,
};

const USAGE: &str = "\
usage: blu check [--allow-unsafe] <main.blu>
       blu --version";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("check") => check(&args[1..]),
        Some("--version" | "-V") => {
            println!("blu {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => usage_error(),
    }
}

fn usage_error() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

/// Checks the script crate rooted at the given file.
fn check(args: &[String]) -> ExitCode {
    let mut allow_unsafe = false;
    let mut roots = Vec::new();
    for arg in args {
        match arg.as_str() {
            "--allow-unsafe" => allow_unsafe = true,
            flag if flag.starts_with('-') => {
                eprintln!("blu: unknown option `{flag}`");
                return usage_error();
            }
            path => roots.push(path),
        }
    }
    let [root] = roots[..] else {
        return usage_error();
    };
    let root = match std::path::absolute(root) {
        Ok(path) => vfs_path(&path),
        Err(err) => {
            eprintln!("blu: cannot use {root}: {err}");
            return ExitCode::from(2);
        }
    };

    let mut std_files = MemoryProvider::new();
    for (path, text) in bluing_std::files() {
        std_files.insert(VfsPath::new(format!("{}/{path}", bluing_std::DIR)), *text);
    }
    let provider = Layered(vec![&std_files, &Disk]);
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
        kind: CrateKind::Script { allow_unsafe },
        deps: vec![std],
    });

    let analysis = match bluing_hir::analyze(&graph, &provider) {
        Ok(analysis) => analysis,
        Err(missing) => {
            eprintln!("blu: cannot read {}", missing.0);
            return ExitCode::from(2);
        }
    };
    let cwd = std::env::current_dir().ok().map(|dir| vfs_path(&dir));
    for diagnostic in &analysis.diagnostics {
        let file = analysis.files.get(diagnostic.file);
        let pos = LineIndex::new(&file.text).line_col(diagnostic.range.start());
        let shown = cwd
            .as_ref()
            .and_then(|cwd| file.path.as_str().strip_prefix(&format!("{cwd}/")))
            .unwrap_or(file.path.as_str());
        eprintln!(
            "{shown}:{}:{}: error[{}]: {}",
            pos.line, pos.col, diagnostic.code, diagnostic.message
        );
    }
    if analysis.diagnostics.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// An OS path as a `/`-separated virtual path.
fn vfs_path(path: &Path) -> VfsPath {
    VfsPath::new(path.to_string_lossy().replace('\\', "/"))
}

/// Reads script files from disk.
struct Disk;

impl SourceProvider for Disk {
    fn read(&self, path: &VfsPath) -> Option<String> {
        fs::read_to_string(path.as_str()).ok()
    }
}
