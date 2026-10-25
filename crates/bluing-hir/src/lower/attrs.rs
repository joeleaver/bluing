//! Reads the attributes the HIR records. The syntax checker has already
//! validated every attribute, so this only extracts their contents.

use bluing_syntax::ra_ap_syntax::{
    AstNode, NodeOrToken, SyntaxKind, TextRange,
    ast::{self, HasAttrs},
};

use crate::Name;

/// The names inside every `#[derive(...)]` on `node`.
pub(crate) fn derives(node: &impl HasAttrs) -> Vec<Name> {
    let mut names = Vec::new();
    for attr in node.attrs() {
        let Some(ast::Meta::TokenTreeMeta(meta)) = attr.meta() else {
            continue;
        };
        let is_derive = meta
            .path()
            .and_then(|path| path.as_single_name_ref())
            .is_some_and(|name| name.text() == "derive");
        let Some(tree) = meta.token_tree().filter(|_| is_derive) else {
            continue;
        };
        for element in tree.syntax().children_with_tokens() {
            if let NodeOrToken::Token(token) = element
                && token.kind() == SyntaxKind::IDENT
            {
                names.push(Name::new(token.text()));
            }
        }
    }
    names
}

/// Every `#[lang = ...]` attribute on `node`: the item name, or the range of
/// an attribute whose value is not a plain string literal.
pub(crate) fn lang(node: &impl HasAttrs) -> Vec<Result<(Name, TextRange), TextRange>> {
    node.attrs()
        .filter_map(|attr| {
            let ast::Meta::KeyValueMeta(meta) = attr.meta()? else {
                return None;
            };
            let name = meta.path()?.as_single_name_ref()?;
            if name.text() != "lang" {
                return None;
            }
            let range = attr.syntax().text_range();
            let value = meta.expr().and_then(|expr| match expr {
                ast::Expr::Literal(literal) => match literal.kind() {
                    ast::LiteralKind::String(string) => string.value().ok().map(|v| v.into_owned()),
                    _ => None,
                },
                _ => None,
            });
            Some(value.map(|value| (Name::new(&value), range)).ok_or(range))
        })
        .collect()
}
