//! Method calls on inherent impls, with automatic dereferencing and
//! referencing of the receiver, probed in rustc's order.

use bluing_base::{Code, TextRange};
use bluing_hir::body::ExprId;
use bluing_hir::hir::{AssocItem, GenericArgs};
use bluing_hir::{FnId, GenericOwner, Name};
use bluing_ty::ty::{InferTy, Mutability, Ty, TyKind};

use crate::fn_ctxt::FnCtxt;

/// The method a call resolves to, and how the receiver is adjusted.
struct Pick {
    method: FnId,
    derefs: u32,
    autoref: Option<Mutability>,
}

impl FnCtxt<'_, '_> {
    pub fn check_method_call(
        &mut self,
        expr: ExprId,
        receiver: ExprId,
        name: &Name,
        name_range: TextRange,
        generic_args: Option<&GenericArgs>,
        args: &[ExprId],
    ) -> Ty {
        let receiver_ty = self.check_expr(receiver);
        if generic_args.is_some() {
            self.error(
                name_range,
                Code::NotYetSupported,
                "generic arguments on method calls are not supported yet",
            );
        }
        let Some(pick) = self.probe(expr, receiver, receiver_ty, name, name_range) else {
            for &arg in args {
                self.check_expr(arg);
            }
            return self.cx.interner.error();
        };
        self.results.method_calls.insert(expr, pick.method);
        if self.cx.items.fn_sigs[pick.method].is_unsafe {
            let range = self.expr_range(expr);
            self.require_unsafe(range, "a call to an `unsafe` method");
        }
        if pick.autoref.is_some() {
            let range = self.expr_range(receiver);
            self.reject_static_mut_ref(range, receiver);
        }
        let adjustment = self.results.adjustments.entry(receiver).or_default();
        adjustment.derefs = pick.derefs;
        adjustment.autoref = pick.autoref;
        let callable = bluing_ty::ty::CallableId::Fn(pick.method);
        let substs = self
            .lifetime_only_substs(GenericOwner::Fn(pick.method))
            .expect("probing only picks methods without type parameters");
        let (inputs, output) = self.callable_sig(callable, substs);
        self.check_args(expr, &inputs[1..], args);
        output
    }

    /// The receiver types to try: the receiver, then what it points to,
    /// repeatedly.
    fn autoderef_steps(&mut self, receiver: ExprId, ty: Ty) -> Option<Vec<Ty>> {
        let mut steps = vec![self.shallow(ty)];
        loop {
            let current = *steps.last().expect("there is a first step");
            match self.kind(current) {
                TyKind::Ref(_, pointee, _) => steps.push(self.shallow(pointee)),
                TyKind::Infer(InferTy::Ty(_)) if steps.len() == 1 => {
                    let range = self.expr_range(receiver);
                    self.error(
                        range,
                        Code::Inference,
                        "type annotations needed: the type of the receiver is not known here",
                    );
                    return None;
                }
                _ => return Some(steps),
            }
        }
    }

    fn probe(
        &mut self,
        expr: ExprId,
        receiver: ExprId,
        receiver_ty: Ty,
        name: &Name,
        name_range: TextRange,
    ) -> Option<Pick> {
        if matches!(self.kind(receiver_ty), TyKind::Error) {
            return None;
        }
        let steps = self.autoderef_steps(receiver, receiver_ty)?;
        // Candidates come from the inherent impls of every step's type; each
        // step is then matched against all of them, as in rustc.
        let mut candidates = Vec::new();
        for &step in &steps {
            for method in self.method_candidates(expr, step, name)? {
                if !candidates.contains(&method) {
                    candidates.push(method);
                }
            }
        }
        for (derefs, &step) in steps.iter().enumerate() {
            // By value first, then `&`, then `&mut`.
            for autoref in [None, Some(Mutability::Not), Some(Mutability::Mut)] {
                let wanted = match autoref {
                    None => step,
                    Some(mutability) => {
                        self.intern(TyKind::Ref(bluing_ty::ty::Region::Erased, step, mutability))
                    }
                };
                for &method in &candidates {
                    if self.method_receiver(method) == wanted {
                        return Some(Pick {
                            method,
                            derefs: derefs as u32,
                            autoref,
                        });
                    }
                }
            }
        }
        let innermost = *steps.last().expect("there is a first step");
        self.no_method(name_range, receiver_ty, innermost, name);
        None
    }

    /// The methods named `name` in the inherent impls of exactly `self_ty`.
    /// Returns `None` after reporting an error.
    fn method_candidates(&mut self, expr: ExprId, self_ty: Ty, name: &Name) -> Option<Vec<FnId>> {
        let hir = self.cx.hir;
        let Some(simplified) = self.cx.simplify(self_ty) else {
            return Some(Vec::new());
        };
        let impls = self
            .cx
            .inherent_impls
            .get(&simplified)
            .cloned()
            .unwrap_or_default();
        let mut candidates = Vec::new();
        for impl_id in impls {
            let has_method = hir.impls[impl_id].items.iter().any(|&item| match item {
                AssocItem::Fn(id) => {
                    hir.functions[id].name == *name && hir.functions[id].self_param.is_some()
                }
                _ => false,
            });
            if !has_method {
                continue;
            }
            let impl_self = self.cx.impl_self_ty(impl_id);
            let impl_self = self.erase_regions(impl_self);
            if self
                .lifetime_only_substs(GenericOwner::Impl(impl_id))
                .is_none()
            {
                let range = self.expr_range(expr);
                self.error(
                    range,
                    Code::NotYetSupported,
                    "methods of generic impls are not supported yet",
                );
                return None;
            }
            if impl_self != self_ty {
                continue;
            }
            for &item in &hir.impls[impl_id].items {
                if let AssocItem::Fn(id) = item
                    && hir.functions[id].name == *name
                    && hir.functions[id].self_param.is_some()
                {
                    if self.lifetime_only_substs(GenericOwner::Fn(id)).is_none() {
                        let range = self.expr_range(expr);
                        self.error(
                            range,
                            Code::NotYetSupported,
                            "calling generic methods is not supported yet",
                        );
                        return None;
                    }
                    let visible = self.cx.resolutions.def_visibilities[&bluing_hir::DefId::Fn(id)]
                        .is_accessible_from(hir, self.module);
                    if !visible {
                        let range = self.expr_range(expr);
                        self.error(range, Code::Private, format!("method `{name}` is private"));
                    }
                    candidates.push(id);
                }
            }
        }
        Some(candidates)
    }

    /// The type a method takes `self` as, seen from the body.
    fn method_receiver(&mut self, method: FnId) -> Ty {
        let sig = self.cx.items.fn_sigs[method];
        let receiver = self.cx.interner.tys(sig.inputs_and_output)[0];
        self.erase_regions(receiver)
    }

    fn no_method(&mut self, range: TextRange, receiver_ty: Ty, innermost: Ty, name: &Name) {
        if self.some_trait_has_item(name) {
            self.error(
                range,
                Code::NotYetSupported,
                "trait methods are not supported yet",
            );
            return;
        }
        if self
            .cx
            .simplify(innermost)
            .is_some_and(|s| !matches!(s, bluing_ty::SimplifiedType::Adt(_)))
        {
            let message = "methods of primitive types come with the standard library and are not supported yet";
            self.error(range, Code::NotYetSupported, message);
            return;
        }
        let message = format!(
            "no method named `{name}` found for `{}`",
            self.display(receiver_ty)
        );
        self.error(range, Code::Unresolved, message);
    }
}
