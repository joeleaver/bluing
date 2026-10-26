use std::fmt;

use text_size::TextRange;

use crate::FileId;

/// What kind of problem a [`Diagnostic`] reports. Every error the compiler
/// can produce has a code here, so the full list lives in one place.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Code {
    /// The source is not valid Rust syntax.
    Syntax,
    /// `unsafe` used in a package that has not opted in.
    Unsafe,
    /// Foreign function interface: `extern` blocks, ABIs on functions or
    /// function pointer types. Bluing never supports these.
    Ffi,
    /// Inline assembly (`asm!` and friends). Bluing never supports these.
    InlineAsm,
    /// `macro_rules!` definitions, which Bluing does not support yet.
    MacroRules,
    /// An attribute outside Bluing's allowlist.
    Attribute,
    /// Valid Rust that the compiler does not handle yet.
    NotYetSupported,
    /// Rust that Bluing deliberately leaves out (`extern crate`, and unstable
    /// features such as `macro` items, negative impls and specialization).
    Unsupported,
    /// A `mod foo;` whose file is missing, or present in two places.
    ModuleFile,
    /// A name or path that does not resolve.
    Unresolved,
    /// A name that resolves to more than one definition.
    Ambiguous,
    /// A path that names something not visible from where it is written.
    Private,
    /// A name defined or imported twice in the same namespace of a module.
    Duplicate,
    /// A path that resolves, but to the wrong kind of item (for example a
    /// struct where a trait is required).
    WrongKind,
    /// A lifetime that is not declared.
    UndeclaredLifetime,
    /// An invalid visibility: `pub(in path)` naming a non-ancestor, a
    /// qualifier where none is allowed, or a re-export wider than its item.
    Visibility,
    /// Misplaced control flow: `break` or `continue` outside a loop, `break`
    /// with a value out of `while`, `return` outside a function, ...
    Control,
}

impl Code {
    pub const ALL: [Code; 17] = [
        Code::Syntax,
        Code::Unsafe,
        Code::Ffi,
        Code::InlineAsm,
        Code::MacroRules,
        Code::Attribute,
        Code::NotYetSupported,
        Code::Unsupported,
        Code::ModuleFile,
        Code::Unresolved,
        Code::Ambiguous,
        Code::Private,
        Code::Duplicate,
        Code::WrongKind,
        Code::UndeclaredLifetime,
        Code::Visibility,
        Code::Control,
    ];

    /// The code whose [`Code::as_str`] is `name`.
    pub fn from_name(name: &str) -> Option<Code> {
        Code::ALL.into_iter().find(|code| code.as_str() == name)
    }

    /// Short stable name, shown as `error[name]` and usable in tests.
    pub fn as_str(self) -> &'static str {
        match self {
            Code::Syntax => "syntax",
            Code::Unsafe => "unsafe",
            Code::Ffi => "ffi",
            Code::InlineAsm => "inline-asm",
            Code::MacroRules => "macro-rules",
            Code::Attribute => "attribute",
            Code::NotYetSupported => "not-yet-supported",
            Code::Unsupported => "unsupported",
            Code::ModuleFile => "module-file",
            Code::Unresolved => "unresolved",
            Code::Ambiguous => "ambiguous",
            Code::Private => "private",
            Code::Duplicate => "duplicate",
            Code::WrongKind => "wrong-kind",
            Code::UndeclaredLifetime => "undeclared-lifetime",
            Code::Visibility => "visibility",
            Code::Control => "control",
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
    pub file: FileId,
    /// Byte range in the file's text.
    pub range: TextRange,
    pub code: Code,
    pub message: String,
}

impl Diagnostic {
    pub fn new(file: FileId, range: TextRange, code: Code, message: impl Into<String>) -> Self {
        Diagnostic {
            file,
            range,
            code,
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Code;

    /// Position in [`Code::ALL`]. Exhaustive, so adding a code without
    /// updating `ALL` fails to compile here.
    fn ordinal(code: Code) -> usize {
        match code {
            Code::Syntax => 0,
            Code::Unsafe => 1,
            Code::Ffi => 2,
            Code::InlineAsm => 3,
            Code::MacroRules => 4,
            Code::Attribute => 5,
            Code::NotYetSupported => 6,
            Code::Unsupported => 7,
            Code::ModuleFile => 8,
            Code::Unresolved => 9,
            Code::Ambiguous => 10,
            Code::Private => 11,
            Code::Duplicate => 12,
            Code::WrongKind => 13,
            Code::UndeclaredLifetime => 14,
            Code::Visibility => 15,
            Code::Control => 16,
        }
    }

    #[test]
    fn all_lists_every_code_once_and_names_round_trip() {
        for (index, &code) in Code::ALL.iter().enumerate() {
            assert_eq!(ordinal(code), index);
            assert_eq!(Code::from_name(code.as_str()), Some(code));
        }
    }
}
