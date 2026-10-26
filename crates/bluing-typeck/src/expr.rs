//! Checking expressions: the dispatcher, and the expressions that build
//! values directly (literals, tuples, arrays, references).

use bluing_base::Code;
use bluing_hir::body::{Expr, ExprId, FloatSuffix, IntSuffix, Literal, PathExpr};
use bluing_hir::resolve::Res;
use bluing_ty::layout;
use bluing_ty::ty::{Const, FloatTy, IntTy, Mutability, ParamIndex, Region, Ty, TyKind, UintTy};

use crate::coerce::CoerceMany;
use crate::fn_ctxt::{Expectation, FnCtxt};

impl FnCtxt<'_, '_> {
    pub fn check_expr(&mut self, expr: ExprId) -> Ty {
        self.check_expr_with(expr, Expectation::None)
    }

    /// Checks `expr` and requires its value to coerce to `expected`.
    pub fn check_expr_coercible(&mut self, expr: ExprId, expected: Ty) -> Ty {
        let ty = self.check_expr_with(expr, Expectation::Has(expected));
        self.coerce(expr, ty, expected);
        expected
    }

    pub fn check_expr_with(&mut self, expr: ExprId, expected: Expectation) -> Ty {
        let body = self.body;
        let ty = match &body.exprs[expr] {
            Expr::Missing => self.cx.interner.error(),
            Expr::Literal(literal) => self.check_literal(expr, literal, expected),
            Expr::Path(PathExpr::Local(local)) => self.local_ty(*local),
            Expr::Path(PathExpr::Item(path)) => self.check_value_path(expr, path),
            Expr::Block(block) => self.check_block(block, expected),
            Expr::If {
                condition,
                then_branch,
                else_branch,
            } => self.check_if(*condition, *then_branch, *else_branch, expected),
            Expr::Loop { body, target } => self.check_loop(*body, *target, expected),
            Expr::While {
                condition,
                body,
                target,
            } => self.check_while(*condition, *body, *target),
            Expr::Break { target, value } => self.check_break(expr, *target, *value),
            Expr::Continue { .. } => self.intern(TyKind::Never),
            Expr::Return(value) => self.check_return(expr, *value),
            Expr::Call { callee, args } => self.check_call(expr, *callee, args),
            Expr::MethodCall {
                receiver,
                method,
                method_range,
                generic_args,
                args,
            } => self.check_method_call(
                expr,
                *receiver,
                method,
                *method_range,
                generic_args.as_ref(),
                args,
            ),
            Expr::Field {
                base,
                name,
                name_range,
            } => self.check_field(expr, *base, name, *name_range),
            Expr::Struct { path, fields, base } => {
                self.check_struct_literal(expr, path, fields, *base)
            }
            Expr::Tuple(elements) => self.check_tuple(elements, expected),
            Expr::Array(elements) => self.check_array(elements, expected),
            Expr::Repeat { value, count } => self.check_repeat(*value, *count, expected),
            Expr::Index { base, index } => self.check_index(expr, *base, *index),
            Expr::Ref {
                mutable,
                expr: inner,
            } => self.check_ref(*mutable, *inner, expected),
            Expr::Unary { op, expr: inner } => self.check_unary(expr, *op, *inner, expected),
            Expr::Binary { op, lhs, rhs } => self.check_binary(expr, *op, *lhs, *rhs, expected),
            Expr::Assign { lhs, rhs } => self.check_assign(*lhs, *rhs),
            Expr::CompoundAssign { op, lhs, rhs } => {
                self.check_compound_assign(expr, *op, *lhs, *rhs)
            }
        };
        self.results.expr_tys.insert(expr, ty);
        if matches!(self.kind(ty), TyKind::Never) {
            self.diverges = true;
        }
        ty
    }

