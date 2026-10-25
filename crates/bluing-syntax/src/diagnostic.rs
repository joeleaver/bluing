use std::fmt;

use ra_ap_syntax::TextRange;

/// What kind of problem a [`Diagnostic`] reports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    /// The source is not valid Rust syntax.
    Syntax,
    /// `unsafe` used without opting in (see [`crate::Options::allow_unsafe`]).
    Unsafe,
    /// Foreign function interface: `extern` blocks, ABIs on functions or
    /// function pointer types. Bluing never supports these.
    Ffi,
    /// Inline assembly (`asm!` and friends). Bluing never supports these.
    InlineAsm,
    /// `macro_rules!` definitions, which Bluing does not support yet.
    MacroRules,
}

impl Code {
    /// Short stable name, shown as `error[name]` and usable in tests.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::Syntax => "syntax",
            Code::Unsafe => "unsafe",
            Code::Ffi => "ffi",
            Code::InlineAsm => "inline-asm",
            Code::MacroRules => "macro-rules",
        }
    }
}

impl fmt::Display for Code {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// An error found in Bluing source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: Code,
    pub message: String,
    /// Byte range in the source text.
    pub range: TextRange,
}

impl Diagnostic {
    pub fn new(code: Code, message: impl Into<String>, range: TextRange) -> Self {
        Diagnostic {
            code,
            message: message.into(),
            range,
        }
    }
}
