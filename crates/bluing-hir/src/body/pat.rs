//! Lowering of patterns. Milestone 3a handles the irrefutable binding
//! patterns: names, `_`, tuples and struct patterns; the rest is 3b.

use bluing_base::{Code, TextRange};
use bluing_syntax::ra_ap_syntax::{
    AstNode,
    ast::{self, HasName},
};

use super::lower::Lowerer;
use super::{FieldPat, Local, LocalId, Pat, PatId};
use crate::hir::Fields;
use crate::resolve::{Lookup, Namespace, Res};
use crate::{DefId, Name};

impl Lowerer<'_, '_> {
    /// Lowers a pattern. New bindings are appended to `bound`; the caller
    /// brings them into scope where the language says they start.
    pub fn pat(&mut self, pat: Option<&ast::Pat>, bound: &mut Vec<(Name, LocalId)>) -> PatId {
        let Some(pat) = pat else {
            // A syntax error the parser already reported.
            return self.alloc_pat(Pat::Missing, TextRange::default());
        };
        let range = pat.syntax().text_range();
        let lowered = self.pat_kind(pat, bound);
        self.alloc_pat(lowered, range)
    }

    fn alloc_pat(&mut self, pat: Pat, range: TextRange) -> PatId {
        let id = self.pats.alloc(pat);
        self.pat_ranges.insert(id, range);
        id
    }

    fn pat_not_yet(&mut self, range: TextRange, what: &str) -> Pat {
        self.error(
            range,
            Code::NotYetSupported,
            format!("{what} are not supported yet"),
        );
        Pat::Missing
    }

    fn pat_kind(&mut self, pat: &ast::Pat, bound: &mut Vec<(Name, LocalId)>) -> Pat {
        let range = pat.syntax().text_range();
        match pat {
            ast::Pat::WildcardPat(_) => Pat::Wild,
            ast::Pat::ParenPat(p) => match p.pat() {
                Some(inner) => self.pat_kind(&inner, bound),
                None => Pat::Missing,
            },
            ast::Pat::IdentPat(p) => self.ident_pat(p, bound),
            ast::Pat::TuplePat(p) => {
                if p.fields()
                    .any(|field| matches!(field, ast::Pat::RestPat(_)))
                {
                    return self.pat_not_yet(range, "`..` in tuple patterns");
                }
                let fields = p
                    .fields()
                    .map(|field| self.pat(Some(&field), bound))
                    .collect();
                Pat::Tuple(fields)
            }
            ast::Pat::RecordPat(p) => self.record_pat(p, bound),
            ast::Pat::LiteralPat(_) => self.pat_not_yet(range, "literal patterns"),
            ast::Pat::RangePat(_) => self.pat_not_yet(range, "range patterns"),
            ast::Pat::RefPat(_) => self.pat_not_yet(range, "reference patterns"),
            ast::Pat::SlicePat(_) => self.pat_not_yet(range, "slice patterns"),
            ast::Pat::TupleStructPat(_) => self.pat_not_yet(range, "tuple struct patterns"),
            ast::Pat::PathPat(_) => self.pat_not_yet(range, "path patterns"),
            ast::Pat::OrPat(_) => self.pat_not_yet(range, "or-patterns"),
            ast::Pat::RestPat(_) => self.pat_not_yet(range, "`..` patterns"),
            ast::Pat::MacroPat(_) => self.pat_not_yet(range, "macros in patterns"),
            ast::Pat::ConstBlockPat(_) => self.pat_not_yet(range, "`const` block patterns"),
            ast::Pat::BoxPat(_)
            | ast::Pat::DerefPat(_)
            | ast::Pat::NotNull(_)
            | ast::Pat::CfgPredPat(_) => {
                let message = "`box`, `deref` and `builtin #` patterns are not supported in Bluing";
                self.error(range, Code::Unsupported, message);
                Pat::Missing
            }
        }
    }

