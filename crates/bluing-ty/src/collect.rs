//! Collects the type of every item's signature.

use std::collections::BTreeMap;

use bluing_base::{Code, FileId};
use bluing_hir::hir::{Container, Fields, SelfParamKind};
use bluing_hir::resolve::FieldOwner;
use bluing_hir::{ConstId, FnId, GenericOwner, GenericParamId, ImplId, StaticId, TypeAliasId};
use la_arena::ArenaMap;

use crate::TyCx;
use crate::generics::generics_of;
use crate::lower::{LowerCtx, PathSource, Placeholder, RegionMode, single_region};
use crate::ty::{AdtId, FnSig, GenericArg, Mutability, ParamIndex, Ty, TyKind};

/// The types in every item's signature.
#[derive(Debug, Default)]
pub struct ItemTypes {
    pub fields: BTreeMap<FieldOwner, Vec<Ty>>,
    pub fn_sigs: ArenaMap<FnId, FnSig>,
    pub impl_self_tys: ArenaMap<ImplId, Ty>,
    pub alias_tys: ArenaMap<TypeAliasId, Ty>,
    pub const_tys: ArenaMap<ConstId, Ty>,
    pub static_tys: ArenaMap<StaticId, Ty>,
    pub const_param_tys: BTreeMap<GenericParamId, Ty>,
    /// The defaults of type and const parameters, in terms of the owner's
    /// parameters.
    pub param_defaults: BTreeMap<GenericParamId, GenericArg>,
}

/// Progress of lowering something computed on demand (a type alias, a
/// parameter default), to detect cycles.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Lazy<T> {
    InProgress,
    Done(T),
}

pub(crate) type AliasState = Lazy<Ty>;

