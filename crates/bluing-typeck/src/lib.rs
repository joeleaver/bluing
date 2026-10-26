//! Type inference and checking of bodies.
//!
//! [`typeck`] checks every function body and constant initializer against
//! the signatures `bluing-ty` collected, and records the type of every
//! expression, pattern and local. What milestone 3a covers is listed in
//! DESIGN.md; anything beyond it is reported as not supported yet.

mod adt;
mod call;
mod coerce;
mod control;
mod expr;
mod finish;
mod fn_ctxt;
mod infer;
mod method;
mod ops;
mod pat;
mod results;
mod unsafety;

pub use results::{Adjustment, AssocValue, TypeckResults};

use std::collections::BTreeMap;

use bluing_hir::body::{Body, BodyOwner};
use bluing_ty::TyCx;
use bluing_ty::ty::TyKind;

use crate::fn_ctxt::FnCtxt;

/// Checks every body, reporting errors to `cx.diagnostics`.
pub fn typeck(
    cx: &mut TyCx<'_>,
    bodies: &BTreeMap<BodyOwner, Body>,
) -> BTreeMap<BodyOwner, TypeckResults> {
    bodies
        .iter()
        .map(|(&owner, body)| (owner, check_body(cx, body)))
        .collect()
}

fn check_body(cx: &mut TyCx<'_>, body: &Body) -> TypeckResults {
    let mut fcx = FnCtxt::new(cx, body);
    match body.owner {
        BodyOwner::Fn(id) => {
            let sig = fcx.cx.items.fn_sigs[id];
            fcx.in_unsafe = sig.is_unsafe;
            let all: Vec<_> = fcx.cx.interner.tys(sig.inputs_and_output).to_vec();
            let all: Vec<_> = all.into_iter().map(|ty| fcx.erase_regions(ty)).collect();
            let (output, mut inputs) = all
                .split_last()
                .map(|(o, i)| (*o, i.iter().copied()))
                .expect("a signature has a return type");
            if let Some(local) = body.self_local {
                let self_ty = inputs.next().unwrap_or_else(|| fcx.cx.interner.error());
                fcx.results.local_tys.insert(local, self_ty);
            }
            for &pat in &body.params {
                let ty = inputs.next().unwrap_or_else(|| fcx.cx.interner.error());
                // A binding checks the size itself; other patterns still
                // need a sized parameter.
                if !matches!(body.pats[pat], bluing_hir::body::Pat::Bind { .. }) {
                    let range = body.pat_ranges.get(pat).copied().unwrap_or_default();
                    fcx.require_sized(range, ty);
                }
                fcx.check_pat(pat, ty);
            }
            if !matches!(fcx.cx.interner.kind(output), TyKind::Never) {
                let range = fcx.cx.hir.functions[id].loc.range;
                fcx.require_sized(range, output);
            }
            fcx.ret_ty = Some(output);
            fcx.check_expr_coercible(body.root, output);
        }
        BodyOwner::Const(id) => {
            let ty = fcx.cx.items.const_tys[id];
            let ty = fcx.erase_regions(ty);
            fcx.check_expr_coercible(body.root, ty);
        }
        BodyOwner::Static(id) => {
            let ty = fcx.cx.items.static_tys[id];
            let ty = fcx.erase_regions(ty);
            fcx.check_expr_coercible(body.root, ty);
        }
    }
    fcx.finish();
    fcx.results
}
