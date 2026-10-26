//! Completing a body's types: literal fallback, types that cannot be
//! inferred, literals that do not fit their type, and writing back resolved
//! types.

use bluing_base::Code;
use bluing_hir::body::UnaryOp;
use bluing_ty::layout;
use bluing_ty::ty::{FloatTy, InferTy, Ty, TyKind};
use std::collections::BTreeSet;

use crate::fn_ctxt::FnCtxt;
use crate::infer::visit;

impl FnCtxt<'_, '_> {
    pub fn finish(&mut self) {
        // Integer and float literal variables nothing decided fall back to
        // `i32` and `f64`.
        let tys: Vec<Ty> = self
            .results
            .expr_tys
            .values()
            .chain(self.results.pat_tys.values())
            .chain(self.results.local_tys.values())
            .copied()
            .collect();
        for ty in tys {
            let resolved = self.resolve(ty);
            let mut literal_vars = Vec::new();
            visit(&self.cx.interner, resolved, &mut |kind| {
                if let TyKind::Infer(InferTy::Int(_) | InferTy::Float(_)) = kind {
                    literal_vars.push(kind.clone());
                }
            });
            for kind in literal_vars {
                let var = self.intern(kind);
                self.infer.fall_back(&mut self.cx.interner, var);
            }
        }

        // Any type variable left could not be inferred. Report it once, where
        // it first appears.
        let mut reported = BTreeSet::new();
        let exprs: Vec<_> = self.results.expr_tys.iter().map(|(e, &t)| (e, t)).collect();
        let mut by_position: Vec<_> = exprs
            .into_iter()
            .map(|(expr, ty)| (self.expr_range(expr), expr, ty))
            .collect();
        by_position.sort_by_key(|(range, ..)| (range.start(), range.end()));
        for (range, expr, ty) in by_position {
            let resolved = self.resolve(ty);
            let mut unknown = Vec::new();
            visit(&self.cx.interner, resolved, &mut |kind| {
                if let TyKind::Infer(InferTy::Ty(var)) = kind {
                    unknown.push(*var);
                }
            });
            if let Some(&var) = unknown.first()
                && reported.insert(var)
            {
                let message = format!(
                    "type annotations needed: cannot infer the type `{}`",
                    self.display(resolved)
                );
                self.error(range, Code::Inference, message);
            }
            let ty = self.final_ty(ty);
            self.results.expr_tys.insert(expr, ty);
        }
        // A binding's type may be unknown without any expression using it.
        let mut pats: Vec<_> = self
            .results
            .pat_tys
            .iter()
            .map(|(p, &t)| {
                (
                    self.body.pat_ranges.get(p).copied().unwrap_or_default(),
                    p,
                    t,
                )
            })
            .collect();
        pats.sort_by_key(|(range, ..)| (range.start(), range.end()));
        for (range, pat, ty) in pats {
            let resolved = self.resolve(ty);
            let mut unknown = Vec::new();
            visit(&self.cx.interner, resolved, &mut |kind| {
                if let TyKind::Infer(InferTy::Ty(var)) = kind {
                    unknown.push(*var);
                }
            });
            if let Some(&var) = unknown.first()
                && reported.insert(var)
            {
                let message = format!(
                    "type annotations needed: cannot infer the type `{}`",
                    self.display(resolved)
                );
                self.error(range, Code::Inference, message);
            }
            let ty = self.final_ty(ty);
            self.results.pat_tys.insert(pat, ty);
        }
        let locals: Vec<_> = self
            .results
            .local_tys
            .iter()
            .map(|(l, &t)| (l, t))
            .collect();
        for (local, ty) in locals {
            let ty = self.final_ty(ty);
            self.results.local_tys.insert(local, ty);
        }

        self.check_literals();
        for expr in std::mem::take(&mut self.deferred_negations) {
            let ty = self.results.expr_tys[expr];
            if matches!(self.cx.interner.kind(ty), TyKind::Uint(_)) {
                self.cannot_apply_unary(expr, UnaryOp::Neg, ty);
            }
        }
    }

    /// A fully resolved type; anything still unknown becomes an error type
    /// (and was reported).
    fn final_ty(&mut self, ty: Ty) -> Ty {
        let resolved = self.resolve(ty);
        let mut has_var = false;
        visit(&self.cx.interner, resolved, &mut |kind| {
            has_var |= matches!(kind, TyKind::Infer(_));
        });
        if has_var {
            self.cx.interner.error()
        } else {
            resolved
        }
    }

    /// Literals must fit their final type (rustc's deny-by-default
    /// `overflowing_literals`).
    fn check_literals(&mut self) {
        for (expr, value, negated) in std::mem::take(&mut self.int_literals) {
            let ty = self.results.expr_tys[expr];
            let kind = self.cx.interner.kind(ty).clone();
            if !layout::int_fits_signed(&kind, value, negated) {
                let range = self.expr_range(expr);
                let message = format!("literal out of range for `{}`", self.display(ty));
                self.error(range, Code::Overflow, message);
            }
        }
        for (expr, text) in std::mem::take(&mut self.float_literals) {
            let ty = self.results.expr_tys[expr];
            let fits = match (self.cx.interner.kind(ty), text.parse::<f64>()) {
                (TyKind::Float(FloatTy::F32), Ok(value)) => (value as f32).is_finite(),
                (TyKind::Float(FloatTy::F64), Ok(value)) => value.is_finite(),
                _ => true,
            };
            if !fits {
                let range = self.expr_range(expr);
                let message = format!("literal out of range for `{}`", self.display(ty));
                self.error(range, Code::Overflow, message);
            }
        }
    }
}
