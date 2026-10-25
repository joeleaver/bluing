//! Rejects the parts of Rust syntax that Bluing leaves out.

use ra_ap_syntax::{
    AstNode, NodeOrToken, SyntaxKind,
    ast::{self, HasName},
};

use crate::{Code, Diagnostic};

/// Per-package switches for the subset check.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Permit `unsafe`. Off by default: ordinary scripts never need it, and
    /// leaving it off keeps them deterministic and safe to hot-reload.
    pub allow_unsafe: bool,
}

const INLINE_ASM_MACROS: &[&str] = &["asm", "global_asm", "naked_asm"];

pub(crate) fn check(file: &ast::SourceFile, options: &Options, out: &mut Vec<Diagnostic>) {
    for element in file.syntax().descendants_with_tokens() {
        match element {
            NodeOrToken::Token(token) => {
                // Scanning tokens rather than nodes catches every form of
                // `unsafe`: blocks, fns, impls, traits, extern blocks and
                // `#[unsafe(...)]` attributes, including inside macro calls.
                if token.kind() == SyntaxKind::UNSAFE_KW && !options.allow_unsafe {
                    out.push(Diagnostic::new(
                        Code::Unsafe,
                        "`unsafe` is not enabled for this package",
                        token.text_range(),
                    ));
                }
            }
            NodeOrToken::Node(node) => {
                if let Some(abi) = ast::Abi::cast(node.clone()) {
                    out.push(Diagnostic::new(
                        Code::Ffi,
                        "foreign function interfaces (`extern`) are not supported in Bluing",
                        abi.syntax().text_range(),
                    ));
                } else if let Some(rules) = ast::MacroRules::cast(node.clone()) {
                    let range = rules.name().map_or(rules.syntax().text_range(), |name| {
                        name.syntax().text_range()
                    });
                    out.push(Diagnostic::new(
                        Code::MacroRules,
                        "`macro_rules!` is not supported yet",
                        range,
                    ));
                } else if let Some(call) = ast::MacroCall::cast(node) {
                    let Some(segment) = call.path().and_then(|path| path.segment()) else {
                        continue;
                    };
                    let Some(name) = segment.name_ref() else {
                        continue;
                    };
                    if INLINE_ASM_MACROS.contains(&name.text()) {
                        out.push(Diagnostic::new(
                            Code::InlineAsm,
                            "inline assembly is not supported in Bluing",
                            call.syntax().text_range(),
                        ));
                    }
                }
            }
        }
    }
}
