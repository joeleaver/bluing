//! Parsing and subset checking for Bluing source.
//!
//! Bluing is a strict subset of Rust: every program this crate accepts must
//! also be valid Rust. Parsing is delegated to rust-analyzer's parser
//! (`ra_ap_syntax`); [`parse`] then rejects the parts of Rust that Bluing
//! leaves out.

mod attrs;
mod subset;

pub use ra_ap_syntax;
pub use subset::{Options, is_inline_asm_call};

use bluing_base::{Code, Diagnostic, FileId};
use ra_ap_syntax::{Edition, SourceFile};

/// The Rust edition Bluing source is parsed as.
pub const EDITION: Edition = Edition::Edition2024;

/// A parsed file together with its syntax and subset diagnostics.
pub struct ParsedFile {
    pub tree: SourceFile,
    /// Diagnostics in source order.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses `text` and checks it against the subset rules.
pub fn parse(file: FileId, text: &str, options: &Options) -> ParsedFile {
    let parse = SourceFile::parse(text, EDITION);
    let mut diagnostics: Vec<Diagnostic> = parse
        .errors()
        .into_iter()
        .map(|error| Diagnostic::new(file, error.range(), Code::Syntax, error.to_string()))
        .collect();
    let tree = parse.tree();
    subset::check(&tree, options, &mut Sink::new(file, &mut diagnostics));
    diagnostics.sort_by_key(|d| (d.range.start(), d.range.end()));
    ParsedFile { tree, diagnostics }
}

/// Collects diagnostics for one file.
pub(crate) struct Sink<'a> {
    file: FileId,
    out: &'a mut Vec<Diagnostic>,
}

impl<'a> Sink<'a> {
    fn new(file: FileId, out: &'a mut Vec<Diagnostic>) -> Self {
        Sink { file, out }
    }

    fn error(&mut self, range: ra_ap_syntax::TextRange, code: Code, message: impl Into<String>) {
        self.out
            .push(Diagnostic::new(self.file, range, code, message));
    }
}