    fn check_literal(&mut self, expr: ExprId, literal: &Literal, expected: Expectation) -> Ty {
        match literal {
            Literal::Bool(_) => self.cx.interner.bool(),
            Literal::Char(_) => self.intern(TyKind::Char),
            Literal::Byte(_) => self.intern(TyKind::Uint(UintTy::U8)),
            Literal::Str(_) => {
                let str_ty = self.intern(TyKind::Str);
                self.intern(TyKind::Ref(Region::Static, str_ty, Mutability::Not))
            }
            Literal::ByteStr(bytes) => {
                let u8_ty = self.intern(TyKind::Uint(UintTy::U8));
                let array = self.intern(TyKind::Array(u8_ty, Const::Value(bytes.len() as u128)));
                self.intern(TyKind::Ref(Region::Static, array, Mutability::Not))
            }
            Literal::Int { value, suffix } => {
                self.int_literals.push((expr, *value, false));
                match suffix {
                    Some(suffix) => self.intern(int_suffix_kind(*suffix)),
                    None => match expected.ty().map(|ty| self.kind(ty)) {
                        // An expected integer type decides the literal's type.
                        Some(kind @ (TyKind::Int(_) | TyKind::Uint(_))) => self.intern(kind),
                        _ => self.infer.new_int_var(&mut self.cx.interner),
                    },
                }
            }
            Literal::Float { text, suffix } => {
                self.float_literals.push((expr, text.clone()));
                match suffix {
                    Some(FloatSuffix::F32) => self.intern(TyKind::Float(FloatTy::F32)),
                    Some(FloatSuffix::F64) => self.intern(TyKind::Float(FloatTy::F64)),
                    None => match expected.ty().map(|ty| self.kind(ty)) {
                        Some(kind @ TyKind::Float(_)) => self.intern(kind),
                        _ => self.infer.new_float_var(&mut self.cx.interner),
                    },
                }
            }
        }
    }

    fn check_tuple(&mut self, elements: &[ExprId], expected: Expectation) -> Ty {
        let expected_elements = match expected.ty().map(|ty| self.kind(ty)) {
            Some(TyKind::Tuple(list)) => Some(self.cx.interner.tys(list).to_vec()),
            _ => None,
        };
        let tys = elements
            .iter()
            .enumerate()
            .map(
                |(i, &element)| match expected_elements.as_ref().and_then(|tys| tys.get(i)) {
                    Some(&expected) => self.check_expr_coercible(element, expected),
                    None => self.check_expr(element),
                },
            )
            .collect();
        let list = self.cx.interner.ty_list(tys);
        self.intern(TyKind::Tuple(list))
    }

    /// The element type an expected array or slice type gives.
    fn expected_element(&mut self, expected: Expectation) -> Option<Ty> {
        match expected.ty().map(|ty| self.kind(ty)) {
            Some(TyKind::Array(element, _) | TyKind::Slice(element)) => Some(element),
            _ => None,
        }
    }

    fn check_array(&mut self, elements: &[ExprId], expected: Expectation) -> Ty {
        let expected_element = self.expected_element(expected);
        let element = if elements.is_empty() {
            expected_element.unwrap_or_else(|| self.new_var())
        } else {
            let mut coerce = CoerceMany::new(expected_element);
            for &element in elements {
                let hint = match coerce.target() {
                    Some(ty) => Expectation::Has(ty),
                    None => Expectation::None,
                };
                let ty = self.check_expr_with(element, hint);
                let range = self.expr_range(element);
                coerce.push(self, Some(element), ty, range);
            }
            coerce.complete(self)
        };
        self.intern(TyKind::Array(element, Const::Value(elements.len() as u128)))
    }

    fn check_repeat(&mut self, value: ExprId, count: ExprId, expected: Expectation) -> Ty {
        let usize_ty = self.cx.interner.usize();
        let length = self.array_length(count);
        self.results.expr_tys.insert(count, usize_ty);
        let element = match self.expected_element(expected) {
            Some(expected) => self.check_expr_coercible(value, expected),
            None => self.check_expr(value),
        };
        // Repeating a value more than once copies it.
        let copies = !matches!(length, Const::Value(0 | 1) | Const::Error);
        if copies && !self.is_trivially_copy(element) {
            let range = self.expr_range(value);
            let message = "repeat expressions with elements that are not of a primitive type are not supported yet";
            self.error(range, Code::NotYetSupported, message);
        }
        self.intern(TyKind::Array(element, length))
    }