impl TyCx<'_> {
    pub(crate) fn collect_items(&mut self) {
        let hir = self.hir;
        let owners: Vec<GenericOwner> = hir
            .structs
            .iter()
            .map(|(id, _)| GenericOwner::Struct(id))
            .chain(hir.enums.iter().map(|(id, _)| GenericOwner::Enum(id)))
            .chain(hir.functions.iter().map(|(id, _)| GenericOwner::Fn(id)))
            .chain(hir.traits.iter().map(|(id, _)| GenericOwner::Trait(id)))
            .chain(hir.impls.iter().map(|(id, _)| GenericOwner::Impl(id)))
            .chain(
                hir.type_aliases
                    .iter()
                    .map(|(id, _)| GenericOwner::TypeAlias(id)),
            )
            .collect();
        for &owner in &owners {
            self.collect_const_params(owner);
        }
        for &owner in &owners {
            self.check_defaults(owner);
        }
        for (id, _) in hir.impls.iter() {
            self.impl_self_ty(id);
        }
        for (id, s) in hir.structs.iter() {
            self.collect_fields(GenericOwner::Struct(id), FieldOwner::Struct(id), &s.fields);
        }
        for (id, e) in hir.enums.iter() {
            for &variant in &e.variants {
                let fields = &hir.variants[variant].fields;
                self.collect_fields(GenericOwner::Enum(id), FieldOwner::Variant(variant), fields);
            }
        }
        for (id, s) in hir.structs.iter() {
            self.check_params_used(GenericOwner::Struct(id), s.loc.file);
        }
        for (id, e) in hir.enums.iter() {
            self.check_params_used(GenericOwner::Enum(id), e.loc.file);
        }
        for (id, _) in hir.functions.iter() {
            self.collect_fn_sig(id);
        }
        for (id, c) in hir.consts.iter() {
            let ty = self.collect_value_ty(c.container, c.loc.file, &c.ty, "constants");
            self.items.const_tys.insert(id, ty);
        }
        for (id, s) in hir.statics.iter() {
            let ty =
                self.collect_value_ty(Container::Module(s.module), s.loc.file, &s.ty, "statics");
            self.items.static_tys.insert(id, ty);
        }
        for (id, alias) in hir.type_aliases.iter() {
            if matches!(alias.container, Container::Module(_)) {
                self.alias_ty(id);
                self.check_params_used(GenericOwner::TypeAlias(id), alias.loc.file);
            }
        }
    }

    fn collect_fields(&mut self, adt: GenericOwner, owner: FieldOwner, fields: &Fields) {
        let fields = match fields {
            Fields::Record(fields) | Fields::Tuple(fields) => fields.as_slice(),
            Fields::Unit => &[],
        };
        let generics = generics_of(self.hir, adt);
        let file = self.owner_file(adt);
        let tys = fields
            .iter()
            .map(|field| {
                let mut cx =
                    self.signature_ctx(file, &generics, RegionMode::Forbidden, "structs and enums");
                self.lower_ty(&mut cx, &field.ty)
            })
            .collect();
        self.items.fields.insert(owner, tys);
    }

    fn collect_fn_sig(&mut self, id: FnId) {
        let hir = self.hir;
        let f = &hir.functions[id];
        let generics = generics_of(hir, GenericOwner::Fn(id));
        let mut cx = self.signature_ctx(f.loc.file, &generics, RegionMode::FnInputs, "functions");
        cx.self_ty = self.container_self_ty(f.container);
        let mut tys = Vec::new();
        let mut self_region = None;
        if let Some(param) = &f.self_param {
            let self_ty = cx.self_ty.unwrap_or_else(|| self.interner.error());
            let ty = match &param.kind {
                SelfParamKind::Value { .. } => self_ty,
                SelfParamKind::Ref { lifetime, mutable } => {
                    let region = self.region(&mut cx, lifetime.as_ref(), param.range);
                    self_region = Some(region);
                    let mutability = if *mutable {
                        Mutability::Mut
                    } else {
                        Mutability::Not
                    };
                    self.interner
                        .intern(TyKind::Ref(region, self_ty, mutability))
                }
                SelfParamKind::Typed { ty, .. } => {
                    let lowered = self.lower_ty(&mut cx, ty);
                    if let TyKind::Ref(region, _, _) = self.interner.kind(lowered) {
                        self_region = Some(*region);
                    }
                    lowered
                }
            };
            tys.push(ty);
        }
        for param in &f.params {
            tys.push(self.lower_ty(&mut cx, &param.ty));
        }
        // The elision rules: the one lifetime among the parameters, or else
        // the lifetime of a `&self` parameter.
        let elided = single_region(&cx.input_regions).or(self_region);
        cx.regions = RegionMode::FnOutput(elided);
        let output = self.lower_ret_ty(&mut cx, f.ret.as_ref());
        tys.push(output);
        let sig = FnSig {
            inputs_and_output: self.interner.ty_list(tys),
            is_unsafe: f.is_unsafe,
        };
        self.items.fn_sigs.insert(id, sig);
    }

    fn collect_value_ty(
        &mut self,
        container: Container,
        file: FileId,
        ty: &bluing_hir::hir::TypeRef,
        what: &'static str,
    ) -> Ty {
        let generics = match container {
            Container::Module(_) => None,
            Container::Trait(id) => Some(generics_of(self.hir, GenericOwner::Trait(id))),
            Container::Impl(id) => Some(generics_of(self.hir, GenericOwner::Impl(id))),
        };
        let resolutions = self.resolutions;
        let mut cx = LowerCtx::new(
            file,
            PathSource::Signature(resolutions),
            generics.as_ref(),
            RegionMode::Static,
            Placeholder::Forbidden(what),
        );
        cx.self_ty = self.container_self_ty(container);
        self.lower_ty(&mut cx, ty)
    }

    /// The type `Self` stands for inside items of `container`.
    fn container_self_ty(&mut self, container: Container) -> Option<Ty> {
        match container {
            Container::Module(_) => None,
            Container::Trait(_) => Some(self.interner.intern(TyKind::Param(ParamIndex(0)))),
            Container::Impl(id) => Some(self.impl_self_ty(id)),
        }
    }

    /// The self type of an impl, lowered once.
    pub fn impl_self_ty(&mut self, id: ImplId) -> Ty {
        if let Some(&ty) = self.items.impl_self_tys.get(id) {
            return ty;
        }
        let hir = self.hir;
        let imp = &hir.impls[id];
        let generics = generics_of(hir, GenericOwner::Impl(id));
        let mut cx = self.signature_ctx(
            imp.loc.file,
            &generics,
            RegionMode::Fresh,
            "implementations",
        );
        let ty = self.lower_ty(&mut cx, &imp.self_ty);
        self.items.impl_self_tys.insert(id, ty);
        ty
    }

    /// The type a module-level type alias expands to, lowered once.
    pub fn alias_ty(&mut self, id: TypeAliasId) -> Ty {
        match self.alias_state.get(id).copied() {
            Some(Lazy::Done(ty)) => return ty,
            Some(Lazy::InProgress) => {
                let alias = &self.hir.type_aliases[id];
                let message = format!("type alias `{}` expands to itself", alias.name);
                self.error(alias.loc.file, alias.loc.range, Code::Cycle, message);
                let error = self.interner.error();
                self.alias_state.insert(id, Lazy::Done(error));
                return error;
            }
            None => {}
        }
        self.alias_state.insert(id, Lazy::InProgress);
        let hir = self.hir;
        let alias = &hir.type_aliases[id];
        let generics = generics_of(hir, GenericOwner::TypeAlias(id));
        let ty = match &alias.ty {
            Some(ty) => {
                let mut cx = self.signature_ctx(
                    alias.loc.file,
                    &generics,
                    RegionMode::Forbidden,
                    "type aliases",
                );
                self.lower_ty(&mut cx, ty)
            }
            None => self.interner.error(),
        };
        // A cycle through this alias already recorded an error type.
        if let Some(Lazy::Done(error)) = self.alias_state.get(id).copied() {
            return error;
        }
        self.alias_state.insert(id, Lazy::Done(ty));
        self.items.alias_tys.insert(id, ty);
        ty
    }

    /// A context for lowering a signature: paths from name resolution, `_`
    /// forbidden.
    pub(crate) fn signature_ctx<'g>(
        &self,
        file: FileId,
        generics: &'g crate::generics::Generics,
        regions: RegionMode,
        what: &'static str,
    ) -> LowerCtx<'g>
    where
        Self: 'g,
    {
        LowerCtx::new(
            file,
            PathSource::Signature(self.resolutions),
            Some(generics),
            regions,
            Placeholder::Forbidden(what),
        )
    }

    pub fn owner_file(&self, owner: GenericOwner) -> FileId {
        let hir = self.hir;
        match owner {
            GenericOwner::Struct(id) => hir.structs[id].loc.file,
            GenericOwner::Enum(id) => hir.enums[id].loc.file,
            GenericOwner::Fn(id) => hir.functions[id].loc.file,
            GenericOwner::Trait(id) => hir.traits[id].loc.file,
            GenericOwner::Impl(id) => hir.impls[id].loc.file,
            GenericOwner::TypeAlias(id) => hir.type_aliases[id].loc.file,
        }
    }

    pub fn owner_name(&self, owner: GenericOwner) -> String {
        let hir = self.hir;
        match owner {
            GenericOwner::Struct(id) => hir.structs[id].name.to_string(),
            GenericOwner::Enum(id) => hir.enums[id].name.to_string(),
            GenericOwner::Fn(id) => hir.functions[id].name.to_string(),
            GenericOwner::Trait(id) => hir.traits[id].name.to_string(),
            GenericOwner::Impl(_) => "impl".to_owned(),
            GenericOwner::TypeAlias(id) => hir.type_aliases[id].name.to_string(),
        }
    }

    pub fn adt_name(&self, adt: AdtId) -> String {
        match adt {
            AdtId::Struct(id) => self.hir.structs[id].name.to_string(),
            AdtId::Enum(id) => self.hir.enums[id].name.to_string(),
        }
    }
}
