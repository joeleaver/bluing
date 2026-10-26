//! Operators, indexing, dereferencing and assignment on primitive types,
//! with rustc's built-in rules. Operators on other types go through traits
//! (`Add`, `Index`, `PartialEq`, ...), which milestone 3c adds.

use bluing_base::Code;
use bluing_hir::DefId;
use bluing_hir::body::{ArithOp, BinaryOp, CmpOp, Expr, ExprId, Literal, PathExpr, UnaryOp};
use bluing_hir::resolve::Res;
use bluing_ty::ty::{InferTy, Ty, TyKind};

use crate::fn_ctxt::{Expectation, FnCtxt};

/// What an operand's type means for the built-in operators.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Integer,
    Float,
    Bool,
    /// `char`, `str` and other primitives with no arithmetic.
    OtherPrimitive,
    /// A type whose operators would come from trait impls.
    NonPrimitive,
    /// Not known yet.
    Unknown,
    Error,
}

impl FnCtxt<'_, '_> {
    fn class(&mut self, ty: Ty) -> Class {
        match self.kind(ty) {
            TyKind::Int(_) | TyKind::Uint(_) | TyKind::Infer(InferTy::Int(_)) => Class::Integer,
            TyKind::Float(_) | TyKind::Infer(InferTy::Float(_)) => Class::Float,
            TyKind::Bool => Class::Bool,
            TyKind::Char | TyKind::Str | TyKind::Never => Class::OtherPrimitive,
            TyKind::Infer(InferTy::Ty(_)) => Class::Unknown,
            TyKind::Error => Class::Error,
            _ => Class::NonPrimitive,
        }
    }

    fn annotations_needed(&mut self, expr: ExprId) {
        let range = self.expr_range(expr);
        self.error(
            range,
            Code::Inference,
            "type annotations needed: the type of this value is not known here",
        );
    }

    fn not_yet_on(&mut self, expr: ExprId, what: &str, ty: Ty) {
        let range = self.expr_range(expr);
        let message = format!(
            "{what} on values of type `{}` are not supported yet",
            self.display(ty)
        );
        self.error(range, Code::NotYetSupported, message);
    }

    pub fn check_unary(
        &mut self,
        expr: ExprId,
        op: UnaryOp,
        inner: ExprId,
        expected: Expectation,
    ) -> Ty {
        match op {
            UnaryOp::Deref => {
                let ty = self.check_expr(inner);
                match self.kind(ty) {
                    TyKind::Ref(_, pointee, _) => pointee,
                    TyKind::RawPtr(pointee, _) => {
                        let range = self.expr_range(expr);
                        self.require_unsafe(range, "dereferencing a raw pointer");
                        pointee
                    }
                    TyKind::Error => ty,
                    TyKind::Infer(InferTy::Ty(_)) => {
                        self.annotations_needed(inner);
                        self.cx.interner.error()
                    }
                    TyKind::Adt(..) | TyKind::Param(_) => {
                        self.not_yet_on(expr, "dereferences", ty);
                        self.cx.interner.error()
                    }
                    _ => {
                        let range = self.expr_range(expr);
                        let message = format!("type `{}` cannot be dereferenced", self.display(ty));
                        self.error(range, Code::Operator, message);
                        self.cx.interner.error()
                    }
                }
            }
            UnaryOp::Not | UnaryOp::Neg => {
                let ty = self.check_expr_with(inner, expected);
                if op == UnaryOp::Neg
                    && let Expr::Literal(Literal::Int { .. }) = self.body.exprs[inner]
                    && let Some(entry) = self.int_literals.iter_mut().find(|(e, ..)| *e == inner)
                {
                    entry.2 = true;
                }
                let kind = self.kind(ty);
                let ok = match (op, self.class(ty)) {
                    (_, Class::Error) => true,
                    (UnaryOp::Not, Class::Integer | Class::Bool) => true,
                    (UnaryOp::Neg, Class::Float) => true,
                    (UnaryOp::Neg, Class::Integer) => match kind {
                        TyKind::Uint(_) => false,
                        // Unknown signedness: checked once types are final.
                        TyKind::Infer(_) => {
                            self.deferred_negations.push(expr);
                            true
                        }
                        _ => true,
                    },
                    (_, Class::Unknown) => {
                        self.annotations_needed(inner);
                        return self.cx.interner.error();
                    }
                    (_, Class::NonPrimitive) => {
                        self.not_yet_on(expr, "operators", ty);
                        return self.cx.interner.error();
                    }
                    _ => false,
                };
                if !ok {
                    self.cannot_apply_unary(expr, op, ty);
                    return self.cx.interner.error();
                }
                ty
            }
        }
    }

