//! Parsing and subset checking for Bluing source.
//!
//! Bluing is a strict subset of Rust: every program this crate accepts must
//! also be valid Rust. Parsing is delegated to rust-analyzer's parser
//! (`ra_ap_syntax`); [`check`] then rejects the parts of Rust that Bluing
//! leaves out.

mod diagnostic;
mod line_index;
mod subset;

pub use diagnostic::{Code, Diagnostic};
pub use line_index::{LineCol, LineIndex};
pub use subset::Options;

use ra_ap_syntax::{Edition, SourceFile};

/// The Rust edition Bluing source is parsed as.
pub const EDITION: Edition = Edition::Edition2024;

/// Parses `text` and returns every syntax and subset diagnostic, in source order.
pub fn check(text: &str, options: &Options) -> Vec<Diagnostic> {
    let parse = SourceFile::parse(text, EDITION);
    let mut diagnostics: Vec<Diagnostic> = parse
        .errors()
        .into_iter()
        .map(|error| Diagnostic::new(Code::Syntax, error.to_string(), error.range()))
        .collect();
    subset::check(&parse.tree(), options, &mut diagnostics);
    diagnostics.sort_by_key(|d| (d.range.start(), d.range.end()));
    diagnostics
}
