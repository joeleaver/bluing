//! Blocks, statements and control flow, including whether control can reach
//! the end of an expression (divergence).

use bluing_hir::body::{Block, BreakTarget, ExprId, Stmt};
use bluing_ty::lower::{LowerCtx, PathSource, Placeholder, RegionMode};
use bluing_ty::ty::{Ty, TyKind};

use crate::coerce::CoerceMany;
use crate::fn_ctxt::{Breakable, Expectation, FnCtxt};

impl FnCtxt<'_, '_> {
    pub fn check_block(&mut self, block: &Block, expected: Expectation) -> Ty {
        let outer_unsafe = self.in_unsafe;
        self.in_unsafe |= block.is_unsafe;
        let ty = match block.target {
            Some(target) => self.check_labeled_block(block, target, expected),
            None => self.check_block_contents(block, expected),
        };
        self.in_unsafe = outer_unsafe;
        ty
    }

    fn check_block_contents(&mut self, block: &Block, expected: Expectation) -> Ty {
        let outer = std::mem::replace(&mut self.diverges, false);
        for stmt in &block.stmts {
            self.check_stmt(stmt);
        }
        let ty = match block.tail {
            Some(tail) => self.check_expr_with(tail, expected),
            // A block that always diverges before its end has type `!`.
            None if self.diverges => self.intern(TyKind::Never),
            None => self.cx.interner.unit(),
        };
        self.diverges |= outer;
        ty
    }

    fn check_labeled_block(
        &mut self,
        block: &Block,
        target: BreakTarget,
        expected: Expectation,
    ) -> Ty {
        let outer = self.diverges;
        self.breakables.push(Breakable {
            target,
            coerce: Some(CoerceMany::new(expected.ty())),
            may_break: false,
        });
        let ty = self.check_block_contents(block, expected);
        let inner_diverges = self.diverges;
        let breakable = self.breakables.pop().expect("pushed above");
        let mut coerce = breakable.coerce.expect("labeled blocks collect values");
        let range = match block.tail {
            Some(tail) => self.expr_range(tail),
            None => self.expr_range(self.body.break_targets[target.0 as usize]),
        };
        coerce.push(self, block.tail, ty, range);
        self.diverges = outer || (inner_diverges && !breakable.may_break);
        coerce.complete(self)
    }

