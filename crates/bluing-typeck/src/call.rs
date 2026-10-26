//! Paths in expressions and function calls.

use bluing_base::Code;
use bluing_hir::body::ExprId;
use bluing_hir::hir::{AssocItem, Fields, Path};
use bluing_hir::resolve::{FieldOwner, PathRes, Res, SelfTyOf};
use bluing_hir::{DefId, FnId, GenericOwner, Name, StructId, VariantId};
use bluing_ty::fold::subst;
use bluing_ty::generics::{ParamKind, generics_of};
use bluing_ty::ty::{AdtId, CallableId, GenericArg, Region, Substs, Ty, TyKind};

use crate::fn_ctxt::FnCtxt;
use crate::results::AssocValue;

impl FnCtxt<'_, '_> {
    fn not_yet(&mut self, expr: ExprId, message: &str) -> Ty {
        let range = self.expr_range(expr);
        self.error(range, Code::NotYetSupported, message);
        self.cx.interner.error()
    }

    /// Erased arguments for an owner whose only parameters are lifetimes, or
    /// `None` if it has type or const parameters (generics, milestone 3c).
    pub fn lifetime_only_substs(&mut self, owner: GenericOwner) -> Option<Substs> {
        let generics = generics_of(self.cx.hir, owner);
        let mut args = Vec::new();
        for param in &generics.params {
            match param.kind {
                ParamKind::Lifetime => args.push(GenericArg::Region(Region::Erased)),
                _ => return None,
            }
        }
        Some(self.cx.interner.substs(args))
    }

    pub fn check_value_path(&mut self, expr: ExprId, path: &Path) -> Ty {
        let Some(res) = self.body.path_res(path) else {
            return self.cx.interner.error();
        };
        if path.segments.iter().any(|segment| segment.args.is_some()) {
            return self.not_yet(
                expr,
                "generic arguments in expression paths are not supported yet",
            );
        }
        if res.unresolved_segments > 0 {
            return self.check_assoc_path(expr, path, res);
        }
        match res.base {
            Res::Def(DefId::Fn(id)) => self.fn_item(expr, id),
            Res::Def(DefId::Const(id)) => {
                let ty = self.cx.items.const_tys[id];
                self.erase_regions(ty)
            }
            Res::Def(DefId::Static(id)) => {
                if self.cx.hir.statics[id].is_mut {
                    let range = self.expr_range(expr);
                    self.require_unsafe(range, "use of a mutable static");
                }
                let ty = self.cx.items.static_tys[id];
                self.erase_regions(ty)
            }
            Res::Def(DefId::Struct(id)) => self.struct_ctor(expr, id),
            Res::Def(DefId::Variant(id)) => self.variant_ctor(expr, id),
            Res::SelfTy(SelfTyOf::Impl(id)) => {
                // `Self` as a value: the constructor of a tuple or unit struct.
                let self_ty = self.cx.impl_self_ty(id);
                let self_ty = self.erase_regions(self_ty);
                match self.kind(self_ty) {
                    TyKind::Adt(AdtId::Struct(s), substs)
                        if !matches!(self.cx.hir.structs[s].fields, Fields::Record(_)) =>
                    {
                        self.ctor_ty(
                            CallableId::StructCtor(s),
                            &self.cx.hir.structs[s].fields.clone(),
                            self_ty,
                            substs,
                        )
                    }
                    _ => {
                        let range = self.expr_range(expr);
                        let message =
                            "`Self` is not a tuple or unit struct here, so it is not a value";
                        self.error(range, Code::WrongKind, message);
                        self.cx.interner.error()
                    }
                }
            }
            Res::GenericParam(id) => {
                // A const parameter used as a value.
                match self.cx.items.const_param_tys.get(&id) {
                    Some(&ty) => ty,
                    None => self.cx.interner.error(),
                }
            }
            // Anything else was reported during name resolution.
            _ => self.cx.interner.error(),
        }
    }

