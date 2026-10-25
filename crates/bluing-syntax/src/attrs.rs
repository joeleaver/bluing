//! The attribute allowlist. Every attribute is either known and accepted,
//! explicitly not supported yet, or rejected; none is silently ignored.

use bluing_base::Code;
use ra_ap_syntax::{AstNode, NodeOrToken, SyntaxKind, TextRange, ast};

use crate::{Options, Sink};

/// The derives the compiler implements itself.
pub const BUILTIN_DERIVES: &[&str] = &[
    "Debug",
    "Clone",
    "Copy",
    "PartialEq",
    "Eq",
    "PartialOrd",
    "Ord",
    "Hash",
    "Default",
];

/// Tool namespaces whose attributes are inert (`#[rustfmt::skip]`).
const INERT_TOOLS: &[&str] = &["rustfmt", "clippy"];

/// The forms an attribute can take.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// `#[name]`
    Word,
    /// `#[name(...)]`
    List,
    /// `#[name = value]`
    KeyValue,
}

pub(crate) fn check(attr: &ast::Attr, options: &Options, sink: &mut Sink<'_>) {
    // A missing meta is a syntax error, already reported by the parser.
    if let Some(meta) = attr.meta() {
        check_meta(&meta, attr.syntax().text_range(), options, sink);
    }
}

fn check_meta(meta: &ast::Meta, range: TextRange, options: &Options, sink: &mut Sink<'_>) {
    let (path, shape) = match meta {
        ast::Meta::CfgMeta(_) => {
            return sink.error(
                range,
                Code::NotYetSupported,
                "`#[cfg]` is not supported yet",
            );
        }
        ast::Meta::CfgAttrMeta(_) => {
            let message = "`#[cfg_attr]` is not supported yet";
            return sink.error(range, Code::NotYetSupported, message);
        }
        // The `unsafe` keyword itself is gated by the token scan; the
        // attribute inside still has to pass the allowlist.
        ast::Meta::UnsafeMeta(meta) => {
            if let Some(inner) = meta.meta() {
                check_meta(&inner, range, options, sink);
            }
            return;
        }
        ast::Meta::PathMeta(meta) => (meta.path(), Shape::Word),
        ast::Meta::TokenTreeMeta(meta) => (meta.path(), Shape::List),
        ast::Meta::KeyValueMeta(meta) => (meta.path(), Shape::KeyValue),
    };
    let Some(path) = path else { return };

    let Some(name) = path.as_single_name_ref() else {
        let tool = path.first_segment().and_then(|segment| segment.name_ref());
        if !tool.is_some_and(|tool| INERT_TOOLS.contains(&tool.text())) {
            let message = format!("attribute `{path}` is not supported in Bluing");
            sink.error(range, Code::Attribute, message);
        }
        return;
    };
    let name = name.text();
    let allowed: &[Shape] = match name {
        "allow" | "warn" | "deny" | "forbid" | "expect" => &[Shape::List],
        "doc" => &[Shape::KeyValue, Shape::List],
        "inline" => &[Shape::Word, Shape::List],
        "cold" => &[Shape::Word],
        "must_use" => &[Shape::Word, Shape::KeyValue],
        "deprecated" => &[Shape::Word, Shape::List, Shape::KeyValue],
        "derive" => &[Shape::List],
        "repr" => &[Shape::List],
        "lang" if options.library => &[Shape::KeyValue],
        "path" => {
            return sink.error(
                range,
                Code::NotYetSupported,
                "`#[path]` is not supported yet",
            );
        }
        _ => {
            let message = format!("attribute `{name}` is not supported in Bluing");
            return sink.error(range, Code::Attribute, message);
        }
    };
    if !allowed.contains(&shape) {
        return sink.error(
            range,
            Code::Attribute,
            format!("malformed `{name}` attribute"),
        );
    }
    if let ast::Meta::TokenTreeMeta(meta) = meta {
        match name {
            "derive" => check_derive(meta, range, sink),
            "repr" => check_repr(meta, range, sink),
            _ => {}
        }
    }
}

/// The plain tokens inside `#[name(...)]`, without delimiters and whitespace.
/// Returns `None` if the list contains a nested group.
fn list_tokens(meta: &ast::TokenTreeMeta) -> Option<Vec<(SyntaxKind, String)>> {
    let tree = meta.token_tree()?;
    let mut tokens = Vec::new();
    for element in tree.syntax().children_with_tokens() {
        match element {
            NodeOrToken::Node(_) => return None,
            NodeOrToken::Token(token) => match token.kind() {
                SyntaxKind::L_PAREN | SyntaxKind::R_PAREN | SyntaxKind::WHITESPACE => {}
                kind => tokens.push((kind, token.text().to_owned())),
            },
        }
    }
    Some(tokens)
}

fn check_derive(meta: &ast::TokenTreeMeta, range: TextRange, sink: &mut Sink<'_>) {
    let tokens = list_tokens(meta).filter(|tokens| {
        tokens
            .iter()
            .all(|(kind, _)| matches!(kind, SyntaxKind::IDENT | SyntaxKind::COMMA))
    });
    let Some(tokens) = tokens else {
        let message = "only plain derive names are supported yet";
        return sink.error(range, Code::NotYetSupported, message);
    };
    for (kind, text) in &tokens {
        if *kind == SyntaxKind::IDENT && !BUILTIN_DERIVES.contains(&text.as_str()) {
            let message =
                format!("`derive({text})` is not supported: Bluing only has the built-in derives");
            sink.error(range, Code::Attribute, message);
        }
    }
}

fn check_repr(meta: &ast::TokenTreeMeta, range: TextRange, sink: &mut Sink<'_>) {
    let supported = match list_tokens(meta).as_deref() {
        Some([(SyntaxKind::IDENT, text)]) => text == "C" || text == "transparent",
        _ => false,
    };
    if !supported {
        let message = "only `repr(C)` and `repr(transparent)` are supported in Bluing";
        sink.error(range, Code::Attribute, message);
    }
}
