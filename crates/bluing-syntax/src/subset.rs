//! Rejects the parts of Rust syntax that Bluing leaves out.

use bluing_base::Code;
use ra_ap_syntax::{
    AstNode, NodeOrToken, SyntaxKind, SyntaxNode,
    ast::{self, HasName},
};

use crate::{Sink, attrs};

/// Per-crate switches for the subset check.
#[derive(Clone, Debug, Default)]
pub struct Options {
    /// Permit `unsafe`. Off by default: ordinary scripts never need it, and
    /// leaving it off keeps them deterministic and safe to hot-reload.
    pub allow_unsafe: bool,
    /// Library mode, used for Bluing's standard library: permits
    /// `#[lang = "..."]` attributes. Library crates also allow `unsafe`.
    pub library: bool,
}

const INLINE_ASM_MACROS: &[&str] = &["asm", "global_asm", "naked_asm"];

/// Whether `call` invokes one of the inline assembly macros.
pub fn is_inline_asm_call(call: &ast::MacroCall) -> bool {
    call.path()
        .and_then(|path| path.segment())
        .and_then(|segment| segment.name_ref())
        .is_some_and(|name| INLINE_ASM_MACROS.contains(&name.text()))
}

pub(crate) fn check(file: &ast::SourceFile, options: &Options, sink: &mut Sink<'_>) {
    let allow_unsafe = options.allow_unsafe || options.library;
    for element in file.syntax().descendants_with_tokens() {
        match element {
            // Scanning tokens rather than nodes catches every form of
            // `unsafe`: blocks, fns, impls, traits, extern blocks and
            // `#[unsafe(...)]` attributes, including inside macro calls.
            NodeOrToken::Token(token) => {
                if token.kind() == SyntaxKind::UNSAFE_KW && !allow_unsafe {
                    sink.error(
                        token.text_range(),
                        Code::Unsafe,
                        "`unsafe` is not enabled for this package",
                    );
                }
            }
            NodeOrToken::Node(node) => check_node(node, options, sink),
        }
    }
}

fn check_node(node: SyntaxNode, options: &Options, sink: &mut Sink<'_>) {
    if let Some(abi) = ast::Abi::cast(node.clone()) {
        sink.error(
            abi.syntax().text_range(),
            Code::Ffi,
            "foreign function interfaces (`extern`) are not supported in Bluing",
        );
    } else if let Some(rules) = ast::MacroRules::cast(node.clone()) {
        let range = rules.name().map_or(rules.syntax().text_range(), |name| {
            name.syntax().text_range()
        });
        sink.error(
            range,
            Code::MacroRules,
            "`macro_rules!` is not supported yet",
        );
    } else if let Some(call) = ast::MacroCall::cast(node.clone()) {
        if is_inline_asm_call(&call) {
            sink.error(
                call.syntax().text_range(),
                Code::InlineAsm,
                "inline assembly is not supported in Bluing",
            );
        }
    } else if let Some(attr) = ast::Attr::cast(node) {
        attrs::check(&attr, options, sink);
    }
}