    fn fn_item(&mut self, expr: ExprId, id: FnId) -> Ty {
        match self.lifetime_only_substs(GenericOwner::Fn(id)) {
            Some(substs) => self.intern(TyKind::FnDef(CallableId::Fn(id), substs)),
            None => self.not_yet(expr, "using generic functions is not supported yet"),
        }
    }

    fn struct_ctor(&mut self, expr: ExprId, id: StructId) -> Ty {
        let Some(substs) = self.lifetime_only_substs(GenericOwner::Struct(id)) else {
            return self.not_yet(
                expr,
                "constructing values of generic types is not supported yet",
            );
        };
        let adt = self.intern(TyKind::Adt(AdtId::Struct(id), substs));
        let fields = self.cx.hir.structs[id].fields.clone();
        self.ctor_ty(CallableId::StructCtor(id), &fields, adt, substs)
    }

    fn variant_ctor(&mut self, expr: ExprId, id: VariantId) -> Ty {
        let parent = self.cx.hir.variants[id].parent;
        let Some(substs) = self.lifetime_only_substs(GenericOwner::Enum(parent)) else {
            return self.not_yet(
                expr,
                "constructing values of generic types is not supported yet",
            );
        };
        let adt = self.intern(TyKind::Adt(AdtId::Enum(parent), substs));
        let fields = self.cx.hir.variants[id].fields.clone();
        self.ctor_ty(CallableId::VariantCtor(id), &fields, adt, substs)
    }

    /// A unit struct or variant is a value of its type; a tuple one is a
    /// constructor function.
    fn ctor_ty(&mut self, callable: CallableId, fields: &Fields, adt: Ty, substs: Substs) -> Ty {
        match fields {
            Fields::Unit => adt,
            _ => self.intern(TyKind::FnDef(callable, substs)),
        }
    }

    /// `Type::item`: an associated function or constant of an inherent impl.
    fn check_assoc_path(&mut self, expr: ExprId, path: &Path, res: PathRes) -> Ty {
        if res.unresolved_segments > 1 {
            return self.not_yet(
                expr,
                "paths through several associated items are not supported yet",
            );
        }
        let name = path
            .segments
            .last()
            .expect("a partial path has segments")
            .name
            .clone();
        let self_ty = match res.base {
            Res::Def(DefId::Struct(id)) => {
                match self.lifetime_only_substs(GenericOwner::Struct(id)) {
                    Some(substs) => self.intern(TyKind::Adt(AdtId::Struct(id), substs)),
                    None => {
                        return self.not_yet(
                            expr,
                            "associated items of generic types are not supported yet",
                        );
                    }
                }
            }
            Res::Def(DefId::Enum(id)) => match self.lifetime_only_substs(GenericOwner::Enum(id)) {
                Some(substs) => self.intern(TyKind::Adt(AdtId::Enum(id), substs)),
                None => {
                    return self.not_yet(
                        expr,
                        "associated items of generic types are not supported yet",
                    );
                }
            },
            Res::Def(DefId::TypeAlias(id)) => {
                match self.lifetime_only_substs(GenericOwner::TypeAlias(id)) {
                    Some(_) => {
                        let ty = self.cx.alias_ty(id);
                        self.erase_regions(ty)
                    }
                    None => {
                        return self.not_yet(
                            expr,
                            "associated items of generic types are not supported yet",
                        );
                    }
                }
            }
            Res::SelfTy(SelfTyOf::Impl(id)) => {
                let ty = self.cx.impl_self_ty(id);
                self.erase_regions(ty)
            }
            Res::PrimTy(_) => {
                return self.not_yet(
                    expr,
                    "associated items of primitive types come with the standard library and are not supported yet",
                );
            }
            Res::GenericParam(_)
            | Res::Def(DefId::Trait(_))
            | Res::QualifiedSelf
            | Res::SelfTy(_) => {
                return self.not_yet(expr, "associated items of traits are not supported yet");
            }
            _ => return self.cx.interner.error(),
        };
        match self.inherent_item(expr, self_ty, &name, false) {
            Some(AssocItem::Fn(id)) => {
                self.results.assoc_values.insert(expr, AssocValue::Fn(id));
                self.fn_item(expr, id)
            }
            Some(AssocItem::Const(id)) => {
                self.results
                    .assoc_values
                    .insert(expr, AssocValue::Const(id));
                let ty = self.cx.items.const_tys[id];
                self.erase_regions(ty)
            }
            Some(AssocItem::TypeAlias(_)) | None => self.cx.interner.error(),
        }
    }

