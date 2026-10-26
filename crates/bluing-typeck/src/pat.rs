//! Checking patterns against the type of the value they match, and that
//! bound values have a size known at compile time.

use bluing_base::{Code, TextRange};
use bluing_hir::body::{FieldPat, Pat, PatId};
use bluing_hir::hir::{Fields, Path};
use bluing_ty::ty::{InferTy, Ty, TyKind};

use crate::fn_ctxt::FnCtxt;

impl FnCtxt<'_, '_> {
    fn pat_range(&self, pat: PatId) -> TextRange {
        self.body.pat_ranges.get(pat).copied().unwrap_or_default()
    }

    pub fn check_pat(&mut self, pat: PatId, expected: Ty) {
        self.results.pat_tys.insert(pat, expected);
        let range = self.pat_range(pat);
        let body = self.body;
        match &body.pats[pat] {
            Pat::Missing | Pat::Wild => {}
            Pat::Bind { local } => {
                self.results.local_tys.insert(*local, expected);
                self.require_sized(range, expected);
            }
            Pat::Tuple(elements) => {
                let element_tys = match self.kind(expected) {
                    TyKind::Tuple(list) if self.cx.interner.tys(list).len() == elements.len() => {
                        self.cx.interner.tys(list).to_vec()
                    }
                    TyKind::Infer(InferTy::Ty(_)) => {
                        let vars: Vec<Ty> = elements.iter().map(|_| self.new_var()).collect();
                        let list = self.cx.interner.ty_list(vars.clone());
                        let tuple = self.intern(TyKind::Tuple(list));
                        self.unify(expected, tuple);
                        vars
                    }
                    kind => {
                        if kind != TyKind::Error {
                            let message = format!(
                                "mismatched types: expected `{}`, found a tuple with {} elements",
                                self.display(expected),
                                elements.len()
                            );
                            self.error(range, Code::TypeMismatch, message);
                        }
                        let error = self.cx.interner.error();
                        vec![error; elements.len()]
                    }
                };
                for (&element, ty) in elements.iter().zip(element_tys) {
                    self.check_pat(element, ty);
                }
            }
            Pat::Struct {
                path,
                fields,
                has_rest,
            } => self.check_struct_pat(range, expected, path, fields, *has_rest),
        }
    }

    fn check_struct_pat(
        &mut self,
        range: TextRange,
        expected: Ty,
        path: &Path,
        fields: &[FieldPat],
        has_rest: bool,
    ) {
        let error = self.cx.interner.error();
        let Some(named) = self.named_struct(path.range, path) else {
            for field in fields {
                self.check_pat(field.pat, error);
            }
            return;
        };
        if named.is_variant {
            // A variant pattern is refutable unless its enum has one variant;
            // refutable patterns are milestone 3b.
            self.error(
                range,
                Code::NotYetSupported,
                "enum variant patterns are not supported yet",
            );
            for field in fields {
                self.check_pat(field.pat, error);
            }
            return;
        }
        self.demand_eq(range, expected, named.ty);
        let field_count = match self.fields_of(named.owner) {
            Fields::Record(fields) | Fields::Tuple(fields) => fields.len(),
            Fields::Unit => 0,
        };
        let mut seen = vec![false; field_count];
        for field in fields {
            match self.field_index(named.owner, &field.name) {
                Some(index) => {
                    if seen[index] {
                        let message = format!(
                            "field `{}` is bound more than once in the pattern",
                            field.name
                        );
                        self.error(field.range, Code::Duplicate, message);
                    }
                    seen[index] = true;
                    self.check_field_visible(field.range, named.owner, index, &field.name);
                    let ty = self.field_ty(named.owner, index, named.substs);
                    self.check_pat(field.pat, ty);
                }
                None => {
                    let message = format!(
                        "`{}` does not have a field named `{}`",
                        named.name, field.name
                    );
                    self.error(field.range, Code::Unresolved, message);
                    self.check_pat(field.pat, error);
                }
            }
        }
        if !has_rest {
            let missing: Vec<String> = (0..field_count)
                .filter(|&i| !seen[i])
                .map(|i| format!("`{}`", self.field_name(named.owner, i)))
                .collect();
            if !missing.is_empty() {
                let message = format!(
                    "pattern does not mention fields {}; add them or `..`",
                    missing.join(", ")
                );
                self.error(range, Code::MissingFields, message);
            }
        }
    }

    /// Requires a bound or passed value to have a size known at compile
    /// time. Type parameters are sized unless bounded with `?Sized`, which
    /// milestone 3c checks.
    pub fn require_sized(&mut self, range: TextRange, ty: Ty) {
        if matches!(self.kind(ty), TyKind::Str | TyKind::Slice(_)) {
            let message = format!(
                "the size of `{}` cannot be known at compile time",
                self.display(ty)
            );
            self.error(range, Code::Unsized, message);
        }
    }
}