    fn ident_pat(&mut self, p: &ast::IdentPat, bound: &mut Vec<(Name, LocalId)>) -> Pat {
        let range = p.syntax().text_range();
        let Some(name_node) = p.name() else {
            return Pat::Missing;
        };
        if p.ref_token().is_some() {
            return self.pat_not_yet(range, "`ref` bindings");
        }
        if p.at_token().is_some() || p.pat().is_some() {
            return self.pat_not_yet(range, "`name @ pattern` bindings");
        }
        let name = Name::new(name_node.text());
        let mutable = p.mut_token().is_some();
        // An identifier is a new binding unless it names a unit struct, unit
        // variant or constant in scope; those make it a constant pattern. It
        // may not shadow a tuple struct, tuple variant or static, and a `mut`
        // binding (which cannot be a constant pattern) may not shadow any of
        // them.
        match self.pattern_name(&name) {
            PatternName::Binding => {}
            PatternName::Constant(_) if !mutable => {
                return self.pat_not_yet(range, "constant patterns");
            }
            PatternName::Constant(what) | PatternName::CannotShadow(what) => {
                let message = format!("a binding cannot shadow {what} `{name}`");
                self.error(range, Code::WrongKind, message);
                return Pat::Missing;
            }
        }
        if bound.iter().any(|(n, _)| n == &name) {
            let message =
                format!("identifier `{name}` is bound more than once in the same pattern");
            self.error(range, Code::Duplicate, message);
            return Pat::Missing;
        }
        let local = self.locals.alloc(Local {
            name: name.clone(),
            mutable,
            range: name_node.syntax().text_range(),
        });
        bound.push((name, local));
        Pat::Bind { local }
    }

    /// What a lone name in a pattern refers to.
    fn pattern_name(&self, name: &Name) -> PatternName {
        let is_const_param = self.scope.generics.iter().any(|(_, generics)| {
            generics.params.iter().any(|param| {
                &param.name == name
                    && matches!(param.kind, crate::hir::GenericParamKind::Const { .. })
            })
        });
        if is_const_param {
            return PatternName::Constant("a const parameter");
        }
        let module = self.scope.module;
        match self
            .env
            .lookup_lexical(module, name, Namespace::Values, true, false)
        {
            Lookup::Found(Res::Def(def)) => match def {
                DefId::Const(_) => PatternName::Constant("a constant"),
                DefId::Static(_) => PatternName::CannotShadow("a static"),
                DefId::Struct(id) => match self.env.hir.structs[id].fields {
                    Fields::Unit => PatternName::Constant("a unit struct"),
                    _ => PatternName::CannotShadow("a tuple struct"),
                },
                DefId::Variant(id) => match self.env.hir.variants[id].fields {
                    Fields::Unit => PatternName::Constant("a unit variant"),
                    _ => PatternName::CannotShadow("a tuple variant"),
                },
                _ => PatternName::Binding,
            },
            _ => PatternName::Binding,
        }
    }

    fn record_pat(&mut self, p: &ast::RecordPat, bound: &mut Vec<(Name, LocalId)>) -> Pat {
        let Some(path) = p.path().and_then(|path| self.types().path(&path)) else {
            return Pat::Missing;
        };
        let list = p.record_pat_field_list();
        let mut fields = Vec::new();
        for field in list.iter().flat_map(|list| list.fields()) {
            let field_range = field.syntax().text_range();
            let name = match (field.name_ref(), field.pat()) {
                (Some(name), _) => Name::new(name.text()),
                // Shorthand `S { x }`: the field name is the binding's name.
                (None, Some(ast::Pat::IdentPat(ident))) => match ident.name() {
                    Some(name) => Name::new(name.text()),
                    None => continue,
                },
                _ => continue,
            };
            let pat = self.pat(field.pat().as_ref(), bound);
            fields.push(FieldPat {
                name,
                range: field_range,
                pat,
            });
        }
        let has_rest = list.is_some_and(|list| list.rest_pat().is_some());
        Pat::Struct {
            path,
            fields,
            has_rest,
        }
    }
}

/// What a lone name in a pattern refers to; the string describes it.
enum PatternName {
    Binding,
    /// Something a plain name pattern would match as a constant.
    Constant(&'static str),
    /// Something no binding may shadow.
    CannotShadow(&'static str),
}
