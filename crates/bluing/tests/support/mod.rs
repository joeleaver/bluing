//! Shared helpers: check a script from in-memory files, and parse fixtures.

#![allow(dead_code)] // Each test binary uses a different subset.

use bluing::{Checked, MemoryProvider, Options, VfsPath, check};
use bluing_base::{Code, LineIndex};

/// The directory script files live under in tests.
pub const ROOT_DIR: &str = "/script";

/// Checks a script crate whose files are `(path relative to the crate
/// directory, text)`; the first file is the crate root.
pub fn check_files(files: &[(&str, &str)]) -> Checked {
    let mut provider = MemoryProvider::new();
    for (path, text) in files {
        provider.insert(VfsPath::new(format!("{ROOT_DIR}/{path}")), *text);
    }
    let root = VfsPath::new(format!("{ROOT_DIR}/{}", files[0].0));
    check(root, &provider, &Options::default()).expect("the crate root exists")
}

/// The codes of every diagnostic, in order.
pub fn codes(checked: &Checked) -> Vec<Code> {
    checked.diagnostics.iter().map(|d| d.code).collect()
}

/// Every diagnostic rendered as `path:line:col: code: message`, for failure
/// messages.
pub fn render(checked: &Checked) -> String {
    checked
        .diagnostics
        .iter()
        .map(|d| {
            let file = checked.files.get(d.file);
            let pos = LineIndex::new(&file.text).line_col(d.range.start());
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
        let mut lines = text.lines();
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
