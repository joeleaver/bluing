//! Combining several values into one type: the branches of an `if`, the
//! `break` values of a loop or labeled block, the elements of an array.

use bluing_base::TextRange;
use bluing_hir::body::ExprId;
use bluing_ty::ty::{Ty, TyKind};

use crate::fn_ctxt::FnCtxt;

/// Collects values that must all coerce to one type. The type is the
/// expected one if the context gives one, and otherwise the first value's.
/// Values of type `!` coerce to anything and do not decide the type.
pub struct CoerceMany {
    target: Option<Ty>,
    /// Whether a value of a type other than `!` was pushed.
    has_value: bool,
}

impl CoerceMany {
    pub fn new(expected: Option<Ty>) -> Self {
        CoerceMany {
            target: expected,
            has_value: false,
        }
    }

    /// The type values must coerce to, if it is decided yet.
    pub fn target(&self) -> Option<Ty> {
        self.target
    }

    /// Adds the value of `expr`, or an implicit value (such as the `()` of a
    /// `break` without a value) located at `range`.
    pub fn push(
        &mut self,
        fcx: &mut FnCtxt<'_, '_>,
        expr: Option<ExprId>,
        ty: Ty,
        range: TextRange,
    ) {
        if matches!(fcx.kind(ty), TyKind::Never) {
            if let Some(expr) = expr {
                fcx.coerce(expr, ty, ty);
            }
            return;
        }
        self.has_value = true;
        match self.target {
            Some(target) => match expr {
                Some(expr) => {
                    fcx.coerce(expr, ty, target);
                }
                None => {
                    fcx.demand_eq(range, target, ty);
                }
            },
            None => self.target = Some(ty),
        }
    }

    /// The combined type: `!` if every value was `!` (or there were none).
    pub fn complete(self, fcx: &mut FnCtxt<'_, '_>) -> Ty {
        match self.target {
            Some(target) if self.has_value => target,
            _ => fcx.intern(TyKind::Never),
        }
    }
}