    /// Finds the associated function or constant `name` in the inherent impls
    /// of `self_ty`, reporting errors. With `methods_only`, only functions
    /// with a `self` parameter count.
    pub fn inherent_item(
        &mut self,
        expr: ExprId,
        self_ty: Ty,
        name: &Name,
        methods_only: bool,
    ) -> Option<AssocItem> {
        let hir = self.cx.hir;
        let simplified = self.cx.simplify(self_ty)?;
        let impls = self
            .cx
            .inherent_impls
            .get(&simplified)
            .cloned()
            .unwrap_or_default();
        for impl_id in impls {
            let impl_self = self.cx.impl_self_ty(impl_id);
            let impl_self = self.erase_regions(impl_self);
            if impl_self != self_ty {
                if self
                    .lifetime_only_substs(GenericOwner::Impl(impl_id))
                    .is_none()
                    && impl_has_item(hir, impl_id, name)
                {
                    self.not_yet(expr, "items of generic impls are not supported yet");
                    return None;
                }
                continue;
            }
            for &item in &hir.impls[impl_id].items {
                let found = match item {
                    AssocItem::Fn(id) => {
                        hir.functions[id].name == *name
                            && (!methods_only || hir.functions[id].self_param.is_some())
                    }
                    AssocItem::Const(id) => {
                        !methods_only && hir.consts[id].name.as_ref() == Some(name)
                    }
                    AssocItem::TypeAlias(_) => false,
                };
                if found {
                    self.check_item_visible(expr, item, name);
                    return Some(item);
                }
            }
        }
        if !methods_only {
            self.missing_assoc(expr, self_ty, name);
        }
        None
    }

    fn check_item_visible(&mut self, expr: ExprId, item: AssocItem, name: &Name) {
        let def = match item {
            AssocItem::Fn(id) => DefId::Fn(id),
            AssocItem::Const(id) => DefId::Const(id),
            AssocItem::TypeAlias(id) => DefId::TypeAlias(id),
        };
        let visible =
            self.cx.resolutions.def_visibilities[&def].is_accessible_from(self.cx.hir, self.module);
        if !visible {
            let range = self.expr_range(expr);
            self.error(
                range,
                Code::Private,
                format!("associated item `{name}` is private"),
            );
        }
    }

    fn missing_assoc(&mut self, expr: ExprId, self_ty: Ty, name: &Name) {
        if self.some_trait_has_item(name) {
            self.not_yet(expr, "associated items of traits are not supported yet");
            return;
        }
        let range = self.expr_range(expr);
        let message = format!(
            "no function or associated item named `{name}` found for `{}`",
            self.display(self_ty)
        );
        self.error(range, Code::Unresolved, message);
    }

    /// Whether any trait declares an item called `name`: a missing inherent
    /// item may then be a trait item, which milestone 3c handles.
    pub fn some_trait_has_item(&self, name: &Name) -> bool {
        let hir = self.cx.hir;
        hir.traits.iter().any(|(_, t)| {
            t.items
                .iter()
                .any(|&item| assoc_name(hir, item) == Some(name))
        })
    }

