//! Impl headers: which types inherent impls may extend, the index of
//! inherent impls used by method lookup, and that every impl parameter is
//! constrained.

use bluing_base::Code;
use bluing_hir::resolve::Res;
use bluing_hir::{CrateKind, DefId, GenericOwner, ImplId, TraitId};

use crate::TyCx;
use crate::generics::{ParamKind, generics_of, hir_params};
use crate::lower::RegionMode;
use crate::ty::{AdtId, Const, FloatTy, GenericArg, IntTy, ParamIndex, Substs, Ty, TyKind, UintTy};

/// A type reduced to what selects its inherent impls.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SimplifiedType {
    Adt(AdtId),
    Bool,
    Char,
    Str,
    Int(IntTy),
    Uint(UintTy),
    Float(FloatTy),
    Slice,
    Array,
    Ref,
    RawPtr,
    Tuple,
    Never,
    FnPtr,
}

/// A trait applied to arguments, the first of which is the `Self` type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraitRef {
    pub trait_id: TraitId,
    pub substs: Substs,
}

impl TyCx<'_> {
    pub fn simplify(&self, ty: Ty) -> Option<SimplifiedType> {
        Some(match self.interner.kind(ty) {
            TyKind::Adt(adt, _) => SimplifiedType::Adt(*adt),
            TyKind::Bool => SimplifiedType::Bool,
            TyKind::Char => SimplifiedType::Char,
            TyKind::Str => SimplifiedType::Str,
            TyKind::Int(int) => SimplifiedType::Int(*int),
            TyKind::Uint(uint) => SimplifiedType::Uint(*uint),
            TyKind::Float(float) => SimplifiedType::Float(*float),
            TyKind::Slice(_) => SimplifiedType::Slice,
            TyKind::Array(..) => SimplifiedType::Array,
            TyKind::Ref(..) => SimplifiedType::Ref,
            TyKind::RawPtr(..) => SimplifiedType::RawPtr,
            TyKind::Tuple(_) => SimplifiedType::Tuple,
            TyKind::Never => SimplifiedType::Never,
            TyKind::FnPtr(_) => SimplifiedType::FnPtr,
            TyKind::FnDef(..) | TyKind::Param(_) | TyKind::Infer(_) | TyKind::Error => return None,
        })
    }

    pub(crate) fn check_impls(&mut self) {
        let hir = self.hir;
        for (id, imp) in hir.impls.iter() {
            let self_ty = self.impl_self_ty(id);
            let trait_ref = imp
                .trait_ref
                .as_ref()
                .and_then(|path| self.lower_trait_ref(id, path, self_ty));
            if let Some(trait_ref) = trait_ref {
                self.impl_trait_refs.insert(id, trait_ref);
            }
            self.check_constrained(id, self_ty, trait_ref);
            if imp.trait_ref.is_none() {
                self.register_inherent(id, self_ty);
            }
        }
    }

    fn lower_trait_ref(
        &mut self,
        id: ImplId,
        path: &bluing_hir::hir::Path,
        self_ty: Ty,
    ) -> Option<TraitRef> {
        let hir = self.hir;
        let res = self.resolutions.path(path.id)?;
        let Res::Def(DefId::Trait(trait_id)) = res.base else {
            return None;
        };
        let imp = &hir.impls[id];
        let generics = generics_of(hir, GenericOwner::Impl(id));
        let mut cx = self.signature_ctx(
            imp.loc.file,
            &generics,
            RegionMode::Fresh,
            "implementations",
        );
        cx.self_ty = Some(self_ty);
        let last = path.segments.last()?;
        let substs = self.generic_args(
            &mut cx,
            GenericOwner::Trait(trait_id),
            last.args.as_ref(),
            last.range,
            vec![GenericArg::Ty(self_ty)],
        );
        Some(TraitRef { trait_id, substs })
    }

    /// Inherent impls may only extend types of the same crate; primitive
    /// types only in the standard library.
    fn register_inherent(&mut self, id: ImplId, self_ty: Ty) {
        let hir = self.hir;
        let imp = &hir.impls[id];
        let krate = hir.modules[imp.module].krate;
        let is_library = self.graph[krate].kind == CrateKind::Library;
        let problem = match self.interner.kind(self_ty) {
            TyKind::Error => return,
            TyKind::Adt(adt, _) => {
                let module = match adt {
                    AdtId::Struct(s) => hir.structs[*s].module,
                    AdtId::Enum(e) => hir.enums[*e].module,
                };
                (hir.modules[module].krate != krate)
                    .then_some("cannot define an inherent `impl` for a type from another crate")
            }
            TyKind::Param(_) => Some("cannot define an inherent `impl` for a type parameter"),
            _ => (!is_library).then_some("cannot define an inherent `impl` for a primitive type"),
        };
        if let Some(message) = problem {
            self.error(imp.loc.file, imp.loc.range, Code::Coherence, message);
            return;
        }
        if let Some(simplified) = self.simplify(self_ty) {
            self.inherent_impls.entry(simplified).or_default().push(id);
        }
    }

    /// Every type and const parameter of an impl must appear in its self type
    /// or trait reference.
    fn check_constrained(&mut self, id: ImplId, self_ty: Ty, trait_ref: Option<TraitRef>) {
        let hir = self.hir;
        let generics = generics_of(hir, GenericOwner::Impl(id));
        let mut used = vec![false; generics.params.len()];
        self.mark_constrained(self_ty, &mut used);
        if let Some(trait_ref) = trait_ref {
            for arg in self.interner.args(trait_ref.substs).to_vec() {
                match arg {
                    GenericArg::Ty(ty) => self.mark_constrained(ty, &mut used),
                    GenericArg::Const(Const::Param(ParamIndex(index))) => {
                        used[index as usize] = true
                    }
                    _ => {}
                }
            }
        }
        let imp = &hir.impls[id];
        for (local, param) in generics.own().iter().enumerate() {
            let what = match param.kind {
                ParamKind::Type { .. } => "type",
                ParamKind::Const { .. } => "const",
                ParamKind::Lifetime => continue,
            };
            if !used[generics.own_start as usize + local] {
                let range = hir_params(hir, GenericOwner::Impl(id)).params[local].range;
                let message = format!(
                    "the {what} parameter `{}` is not constrained by the impl's trait or self type",
                    param.name
                );
                self.error(imp.loc.file, range, Code::UnusedParam, message);
            }
        }
    }

    fn mark_constrained(&self, ty: Ty, used: &mut [bool]) {
        match self.interner.kind(ty) {
            TyKind::Param(ParamIndex(index)) => used[*index as usize] = true,
            TyKind::Adt(_, substs) => {
                for arg in self.interner.args(*substs) {
                    match arg {
                        GenericArg::Ty(ty) => self.mark_constrained(*ty, used),
                        GenericArg::Const(Const::Param(ParamIndex(index))) => {
                            used[*index as usize] = true
                        }
                        _ => {}
                    }
                }
            }
            TyKind::Ref(_, inner, _) | TyKind::RawPtr(inner, _) | TyKind::Slice(inner) => {
                self.mark_constrained(*inner, used);
            }
            TyKind::Array(elem, len) => {
                self.mark_constrained(*elem, used);
                if let Const::Param(ParamIndex(index)) = len {
                    used[*index as usize] = true;
                }
            }
            TyKind::Tuple(list) => {
                for &ty in self.interner.tys(*list) {
                    self.mark_constrained(ty, used);
                }
            }
            TyKind::FnPtr(sig) => {
                for &ty in self.interner.tys(sig.inputs_and_output) {
                    self.mark_constrained(ty, used);
                }
            }
            _ => {}
        }
    }
}