    fn check_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Expr { expr, has_semi } => {
                if *has_semi {
                    self.check_expr(*expr);
                } else {
                    // A block-like expression used as a statement must be `()`.
                    let unit = self.cx.interner.unit();
                    let ty = self.check_expr_with(*expr, Expectation::Has(unit));
                    self.coerce(*expr, ty, unit);
                }
            }
            Stmt::Let { pat, ty, init } => {
                let declared = ty.as_ref().map(|ty| self.lower_body_ty(ty));
                let init_ty = init.map(|init| match declared {
                    Some(declared) => {
                        let ty = self.check_expr_with(init, Expectation::Has(declared));
                        self.coerce(init, ty, declared);
                        declared
                    }
                    None => self.check_expr(init),
                });
                let ty = match declared.or(init_ty) {
                    Some(ty) => ty,
                    None => self.new_var(),
                };
                self.check_pat(*pat, ty);
            }
        }
    }

    /// Lowers a type written in the body, where `_` is inferred.
    pub fn lower_body_ty(&mut self, ty: &bluing_hir::hir::TypeRef) -> Ty {
        let infer = &mut self.infer;
        let mut fresh = |interner: &mut bluing_ty::ty::Interner| infer.new_ty_var(interner);
        let mut cx = LowerCtx::new(
            self.body.file,
            PathSource::Body(self.body),
            Some(&self.generics),
            RegionMode::Erased,
            Placeholder::Infer(&mut fresh),
        );
        self.cx.lower_ty(&mut cx, ty)
    }

    pub fn check_if(
        &mut self,
        condition: ExprId,
        then_branch: ExprId,
        else_branch: Option<ExprId>,
        expected: Expectation,
    ) -> Ty {
        let bool_ty = self.cx.interner.bool();
        let condition_ty = self.check_expr_with(condition, Expectation::Has(bool_ty));
        self.coerce(condition, condition_ty, bool_ty);
        let condition_diverges = std::mem::replace(&mut self.diverges, false);
        match else_branch {
            None => {
                // Without `else`, the `then` branch must be `()`.
                let unit = self.cx.interner.unit();
                let then_ty = self.check_expr_with(then_branch, Expectation::Has(unit));
                self.coerce(then_branch, then_ty, unit);
                self.diverges = condition_diverges;
                unit
            }
            Some(else_branch) => {
                let mut coerce = CoerceMany::new(expected.ty());
                let then_ty = self.check_expr_with(then_branch, expected);
                let then_diverges = std::mem::replace(&mut self.diverges, false);
                let range = self.expr_range(then_branch);
                coerce.push(self, Some(then_branch), then_ty, range);
                let else_ty = self.check_expr_with(else_branch, expected);
                let else_diverges = self.diverges;
                let range = self.expr_range(else_branch);
                coerce.push(self, Some(else_branch), else_ty, range);
                self.diverges = condition_diverges || (then_diverges && else_diverges);
                coerce.complete(self)
            }
        }
    }

    pub fn check_loop(&mut self, body: ExprId, target: BreakTarget, expected: Expectation) -> Ty {
        let outer = self.diverges;
        self.breakables.push(Breakable {
            target,
            coerce: Some(CoerceMany::new(expected.ty())),
            may_break: false,
        });
        self.check_unit_body(body);
        let breakable = self.breakables.pop().expect("pushed above");
        // Without a `break`, control never leaves the loop.
        self.diverges = outer || !breakable.may_break;
        if breakable.may_break {
            breakable
                .coerce
                .expect("loops collect values")
                .complete(self)
        } else {
            self.intern(TyKind::Never)
        }
    }

    pub fn check_while(&mut self, condition: ExprId, body: ExprId, target: BreakTarget) -> Ty {
        let outer = self.diverges;
        let bool_ty = self.cx.interner.bool();
        let condition_ty = self.check_expr_with(condition, Expectation::Has(bool_ty));
        self.coerce(condition, condition_ty, bool_ty);
        self.breakables.push(Breakable {
            target,
            coerce: None,
            may_break: false,
        });
        self.check_unit_body(body);
        self.breakables.pop();
        // The condition may be false at once, so the body may never run.
        self.diverges = outer;
        self.cx.interner.unit()
    }

    fn check_unit_body(&mut self, body: ExprId) {
        let unit = self.cx.interner.unit();
        let ty = self.check_expr_with(body, Expectation::Has(unit));
        self.coerce(body, ty, unit);
    }

    pub fn check_break(&mut self, expr: ExprId, target: BreakTarget, value: Option<ExprId>) -> Ty {
        let index = self
            .breakables
            .iter()
            .rposition(|b| b.target == target)
            .expect("lowering resolved the break target to an enclosing loop or block");
        let coerce = self.breakables[index].coerce.take();
        if let Some(mut coerce) = coerce {
            let expected = match coerce.target() {
                Some(ty) => Expectation::Has(ty),
                None => Expectation::None,
            };
            match value {
                Some(value) => {
                    let ty = self.check_expr_with(value, expected);
                    let range = self.expr_range(value);
                    coerce.push(self, Some(value), ty, range);
                }
                None => {
                    let unit = self.cx.interner.unit();
                    let range = self.expr_range(expr);
                    coerce.push(self, None, unit, range);
                }
            }
            self.breakables[index].coerce = Some(coerce);
        }
        self.breakables[index].may_break = true;
        self.intern(TyKind::Never)
    }

    pub fn check_return(&mut self, expr: ExprId, value: Option<ExprId>) -> Ty {
        if let Some(ret) = self.ret_ty {
            match value {
                Some(value) => {
                    let ty = self.check_expr_with(value, Expectation::Has(ret));
                    self.coerce(value, ty, ret);
                }
                None => {
                    let unit = self.cx.interner.unit();
                    let range = self.expr_range(expr);
                    self.demand_eq(range, ret, unit);
                }
            }
        } else if let Some(value) = value {
            // `return` outside a function was reported by lowering.
            self.check_expr(value);
        }
        self.intern(TyKind::Never)
    }
}