    /// The constant length of a repeat expression: an integer literal or a
    /// `usize` const parameter, so far.
    fn array_length(&mut self, count: ExprId) -> Const {
        let range = self.expr_range(count);
        let body = self.body;
        match &body.exprs[count] {
            Expr::Literal(Literal::Int { value, suffix }) => {
                if !matches!(suffix, None | Some(IntSuffix::Usize)) {
                    self.error(
                        range,
                        Code::TypeMismatch,
                        "mismatched types: expected `usize`",
                    );
                    return Const::Error;
                }
                if !layout::int_fits(&TyKind::Uint(UintTy::Usize), *value) {
                    self.error(range, Code::Overflow, "literal out of range for `usize`");
                    return Const::Error;
                }
                Const::Value(*value)
            }
            Expr::Path(PathExpr::Item(path)) => match body.path_res(path).map(|res| res.base) {
                Some(Res::GenericParam(id)) => Const::Param(ParamIndex(
                    bluing_ty::generics::param_index(self.cx.hir, id).0,
                )),
                Some(Res::Error) | None => Const::Error,
                Some(_) => {
                    let message =
                        "only literals and const parameters are supported as array lengths so far";
                    self.error(range, Code::NotYetSupported, message);
                    Const::Error
                }
            },
            Expr::Missing => Const::Error,
            _ => {
                let message =
                    "only literals and const parameters are supported as array lengths so far";
                self.error(range, Code::NotYetSupported, message);
                Const::Error
            }
        }
    }

    /// Whether `ty` is `Copy` by the language's built-in rules, without
    /// consulting trait impls (which milestone 3c adds).
    pub fn is_trivially_copy(&mut self, ty: Ty) -> bool {
        match self.kind(ty) {
            TyKind::Bool
            | TyKind::Char
            | TyKind::Int(_)
            | TyKind::Uint(_)
            | TyKind::Float(_)
            | TyKind::Never
            | TyKind::RawPtr(..)
            | TyKind::FnPtr(_)
            | TyKind::FnDef(..)
            | TyKind::Error
            | TyKind::Ref(_, _, Mutability::Not) => true,
            TyKind::Tuple(list) => {
                let tys = self.cx.interner.tys(list).to_vec();
                tys.into_iter().all(|ty| self.is_trivially_copy(ty))
            }
            TyKind::Array(element, _) => self.is_trivially_copy(element),
            _ => false,
        }
    }

    fn check_ref(&mut self, mutable: bool, inner: ExprId, expected: Expectation) -> Ty {
        let hint = match expected.ty().map(|ty| self.kind(ty)) {
            Some(TyKind::Ref(_, pointee, _)) => Expectation::Has(pointee),
            _ => Expectation::None,
        };
        let inner_ty = self.check_expr_with(inner, hint);
        let range = self.expr_range(inner);
        self.reject_static_mut_ref(range, inner);
        let mutability = if mutable {
            Mutability::Mut
        } else {
            Mutability::Not
        };
        self.intern(TyKind::Ref(Region::Erased, inner_ty, mutability))
    }
}

fn int_suffix_kind(suffix: IntSuffix) -> TyKind {
    match suffix {
        IntSuffix::I8 => TyKind::Int(IntTy::I8),
        IntSuffix::I16 => TyKind::Int(IntTy::I16),
        IntSuffix::I32 => TyKind::Int(IntTy::I32),
        IntSuffix::I64 => TyKind::Int(IntTy::I64),
        IntSuffix::I128 => TyKind::Int(IntTy::I128),
        IntSuffix::Isize => TyKind::Int(IntTy::Isize),
        IntSuffix::U8 => TyKind::Uint(UintTy::U8),
        IntSuffix::U16 => TyKind::Uint(UintTy::U16),
        IntSuffix::U32 => TyKind::Uint(UintTy::U32),
        IntSuffix::U64 => TyKind::Uint(UintTy::U64),
        IntSuffix::U128 => TyKind::Uint(UintTy::U128),
        IntSuffix::Usize => TyKind::Uint(UintTy::Usize),
    }
}