    pub fn cannot_apply_unary(&mut self, expr: ExprId, op: UnaryOp, ty: Ty) {
        let symbol = if op == UnaryOp::Neg { "-" } else { "!" };
        let range = self.expr_range(expr);
        let message = format!(
            "cannot apply unary operator `{symbol}` to type `{}`",
            self.display(ty)
        );
        self.error(range, Code::Operator, message);
    }

    pub fn check_binary(
        &mut self,
        expr: ExprId,
        op: BinaryOp,
        lhs: ExprId,
        rhs: ExprId,
        expected: Expectation,
    ) -> Ty {
        match op {
            BinaryOp::Logic(_) => {
                let bool_ty = self.cx.interner.bool();
                self.check_expr_coercible(lhs, bool_ty);
                // The right operand may not run, so it does not make the
                // whole expression diverge.
                let diverges = self.diverges;
                self.check_expr_coercible(rhs, bool_ty);
                self.diverges = diverges;
                bool_ty
            }
            BinaryOp::Arith(op) => self.check_arith(expr, op, lhs, rhs, expected),
            BinaryOp::Cmp(op) => self.check_comparison(expr, op, lhs, rhs),
        }
    }

    fn check_arith(
        &mut self,
        expr: ExprId,
        op: ArithOp,
        lhs: ExprId,
        rhs: ExprId,
        expected: Expectation,
    ) -> Ty {
        let is_shift = matches!(op, ArithOp::Shl | ArithOp::Shr);
        let lhs_ty = self.check_expr_with(
            lhs,
            if is_shift {
                expected
            } else {
                Expectation::None
            },
        );
        let rhs_hint = if is_shift {
            Expectation::None
        } else {
            Expectation::Has(lhs_ty)
        };
        let rhs_ty = self.check_expr_with(rhs, rhs_hint);
        self.builtin_arith(expr, op, lhs, lhs_ty, rhs, rhs_ty)
    }

    /// The result type of `lhs op rhs` on primitive types.
    fn builtin_arith(
        &mut self,
        expr: ExprId,
        op: ArithOp,
        lhs: ExprId,
        lhs_ty: Ty,
        rhs: ExprId,
        rhs_ty: Ty,
    ) -> Ty {
        let (lhs_class, rhs_class) = (self.class(lhs_ty), self.class(rhs_ty));
        match (lhs_class, rhs_class) {
            (Class::Error, _) | (_, Class::Error) => return self.cx.interner.error(),
            (Class::Unknown, _) => {
                self.annotations_needed(lhs);
                return self.cx.interner.error();
            }
            (Class::NonPrimitive, _) | (_, Class::NonPrimitive) => {
                self.not_yet_on(expr, "operators", lhs_ty);
                return self.cx.interner.error();
            }
            (_, Class::Unknown) => {
                self.annotations_needed(rhs);
                return self.cx.interner.error();
            }
            _ => {}
        }
        let ok = match op {
            // Shifts take any integer types on both sides.
            ArithOp::Shl | ArithOp::Shr => {
                lhs_class == Class::Integer && rhs_class == Class::Integer
            }
            ArithOp::BitAnd | ArithOp::BitOr | ArithOp::BitXor => {
                matches!(lhs_class, Class::Integer | Class::Bool) && lhs_class == rhs_class
            }
            _ => matches!(lhs_class, Class::Integer | Class::Float) && lhs_class == rhs_class,
        };
        if !ok {
            let range = self.expr_range(expr);
            let message = format!(
                "cannot apply `{}` to `{}` and `{}`",
                op_symbol(op),
                self.display(lhs_ty),
                self.display(rhs_ty)
            );
            self.error(range, Code::Operator, message);
            return self.cx.interner.error();
        }
        if !matches!(op, ArithOp::Shl | ArithOp::Shr) {
            let range = self.expr_range(rhs);
            self.demand_eq(range, lhs_ty, rhs_ty);
        }
        lhs_ty
    }

    fn check_comparison(&mut self, expr: ExprId, op: CmpOp, lhs: ExprId, rhs: ExprId) -> Ty {
        let lhs_ty = self.check_expr(lhs);
        let rhs_ty = self.check_expr_with(rhs, Expectation::Has(lhs_ty));
        let bool_ty = self.cx.interner.bool();
        let class = self.class(lhs_ty);
        match class {
            Class::Error => {}
            Class::Unknown => self.annotations_needed(lhs),
            Class::Integer | Class::Float | Class::Bool => {
                let range = self.expr_range(rhs);
                self.demand_eq(range, lhs_ty, rhs_ty);
            }
            Class::OtherPrimitive if matches!(self.kind(lhs_ty), TyKind::Char) => {
                let range = self.expr_range(rhs);
                self.demand_eq(range, lhs_ty, rhs_ty);
            }
            _ => {
                let what = match op {
                    CmpOp::Eq | CmpOp::Ne => "equality comparisons",
                    _ => "ordering comparisons",
                };
                self.not_yet_on(expr, what, lhs_ty);
            }
        }
        bool_ty
    }

