//! Lowering of types, paths, generic parameters, bounds and lifetimes.

use bluing_base::{Code, Diagnostic, FileId, TextRange};
use bluing_syntax::ra_ap_syntax::{
    AstNode, AstPtr,
    ast::{self, HasGenericArgs, HasGenericParams, HasName, HasTypeBounds},
};

use crate::hir::{
    AssocBinding, AssocBindingValue, ConstExprPtr, GenericArg, GenericArgs, GenericParam,
    GenericParamKind, GenericParams, InFile, LifetimeRef, Path, PathKind, PathSegment,
    QualifiedSelf, TypeBound, TypeRef, WherePredicate,
};
use crate::{LifetimeId, Name, PathId};

/// Lowers syntax inside one file, allocating path and lifetime ids.
pub(crate) struct TypeLowerer<'a> {
    pub file: FileId,
    pub path_count: &'a mut u32,
    pub lifetime_count: &'a mut u32,
    pub diagnostics: &'a mut Vec<Diagnostic>,
}

impl TypeLowerer<'_> {
    fn error(&mut self, range: TextRange, code: Code, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::new(self.file, range, code, message));
    }

    fn range(node: &impl AstNode) -> TextRange {
        node.syntax().text_range()
    }

    pub fn const_expr(&self, arg: Option<ast::ConstArg>) -> Option<ConstExprPtr> {
        self.expr(arg.and_then(|arg| arg.expr()))
    }

    pub fn expr(&self, expr: Option<ast::Expr>) -> Option<ConstExprPtr> {
        expr.map(|expr| InFile {
            file: self.file,
            value: AstPtr::new(&expr),
        })
    }

    pub fn ty_opt(&mut self, ty: Option<ast::Type>) -> TypeRef {
        // A missing type is a syntax error the parser already reported.
        ty.map_or(TypeRef::Error, |ty| self.ty(&ty))
    }

    pub fn ty(&mut self, ty: &ast::Type) -> TypeRef {
        match ty {
            ast::Type::PathType(ty) => match ty.path() {
                Some(path) => self.path(&path).map_or(TypeRef::Error, TypeRef::Path),
                None => TypeRef::Error,
            },
            ast::Type::RefType(ty) => TypeRef::Reference {
                lifetime: ty.lifetime().map(|lt| self.lifetime(&lt)),
                mutable: ty.mut_token().is_some(),
                inner: Box::new(self.ty_opt(ty.ty())),
            },
            ast::Type::PtrType(ty) => TypeRef::RawPtr {
                mutable: ty.mut_token().is_some(),
                inner: Box::new(self.ty_opt(ty.ty())),
            },
            ast::Type::SliceType(ty) => TypeRef::Slice(Box::new(self.ty_opt(ty.ty()))),
            ast::Type::ArrayType(ty) => TypeRef::Array {
                elem: Box::new(self.ty_opt(ty.ty())),
                len: self.const_expr(ty.const_arg()),
            },
            ast::Type::TupleType(ty) => {
                TypeRef::Tuple(ty.fields().map(|field| self.ty(&field)).collect())
            }
            ast::Type::NeverType(_) => TypeRef::Never,
            ast::Type::InferType(_) => TypeRef::Infer,
            ast::Type::ParenType(ty) => self.ty_opt(ty.ty()),
            ast::Type::FnPtrType(ty) => {
                let params = ty
                    .param_list()
                    .into_iter()
                    .flat_map(|list| list.params())
                    .map(|param| self.ty_opt(param.ty()))
                    .collect();
                let ret = match ty.ret_type() {
                    Some(ret) => self.ty_opt(ret.ty()),
                    None => TypeRef::Tuple(Vec::new()),
                };
                TypeRef::FnPtr {
                    params,
                    ret: Box::new(ret),
                    is_unsafe: ty.unsafe_token().is_some(),
                }
            }
            ast::Type::ImplTraitType(ty) => TypeRef::ImplTrait(self.bounds(ty.type_bound_list())),
            ast::Type::DynTraitType(ty) => TypeRef::DynTrait(self.bounds(ty.type_bound_list())),
            ast::Type::ForType(ty) => {
                let message = "explicit `for<..>` binders are not supported yet";
                self.error(Self::range(ty), Code::NotYetSupported, message);
                TypeRef::Error
            }
            ast::Type::MacroType(ty) => {
                let message = "macros in type position are not supported yet";
                self.error(Self::range(ty), Code::NotYetSupported, message);
                TypeRef::Error
            }
            ast::Type::PatternType(ty) => {
                let message = "pattern types are not supported in Bluing";
                self.error(Self::range(ty), Code::Unsupported, message);
                TypeRef::Error
            }
        }
    }

    pub fn path(&mut self, path: &ast::Path) -> Option<Path> {
        let range = Self::range(path);
        let mut kind = PathKind::Plain;
        let mut qself = None;
        let mut segments = Vec::new();
        for (index, segment) in path.segments().enumerate() {
            // A leading `::` is attached to the first segment.
            if index == 0 && segment.coloncolon_token().is_some() {
                kind = PathKind::Global;
            }
            let leading = segments.is_empty() && qself.is_none();
            let segment_range = Self::range(&segment);
            if segment.return_type_syntax().is_some() {
                let message = "return type notation is not supported in Bluing";
                self.error(segment_range, Code::Unsupported, message);
                return None;
            }
            match segment.kind()? {
                ast::PathSegmentKind::Name(name) => {
                    let args = self.segment_args(&segment);
                    segments.push(PathSegment {
                        name: Name::new(name.text()),
                        range: segment_range,
                        args,
                    });
                }
                ast::PathSegmentKind::SelfTypeKw => {
                    let args = self.segment_args(&segment);
                    segments.push(PathSegment {
                        name: Name::new("Self"),
                        range: segment_range,
                        args,
                    });
                }
                ast::PathSegmentKind::CrateKw if leading && index == 0 => kind = PathKind::Crate,
                ast::PathSegmentKind::SelfKw if leading && index == 0 => {
                    kind = PathKind::SelfModule;
                }
                ast::PathSegmentKind::SuperKw
                    if leading && matches!(kind, PathKind::Plain | PathKind::Super(_)) =>
                {
                    kind = match kind {
                        PathKind::Super(n) => PathKind::Super(n + 1),
                        _ => PathKind::Super(1),
                    };
                }
                ast::PathSegmentKind::SuperKw
                    if leading && kind == PathKind::SelfModule && index == 1 =>
                {
                    // `self::super::` is the same as `super::`.
                    kind = PathKind::Super(1);
                }
                ast::PathSegmentKind::Type {
                    type_ref,
                    trait_ref,
                } if index == 0 => {
                    let ty = self.ty_opt(type_ref);
                    let trait_ref = match trait_ref.and_then(|t| t.path()) {
                        Some(path) => Some(self.path(&path)?),
                        None => None,
                    };
                    qself = Some(Box::new(QualifiedSelf { ty, trait_ref }));
                }
                ast::PathSegmentKind::CrateKw
                | ast::PathSegmentKind::SelfKw
                | ast::PathSegmentKind::SuperKw
                | ast::PathSegmentKind::Type { .. } => {
                    let message = format!("`{segment}` can only be used at the start of a path");
                    self.error(segment_range, Code::Unresolved, message);
                    return None;
                }
            }
        }
        if segments.is_empty() && qself.is_none() && kind != PathKind::Plain {
            let message = "a path must end with a name";
            self.error(range, Code::Unresolved, message);
            return None;
        }
        let id = PathId(*self.path_count);
        *self.path_count += 1;
        Some(Path {
            id,
            kind,
            qself,
            segments,
            range,
        })
    }

    fn segment_args(&mut self, segment: &ast::PathSegment) -> Option<GenericArgs> {
        if let Some(list) = segment.parenthesized_arg_list() {
            let inputs = list.type_args().map(|arg| self.ty_opt(arg.ty())).collect();
            let output = match segment.ret_type() {
                Some(ret) => self.ty_opt(ret.ty()),
                None => TypeRef::Tuple(Vec::new()),
            };
            return Some(GenericArgs {
                args: vec![GenericArg::Type(TypeRef::Tuple(inputs))],
                bindings: vec![AssocBinding {
                    name: Name::new("Output"),
                    range: Self::range(&list),
                    args: None,
                    value: AssocBindingValue::Type(output),
                }],
                parenthesized: true,
            });
        }
        let list = segment.generic_arg_list()?;
        Some(self.generic_args(&list))
    }

    fn generic_args(&mut self, list: &ast::GenericArgList) -> GenericArgs {
        let mut args = Vec::new();
        let mut bindings = Vec::new();
        for arg in list.generic_args() {
            match arg {
                ast::GenericArg::TypeArg(arg) => args.push(GenericArg::Type(self.ty_opt(arg.ty()))),
                ast::GenericArg::LifetimeArg(arg) => {
                    if let Some(lt) = arg.lifetime() {
                        args.push(GenericArg::Lifetime(self.lifetime(&lt)));
                    }
                }
                ast::GenericArg::ConstArg(arg) => {
                    if let Some(expr) = self.expr(arg.expr()) {
                        args.push(GenericArg::Const(expr));
                    }
                }
                ast::GenericArg::AssocTypeArg(arg) => {
                    if let Some(binding) = self.assoc_binding(&arg) {
                        bindings.push(binding);
                    }
                }
            }
        }
        GenericArgs {
            args,
            bindings,
            parenthesized: false,
        }
    }

    fn assoc_binding(&mut self, arg: &ast::AssocTypeArg) -> Option<AssocBinding> {
        let range = Self::range(arg);
        let name = arg.name_ref()?;
        if arg.const_arg().is_some() || arg.param_list().is_some() {
            let message = "associated constant and return type bindings are not supported";
            self.error(range, Code::Unsupported, message);
            return None;
        }
        let args = arg.generic_arg_list().map(|list| self.generic_args(&list));
        let value = match arg.ty() {
            Some(ty) => AssocBindingValue::Type(self.ty(&ty)),
            None => AssocBindingValue::Bounds(self.bounds(arg.type_bound_list())),
        };
        Some(AssocBinding {
            name: Name::new(name.text()),
            range,
            args,
            value,
        })
    }

    pub fn lifetime(&mut self, lifetime: &ast::Lifetime) -> LifetimeRef {
        let text = lifetime.text();
        let id = LifetimeId(*self.lifetime_count);
        *self.lifetime_count += 1;
        LifetimeRef {
            id,
            name: Name::new(text.strip_prefix('\'').unwrap_or(text)),
            range: Self::range(lifetime),
        }
    }

    pub fn bounds(&mut self, list: Option<ast::TypeBoundList>) -> Vec<TypeBound> {
        list.into_iter()
            .flat_map(|list| list.bounds())
            .map(|bound| self.bound(&bound))
            .collect()
    }

    fn bound(&mut self, bound: &ast::TypeBound) -> TypeBound {
        let range = Self::range(bound);
        if bound.for_binder().is_some() {
            let message = "explicit `for<..>` binders are not supported yet";
            self.error(range, Code::NotYetSupported, message);
            return TypeBound::Error;
        }
        if bound.use_token().is_some() {
            let message = "`use<..>` bounds are not supported yet";
            self.error(range, Code::NotYetSupported, message);
            return TypeBound::Error;
        }
        if bound.const_token().is_some()
            || bound.tilde_token().is_some()
            || bound.async_token().is_some()
        {
            let message = "`const` and `async` trait bounds are not supported in Bluing";
            self.error(range, Code::Unsupported, message);
            return TypeBound::Error;
        }
        if let Some(lifetime) = bound.lifetime() {
            return TypeBound::Lifetime(self.lifetime(&lifetime));
        }
        let mut ty = bound.ty();
        while let Some(ast::Type::ParenType(paren)) = &ty {
            ty = paren.ty();
        }
        match ty {
            Some(ast::Type::PathType(ty)) => match ty.path().and_then(|p| self.path(&p)) {
                Some(path) => TypeBound::Trait {
                    path,
                    maybe: bound.question_mark_token().is_some(),
                },
                None => TypeBound::Error,
            },
            Some(_) => {
                self.error(range, Code::WrongKind, "expected a trait or lifetime bound");
                TypeBound::Error
            }
            None => TypeBound::Error,
        }
    }

    pub fn generics(&mut self, node: &impl HasGenericParams) -> GenericParams {
        let mut generics = GenericParams::default();
        for param in node
            .generic_param_list()
            .into_iter()
            .flat_map(|list| list.generic_params())
        {
            if let Some(param) = self.generic_param(&param) {
                generics.params.push(param);
            }
        }
        for pred in node
            .where_clause()
            .into_iter()
            .flat_map(|clause| clause.predicates())
        {
            if let Some(pred) = self.where_predicate(&pred) {
                generics.where_predicates.push(pred);
            }
        }
        generics
    }

    fn generic_param(&mut self, param: &ast::GenericParam) -> Option<GenericParam> {
        let (name, range, kind) = match param {
            ast::GenericParam::LifetimeParam(param) => {
                let lifetime = param.lifetime()?;
                let text = lifetime.text();
                let name = Name::new(text.strip_prefix('\'').unwrap_or(text));
                let bounds = self.lifetime_bounds(param.type_bound_list());
                (
                    name,
                    Self::range(&lifetime),
                    GenericParamKind::Lifetime { bounds },
                )
            }
            ast::GenericParam::TypeParam(param) => {
                let name = param.name()?;
                let kind = GenericParamKind::Type {
                    bounds: self.bounds(param.type_bound_list()),
                    default: param.default_type().map(|ty| self.ty(&ty)),
                };
                (Name::new(name.text()), Self::range(&name), kind)
            }
            ast::GenericParam::ConstParam(param) => {
                let name = param.name()?;
                let kind = GenericParamKind::Const {
                    ty: self.ty_opt(param.ty()),
                    default: self.const_expr(param.default_val()),
                };
                (Name::new(name.text()), Self::range(&name), kind)
            }
        };
        Some(GenericParam { name, range, kind })
    }

    fn lifetime_bounds(&mut self, list: Option<ast::TypeBoundList>) -> Vec<LifetimeRef> {
        let mut lifetimes = Vec::new();
        for bound in list.into_iter().flat_map(|list| list.bounds()) {
            match bound.lifetime() {
                Some(lifetime) => lifetimes.push(self.lifetime(&lifetime)),
                None => {
                    let message = "a lifetime can only be bounded by other lifetimes";
                    self.error(Self::range(&bound), Code::WrongKind, message);
                }
            }
        }
        lifetimes
    }

    fn where_predicate(&mut self, pred: &ast::WherePred) -> Option<WherePredicate> {
        if pred.for_binder().is_some() {
            let message = "explicit `for<..>` binders are not supported yet";
            self.error(Self::range(pred), Code::NotYetSupported, message);
            return None;
        }
        if let Some(lifetime) = pred.lifetime() {
            let lifetime = self.lifetime(&lifetime);
            let bounds = self.lifetime_bounds(pred.type_bound_list());
            return Some(WherePredicate::Lifetime { lifetime, bounds });
        }
        let ty = self.ty_opt(pred.ty());
        let bounds = self.bounds(pred.type_bound_list());
        Some(WherePredicate::Type { ty, bounds })
    }
}
