//! Shared helpers: build a compilation of a script crate plus the embedded
//! standard library from in-memory files, and parse fixture files.

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

/// A test case written as one file:
///
/// ```text
/// // bluing: ok                 (or a comma-separated list of codes)
/// // rustc: ok                  (or `error`)
/// //- main.blu                  (optional; starts a file, default main.blu)
/// ...source...
/// ```
pub struct Fixture {
    pub files: Vec<(String, String)>,
    /// The diagnostic codes Bluing must report, in order. Empty means none.
    pub bluing: Vec<Code>,
    pub rustc_ok: bool,
}

impl Fixture {
    pub fn parse(text: &str) -> Result<Fixture, String> {
        let mut lines = text.lines().peekable();
        let bluing = header(lines.next(), "bluing")?;
        let rustc = header(lines.next(), "rustc")?;
        let bluing = if bluing == "ok" {
            Vec::new()
        } else {
            bluing
                .split(',')
                .map(|name| Code::from_name(name.trim()).ok_or(format!("unknown code `{name}`")))
                .collect::<Result<_, _>>()?
        };
        let rustc_ok = match rustc.as_str() {
            "ok" => true,
            "error" => false,
            other => {
                return Err(format!(
                    "`// rustc:` must be `ok` or `error`, not `{other}`"
                ));
            }
        };
        let mut files: Vec<(String, String)> = Vec::new();
        for line in lines {
            if let Some(path) = line.strip_prefix("//- ") {
                files.push((path.trim().to_owned(), String::new()));
                continue;
            }
            if files.is_empty() {
                files.push(("main.blu".to_owned(), String::new()));
            }
            let text = &mut files.last_mut().expect("a file was just pushed").1;
            text.push_str(line);
            text.push('\n');
        }
        if files.is_empty() {
            return Err("a fixture needs some source".to_owned());
        }
        Ok(Fixture {
            files,
            bluing,
            rustc_ok,
        })
    }

    pub fn file_refs(&self) -> Vec<(&str, &str)> {
        self.files
            .iter()
            .map(|(path, text)| (path.as_str(), text.as_str()))
            .collect()
    }
}

fn header(line: Option<&str>, key: &str) -> Result<String, String> {
    line.and_then(|line| line.strip_prefix(&format!("// {key}:")))
        .map(|value| value.trim().to_owned())
        .ok_or(format!("expected a `// {key}: ...` header line"))
}