    pub fn check_compound_assign(
        &mut self,
        expr: ExprId,
        op: ArithOp,
        lhs: ExprId,
        rhs: ExprId,
    ) -> Ty {
        self.check_place(lhs);
        let lhs_ty = self.check_expr(lhs);
        let is_shift = matches!(op, ArithOp::Shl | ArithOp::Shr);
        let rhs_hint = if is_shift {
            Expectation::None
        } else {
            Expectation::Has(lhs_ty)
        };
        let rhs_ty = self.check_expr_with(rhs, rhs_hint);
        self.builtin_arith(expr, op, lhs, lhs_ty, rhs, rhs_ty);
        self.cx.interner.unit()
    }

    pub fn check_assign(&mut self, lhs: ExprId, rhs: ExprId) -> Ty {
        let destructuring = match &self.body.exprs[lhs] {
            Expr::Tuple(_) | Expr::Array(_) | Expr::Struct { .. } => true,
            // `S(a, b) = ...` destructures; any other call is not a place.
            Expr::Call { callee, .. } => match &self.body.exprs[*callee] {
                Expr::Path(PathExpr::Item(path)) => matches!(
                    self.body.path_res(path).map(|res| res.base),
                    Some(Res::Def(DefId::Struct(_) | DefId::Variant(_)) | Res::SelfTy(_))
                ),
                _ => false,
            },
            _ => false,
        };
        if destructuring {
            let range = self.expr_range(lhs);
            self.error(
                range,
                Code::NotYetSupported,
                "destructuring assignments are not supported yet",
            );
            self.check_expr(rhs);
            return self.cx.interner.unit();
        }
        self.check_place(lhs);
        let lhs_ty = self.check_expr(lhs);
        self.check_expr_coercible(rhs, lhs_ty);
        self.cx.interner.unit()
    }

    /// Reports an assignment target that is not a place expression.
    fn check_place(&mut self, expr: ExprId) {
        let is_place = match &self.body.exprs[expr] {
            Expr::Path(PathExpr::Local(_)) => true,
            Expr::Path(PathExpr::Item(path)) => matches!(
                self.body
                    .path_res(path)
                    .map(|res| (res.base, res.unresolved_segments)),
                Some((Res::Def(DefId::Static(_)), 0)) | Some((Res::Error, _)) | None
            ),
            Expr::Field { .. }
            | Expr::Index { .. }
            | Expr::Unary {
                op: UnaryOp::Deref, ..
            }
            | Expr::Missing => true,
            _ => false,
        };
        if !is_place {
            let range = self.expr_range(expr);
            self.error(
                range,
                Code::Place,
                "invalid left-hand side of assignment: this is not a place",
            );
        }
    }

    pub fn check_index(&mut self, expr: ExprId, base: ExprId, index: ExprId) -> Ty {
        let base_ty = self.check_expr(base);
        let usize_ty = self.cx.interner.usize();
        let mut current = base_ty;
        let mut derefs = 0;
        loop {
            match self.kind(current) {
                TyKind::Array(element, _) | TyKind::Slice(element) => {
                    if derefs > 0 {
                        self.results.adjustments.entry(base).or_default().derefs = derefs;
                    }
                    self.check_expr_coercible(index, usize_ty);
                    return element;
                }
                TyKind::Ref(_, pointee, _) => {
                    current = pointee;
                    derefs += 1;
                }
                TyKind::Error => {
                    self.check_expr(index);
                    return current;
                }
                TyKind::Infer(InferTy::Ty(_)) => {
                    self.annotations_needed(base);
                    self.check_expr(index);
                    return self.cx.interner.error();
                }
                TyKind::Adt(..) | TyKind::Param(_) => {
                    self.not_yet_on(expr, "indexing operations", base_ty);
                    self.check_expr(index);
                    return self.cx.interner.error();
                }
                _ => {
                    let range = self.expr_range(expr);
                    let message = format!(
                        "cannot index into a value of type `{}`",
                        self.display(base_ty)
                    );
                    self.error(range, Code::Operator, message);
                    self.check_expr(index);
                    return self.cx.interner.error();
                }
            }
        }
    }
}

fn op_symbol(op: ArithOp) -> &'static str {
    match op {
        ArithOp::Add => "+",
        ArithOp::Sub => "-",
        ArithOp::Mul => "*",
        ArithOp::Div => "/",
        ArithOp::Rem => "%",
        ArithOp::BitAnd => "&",
        ArithOp::BitOr => "|",
        ArithOp::BitXor => "^",
        ArithOp::Shl => "<<",
        ArithOp::Shr => ">>",
    }
}
