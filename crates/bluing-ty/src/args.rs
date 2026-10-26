//! Generic arguments of a type path, and the constants that can appear in
//! types: array lengths and const generic arguments.

use bluing_base::{Code, TextRange};
use bluing_hir::body::{IntSuffix, Literal, LiteralError, lower_literal};
use bluing_hir::hir::{
    ConstExprPtr, GenericArg as HirArg, GenericArgs, GenericParamKind, TypeRef, TypeRefKind,
};
use bluing_hir::ra_ap_syntax::{AstNode, ast};
use bluing_hir::resolve::Res;
use bluing_hir::{DefId, GenericOwner};

use crate::TyCx;
use crate::generics::{ParamKind, generics_of, hir_params};
use crate::lower::LowerCtx;
use crate::ty::{Const, GenericArg, ParamIndex, Region, Substs, Ty, TyKind, UintTy};

impl TyCx<'_> {
    /// The arguments of a path to a struct, enum or type alias, checking
    /// their number and kinds and filling in defaults and elided lifetimes.
    ///
    /// `prefix` holds the arguments for the parameters before the owner's
    /// own: a trait's `Self`.
    pub fn generic_args(
        &mut self,
        cx: &mut LowerCtx<'_>,
        owner: GenericOwner,
        args: Option<&GenericArgs>,
        range: TextRange,
        prefix: Vec<GenericArg>,
    ) -> Substs {
        let generics = generics_of(self.hir, owner);
        let own = generics.own().to_vec();
        let name = self.owner_name(owner);
        let mut lifetimes = Vec::new();
        let mut others = Vec::new();
        if let Some(args) = args {
            if args.parenthesized {
                let message = "parenthesized arguments are only allowed on the `Fn` traits";
                self.error(cx.file, range, Code::GenericArgs, message);
            }
            if !args.bindings.is_empty() {
                let message = "associated item constraints are not allowed here";
                self.error(cx.file, range, Code::GenericArgs, message);
            }
            for arg in &args.args {
                match arg {
                    HirArg::Lifetime(lifetime) if !others.is_empty() => {
                        let message =
                            "lifetime arguments must come before type and const arguments";
                        self.error(cx.file, lifetime.range, Code::GenericArgs, message);
                    }
                    HirArg::Lifetime(lifetime) => lifetimes.push(lifetime),
                    other => others.push(other),
                }
            }
        }
        let lifetime_params = own.iter().filter(|p| p.kind == ParamKind::Lifetime).count();
        let lifetimes_ok = lifetimes.is_empty() || lifetimes.len() == lifetime_params;
        if !lifetimes_ok {
            let message = format!(
                "`{name}` takes {lifetime_params} lifetime argument{} but {} were supplied",
                plural(lifetime_params),
                lifetimes.len()
            );
            self.error(cx.file, range, Code::GenericArgs, message);
        }
        let non_lifetime: Vec<_> = own
            .iter()
            .filter(|p| p.kind != ParamKind::Lifetime)
            .collect();
        let required = non_lifetime
            .iter()
            .filter(|p| {
                matches!(
                    p.kind,
                    ParamKind::Type { has_default: false }
                        | ParamKind::Const { has_default: false }
                )
            })
            .count();
        if others.len() > non_lifetime.len() || others.len() < required {
            let expected = if required == non_lifetime.len() {
                format!("{required}")
            } else {
                format!("between {required} and {}", non_lifetime.len())
            };
            let message = format!(
                "`{name}` takes {expected} generic argument{} but {} were supplied",
                plural(non_lifetime.len()),
                others.len()
            );
            self.error(cx.file, range, Code::GenericArgs, message);
        }

        let mut out: Vec<GenericArg> = prefix;
        let (mut next_lifetime, mut next_other) = (0, 0);
        for (local, param) in own.iter().enumerate() {
            let arg = match param.kind {
                ParamKind::Lifetime => {
                    let region = if lifetimes.is_empty() {
                        self.elided_region(cx, range)
                    } else if lifetimes_ok {
                        let lifetime = lifetimes[next_lifetime];
                        self.region(cx, Some(lifetime), lifetime.range)
                    } else {
                        Region::Error
                    };
                    next_lifetime += 1;
                    GenericArg::Region(region)
                }
                kind => {
                    let arg = match others.get(next_other) {
                        Some(arg) => self.lower_arg(cx, owner, local, kind, arg),
                        None => self.param_default(owner, local, &out),
                    };
                    next_other += 1;
                    arg
                }
            };
            out.push(arg);
        }
        self.interner.substs(out)
    }

    fn lower_arg(
        &mut self,
        cx: &mut LowerCtx<'_>,
        owner: GenericOwner,
        local: usize,
        kind: ParamKind,
        arg: &HirArg,
    ) -> GenericArg {
        match (kind, arg) {
            (ParamKind::Type { .. }, HirArg::Type(ty)) => {
                let names_constant =
                    matches!(single_path_res(cx, ty), Some(Res::Def(DefId::Const(_))))
                        || is_const_param(self, cx, ty);
                if names_constant {
                    let message = "a constant was provided where a type was expected";
                    self.error(cx.file, ty.range, Code::GenericArgs, message);
                    return GenericArg::Ty(self.interner.error());
                }
                GenericArg::Ty(self.lower_ty(cx, ty))
            }
            (ParamKind::Type { .. }, HirArg::Const(expr)) => {
                let range = expr_range(self, expr);
                let message = "a constant was provided where a type was expected";
                self.error(cx.file, range, Code::GenericArgs, message);
                GenericArg::Ty(self.interner.error())
            }
            (ParamKind::Const { .. }, HirArg::Type(ty)) => {
                let expected = self.const_param_ty(owner, local);
                match single_path_res(cx, ty) {
                    Some(Res::GenericParam(id)) if is_const_param(self, cx, ty) => {
                        GenericArg::Const(Const::Param(crate::generics::param_index(self.hir, id)))
                    }
                    Some(Res::Def(DefId::Const(_))) => {
                        let message = "constant items as generic arguments are not supported yet";
                        self.error(cx.file, ty.range, Code::NotYetSupported, message);
                        GenericArg::Const(Const::Error)
                    }
                    _ => {
                        let message = format!(
                            "a type was provided where a constant of type `{}` was expected",
                            self.display(expected, None)
                        );
                        self.error(cx.file, ty.range, Code::GenericArgs, message);
                        GenericArg::Const(Const::Error)
                    }
                }
            }
            (ParamKind::Const { .. }, HirArg::Const(expr)) => {
                let expected = self.const_param_ty(owner, local);
                GenericArg::Const(self.lower_const_expr(cx, expr, expected))
            }
            (ParamKind::Lifetime, _) | (_, HirArg::Lifetime(_)) => {
                unreachable!("lifetimes are matched separately")
            }
        }
    }

    /// The default of the `local`th own parameter of `owner`, with the
    /// arguments before it substituted.
    fn param_default(
        &mut self,
        owner: GenericOwner,
        local: usize,
        before: &[GenericArg],
    ) -> GenericArg {
        let id = bluing_hir::GenericParamId {
            owner,
            index: local as u32,
        };
        match self.param_default_arg(id) {
            Some(GenericArg::Ty(ty)) => {
                let substs = self.interner.substs(before.to_vec());
                GenericArg::Ty(crate::fold::subst(&mut self.interner, ty, substs))
            }
            Some(default) => default,
            // A missing argument without a default was reported by the caller.
            None => match hir_params(self.hir, owner).params[local].kind {
                GenericParamKind::Const { .. } => GenericArg::Const(Const::Error),
                _ => GenericArg::Ty(self.interner.error()),
            },
        }
    }

    /// Whether `ty` mentions a type parameter with index `from` or later.
    pub(crate) fn mentions_param_from(&self, ty: Ty, from: usize) -> bool {
        match self.interner.kind(ty) {
            TyKind::Param(index) => index.0 as usize >= from,
            TyKind::Adt(_, substs) | TyKind::FnDef(_, substs) => {
                self.interner.args(*substs).iter().any(|arg| match arg {
                    GenericArg::Ty(ty) => self.mentions_param_from(*ty, from),
                    GenericArg::Const(Const::Param(index)) => index.0 as usize >= from,
                    _ => false,
                })
            }
            TyKind::Ref(_, inner, _) | TyKind::RawPtr(inner, _) | TyKind::Slice(inner) => {
                self.mentions_param_from(*inner, from)
            }
            TyKind::Array(elem, len) => {
                self.mentions_param_from(*elem, from)
                    || matches!(len, Const::Param(index) if index.0 as usize >= from)
            }
            TyKind::Tuple(list) => self
                .interner
                .tys(*list)
                .iter()
                .any(|ty| self.mentions_param_from(*ty, from)),
            TyKind::FnPtr(sig) => self
                .interner
                .tys(sig.inputs_and_output)
                .iter()
                .any(|ty| self.mentions_param_from(*ty, from)),
            _ => false,
        }
    }

    /// Lowers a constant in a type: an array length or a const argument.
    /// Supports literals and const parameters so far.
    pub fn lower_const_expr(
        &mut self,
        cx: &mut LowerCtx<'_>,
        expr: &ConstExprPtr,
        expected: Ty,
    ) -> Const {
        let root = self.files.get(expr.file).tree.syntax().clone();
        let node = expr.value.to_node(&root);
        let range = node.syntax().text_range();
        match &node {
            ast::Expr::Literal(literal) => match lower_literal(literal) {
                Ok(literal) => self.const_literal(cx, range, &literal, expected),
                Err(LiteralError::Reported) => Const::Error,
                Err(LiteralError::Error(code, message)) => {
                    self.error(cx.file, range, code, message);
                    Const::Error
                }
            },
            ast::Expr::PathExpr(path) => {
                let name = path
                    .path()
                    .and_then(|path| path.as_single_name_ref())
                    .map(|name| bluing_hir::Name::new(name.text()));
                let param = name.and_then(|name| {
                    cx.generics.and_then(|generics| {
                        generics.params.iter().rposition(|p| {
                            p.name == name && matches!(p.kind, ParamKind::Const { .. })
                        })
                    })
                });
                match param {
                    Some(index) => Const::Param(ParamIndex(index as u32)),
                    None => {
                        let message = "only literals and const parameters are supported in constants in types so far";
                        self.error(cx.file, range, Code::NotYetSupported, message);
                        Const::Error
                    }
                }
            }
            _ => {
                let message =
                    "only literals and const parameters are supported in constants in types so far";
                self.error(cx.file, range, Code::NotYetSupported, message);
                Const::Error
            }
        }
    }

    fn const_literal(
        &mut self,
        cx: &mut LowerCtx<'_>,
        range: TextRange,
        literal: &Literal,
        expected: Ty,
    ) -> Const {
        let expected_kind = self.interner.kind(expected).clone();
        let value = match (literal, &expected_kind) {
            (Literal::Int { value, suffix }, TyKind::Int(_) | TyKind::Uint(_)) => {
                if let Some(suffix) = suffix
                    && suffix_ty(self, *suffix) != expected
                {
                    self.mismatch(cx, range, expected);
                    return Const::Error;
                }
                if !crate::layout::int_fits(&expected_kind, *value) {
                    let message = format!(
                        "literal out of range for `{}`",
                        self.display(expected, None)
                    );
                    self.error(cx.file, range, Code::Overflow, message);
                    return Const::Error;
                }
                *value
            }
            (Literal::Bool(value), TyKind::Bool) => u128::from(*value),
            (Literal::Char(c), TyKind::Char) => u128::from(u32::from(*c)),
            (_, TyKind::Error) => return Const::Error,
            _ => {
                self.mismatch(cx, range, expected);
                return Const::Error;
            }
        };
        Const::Value(value)
    }

    fn mismatch(&mut self, cx: &LowerCtx<'_>, range: TextRange, expected: Ty) {
        let message = format!(
            "mismatched types: expected a constant of type `{}`",
            self.display(expected, None)
        );
        self.error(cx.file, range, Code::TypeMismatch, message);
    }

    /// The declared type of a const parameter (`usize` for array lengths).
    pub fn const_param_ty(&mut self, owner: GenericOwner, local: usize) -> Ty {
        let id = bluing_hir::GenericParamId {
            owner,
            index: local as u32,
        };
        match self.items.const_param_tys.get(&id) {
            Some(&ty) => ty,
            None => self.interner.error(),
        }
    }

    pub fn usize_ty(&mut self) -> Ty {
        self.interner.intern(TyKind::Uint(UintTy::Usize))
    }
}

fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

fn suffix_ty(cx: &mut TyCx<'_>, suffix: IntSuffix) -> Ty {
    use crate::ty::IntTy;
    let kind = match suffix {
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
    };
    cx.interner.intern(kind)
}

/// The resolution of a type argument written as a lone path.
fn single_path_res(cx: &LowerCtx<'_>, ty: &TypeRef) -> Option<Res> {
    let TypeRefKind::Path(path) = &ty.kind else {
        return None;
    };
    let res = cx.paths.path(path)?;
    (res.unresolved_segments == 0 && path.segments.len() == 1).then_some(res.base)
}

/// Whether a type argument names a const parameter.
fn is_const_param(cx: &TyCx<'_>, lower: &LowerCtx<'_>, ty: &TypeRef) -> bool {
    match single_path_res(lower, ty) {
        Some(Res::GenericParam(id)) => matches!(
            hir_params(cx.hir, id.owner).params[id.index as usize].kind,
            GenericParamKind::Const { .. }
        ),
        _ => false,
    }
}

fn expr_range(cx: &TyCx<'_>, expr: &ConstExprPtr) -> TextRange {
    let root = cx.files.get(expr.file).tree.syntax().clone();
    expr.value.to_node(&root).syntax().text_range()
}
