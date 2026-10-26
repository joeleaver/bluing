//! Operations that need an `unsafe` block or function, and references to
//! mutable statics, which the 2024 edition rejects outright.

use bluing_base::{Code, TextRange};
use bluing_hir::DefId;
use bluing_hir::body::{Expr, ExprId, PathExpr};
use bluing_hir::resolve::Res;

use crate::fn_ctxt::FnCtxt;

impl FnCtxt<'_, '_> {
    /// Reports `what` unless the code is inside an `unsafe` block or an
    /// `unsafe` function.
    pub fn require_unsafe(&mut self, range: TextRange, what: &str) {
        if !self.in_unsafe {
            let message = format!("{what} is unsafe and requires an `unsafe` block or function");
            self.error(range, Code::RequiresUnsafe, message);
        }
    }

    /// Whether `expr` names a mutable static.
    pub fn is_mutable_static(&self, expr: ExprId) -> bool {
        let Expr::Path(PathExpr::Item(path)) = &self.body.exprs[expr] else {
            return false;
        };
        match self.body.path_res(path) {
            Some(res) if res.unresolved_segments == 0 => match res.base {
                Res::Def(DefId::Static(id)) => self.cx.hir.statics[id].is_mut,
                _ => false,
            },
            _ => false,
        }
    }

    /// References to mutable statics are rejected even in `unsafe` code
    /// (rustc's `static_mut_refs`, denied in the 2024 edition).
    pub fn reject_static_mut_ref(&mut self, range: TextRange, expr: ExprId) {
        if self.is_mutable_static(expr) {
            let message = "creating a reference to a mutable static is not allowed: use `&raw const` or `&raw mut`";
            self.error(range, Code::DeniedLint, message);
        }
    }
}
