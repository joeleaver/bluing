//! Shared foundations of the Bluing compiler: file ids, diagnostics and their
//! codes, and mapping byte offsets to lines and columns.

mod diagnostic;
mod line_index;

pub use diagnostic::{Code, Diagnostic};
pub use line_index::{LineCol, LineIndex};
pub use text_size::{TextRange, TextSize};

/// Identifies one source file within a compilation.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FileId(pub u32);