    pub fn check_call(&mut self, expr: ExprId, callee: ExprId, args: &[ExprId]) -> Ty {
        let callee_ty = self.check_expr(callee);
        let is_unsafe = match self.kind(callee_ty) {
            TyKind::FnDef(CallableId::Fn(id), _) => self.cx.items.fn_sigs[id].is_unsafe,
            TyKind::FnPtr(sig) => sig.is_unsafe,
            _ => false,
        };
        if is_unsafe {
            let range = self.expr_range(expr);
            self.require_unsafe(range, "a call to an `unsafe` function");
        }
        let (inputs, output) = match self.kind(callee_ty) {
            TyKind::FnDef(callable, substs) => self.callable_sig(callable, substs),
            TyKind::FnPtr(sig) => {
                let (inputs, output) = self.cx.interner.sig_parts(sig);
                (inputs.to_vec(), output)
            }
            TyKind::Error => {
                for &arg in args {
                    self.check_expr(arg);
                }
                return self.cx.interner.error();
            }
            _ => {
                let range = self.expr_range(callee);
                let message = format!("expected a function, found `{}`", self.display(callee_ty));
                self.error(range, Code::TypeMismatch, message);
                for &arg in args {
                    self.check_expr(arg);
                }
                return self.cx.interner.error();
            }
        };
        self.check_args(expr, &inputs, args);
        output
    }

    /// Checks arguments against parameter types, and their number.
    pub fn check_args(&mut self, expr: ExprId, inputs: &[Ty], args: &[ExprId]) {
        if inputs.len() != args.len() {
            let range = self.expr_range(expr);
            let message = format!(
                "this function takes {} argument{} but {} were supplied",
                inputs.len(),
                if inputs.len() == 1 { "" } else { "s" },
                args.len()
            );
            self.error(range, Code::ArgCount, message);
        }
        for (i, &arg) in args.iter().enumerate() {
            match inputs.get(i) {
                Some(&input) => {
                    self.check_expr_coercible(arg, input);
                }
                None => {
                    self.check_expr(arg);
                }
            }
        }
    }

    /// The parameter and return types of a function or constructor.
    pub fn callable_sig(&mut self, callable: CallableId, substs: Substs) -> (Vec<Ty>, Ty) {
        let hir = self.cx.hir;
        let (field_owner, adt) = match callable {
            CallableId::Fn(id) => {
                let sig = self.cx.items.fn_sigs[id];
                let all = self.cx.interner.tys(sig.inputs_and_output).to_vec();
                let all: Vec<Ty> = all
                    .into_iter()
                    .map(|ty| {
                        let ty = subst(&mut self.cx.interner, ty, substs);
                        self.erase_regions(ty)
                    })
                    .collect();
                let (output, inputs) = all.split_last().expect("a signature has a return type");
                return (inputs.to_vec(), *output);
            }
            CallableId::StructCtor(id) => (FieldOwner::Struct(id), AdtId::Struct(id)),
            CallableId::VariantCtor(id) => (
                FieldOwner::Variant(id),
                AdtId::Enum(hir.variants[id].parent),
            ),
        };
        let fields = self.cx.items.fields[&field_owner].clone();
        let inputs = fields
            .into_iter()
            .map(|ty| {
                let ty = subst(&mut self.cx.interner, ty, substs);
                self.erase_regions(ty)
            })
            .collect();
        (inputs, self.intern(TyKind::Adt(adt, substs)))
    }
}

fn impl_has_item(hir: &bluing_hir::Hir, impl_id: bluing_hir::ImplId, name: &Name) -> bool {
    hir.impls[impl_id]
        .items
        .iter()
        .any(|&item| assoc_name(hir, item) == Some(name))
}

fn assoc_name(hir: &bluing_hir::Hir, item: AssocItem) -> Option<&Name> {
    match item {
        AssocItem::Fn(id) => Some(&hir.functions[id].name),
        AssocItem::Const(id) => hir.consts[id].name.as_ref(),
        AssocItem::TypeAlias(id) => Some(&hir.type_aliases[id].name),
    }
}
