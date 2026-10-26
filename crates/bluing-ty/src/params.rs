//! Checks on generic parameters: the types of const parameters, defaults,
//! and parameters an item never uses.

use bluing_base::{Code, FileId};
use bluing_hir::hir::GenericParamKind;
use bluing_hir::resolve::FieldOwner;
use bluing_hir::{GenericOwner, GenericParamId};

use crate::TyCx;
use crate::collect::Lazy;
use crate::generics::{generics_of, hir_params};
use crate::lower::RegionMode;
use crate::ty::{GenericArg, ParamIndex, Region, Ty, TyKind};

impl TyCx<'_> {
    /// Lowers the types of const parameters, which may only be integers,
    /// `bool` or `char`.
    pub(crate) fn collect_const_params(&mut self, owner: GenericOwner) {
        let hir = self.hir;
        let generics = generics_of(hir, owner);
        let file = self.owner_file(owner);
        for (index, param) in hir_params(hir, owner).params.iter().enumerate() {
            let GenericParamKind::Const { ty, .. } = &param.kind else {
                continue;
            };
            let mut cx = self.signature_ctx(
                file,
                &generics,
                RegionMode::Forbidden,
                "const parameter types",
            );
            let lowered = self.lower_ty(&mut cx, ty);
            let allowed = matches!(
                self.interner.kind(lowered),
                TyKind::Int(_) | TyKind::Uint(_) | TyKind::Bool | TyKind::Char | TyKind::Error
            );
            if !allowed {
                let message = format!(
                    "`{}` is not allowed as the type of a const parameter: only integers, `bool` and `char` are",
                    self.display(lowered, Some(&generics))
                );
                self.error(file, ty.range, Code::WrongKind, message);
            }
            let id = GenericParamId {
                owner,
                index: index as u32,
            };
            self.items.const_param_tys.insert(id, lowered);
        }
    }

    /// Type parameter defaults are only allowed on structs, enums, type
    /// aliases and traits. Lowering each default reports its errors once.
    pub(crate) fn check_defaults(&mut self, owner: GenericOwner) {
        let hir = self.hir;
        let file = self.owner_file(owner);
        let allows_defaults = matches!(
            owner,
            GenericOwner::Struct(_)
                | GenericOwner::Enum(_)
                | GenericOwner::TypeAlias(_)
                | GenericOwner::Trait(_)
        );
        for (local, param) in hir_params(hir, owner).params.iter().enumerate() {
            match &param.kind {
                GenericParamKind::Type {
                    default: Some(default),
                    ..
                } if !allows_defaults => {
                    let message = "defaults for type parameters are only allowed on structs, enums, \
                                   type aliases and traits";
                    self.error(file, default.range, Code::GenericArgs, message);
                }
                GenericParamKind::Type {
                    default: Some(_), ..
                }
                | GenericParamKind::Const {
                    default: Some(_), ..
                } => {
                    self.param_default_arg(GenericParamId {
                        owner,
                        index: local as u32,
                    });
                }
                _ => {}
            }
        }
    }

    /// The default of a type or const parameter, in terms of the owner's
    /// parameters, lowered once. A default may only refer to earlier
    /// parameters.
    pub(crate) fn param_default_arg(&mut self, id: GenericParamId) -> Option<GenericArg> {
        let hir = self.hir;
        let owner = id.owner;
        let file = self.owner_file(owner);
        let param = &hir_params(hir, owner).params[id.index as usize];
        match self.default_state.get(&id).copied() {
            Some(Lazy::Done(arg)) => return Some(arg),
            Some(Lazy::InProgress) => {
                let message = format!("the default of `{}` depends on itself", param.name);
                self.error(file, param.range, Code::Cycle, message);
                let error = GenericArg::Ty(self.interner.error());
                self.default_state.insert(id, Lazy::Done(error));
                return Some(error);
            }
            None => {}
        }
        self.default_state.insert(id, Lazy::InProgress);
        let generics = generics_of(hir, owner);
        let mut cx = self.signature_ctx(
            file,
            &generics,
            RegionMode::Forbidden,
            "generic parameter defaults",
        );
        let arg = match &param.kind {
            GenericParamKind::Type {
                default: Some(default),
                ..
            } => {
                let ty = self.lower_ty(&mut cx, default);
                let index = generics.own_start as usize + id.index as usize;
                if self.mentions_param_from(ty, index) {
                    let message = "a generic parameter's default can only use earlier parameters";
                    self.error(file, default.range, Code::GenericArgs, message);
                    GenericArg::Ty(self.interner.error())
                } else {
                    GenericArg::Ty(ty)
                }
            }
            GenericParamKind::Const {
                default: Some(default),
                ..
            } => {
                let expected = self.const_param_ty(owner, id.index as usize);
                GenericArg::Const(self.lower_const_expr(&mut cx, default, expected))
            }
            _ => {
                self.default_state.remove(&id);
                return None;
            }
        };
        // A cycle through this default already recorded an error.
        if let Some(Lazy::Done(error)) = self.default_state.get(&id).copied() {
            return Some(error);
        }
        self.default_state.insert(id, Lazy::Done(arg));
        self.items.param_defaults.insert(id, arg);
        Some(arg)
    }

    /// Every type and lifetime parameter of a struct, enum or type alias must
    /// be used (const parameters may go unused).
    pub(crate) fn check_params_used(&mut self, owner: GenericOwner, file: FileId) {
        let hir = self.hir;
        let generics = generics_of(hir, owner);
        let mut used = vec![false; generics.params.len()];
        let tys: Vec<Ty> = match owner {
            GenericOwner::Struct(id) => self.items.fields[&FieldOwner::Struct(id)].clone(),
            GenericOwner::Enum(id) => hir.enums[id]
                .variants
                .iter()
                .flat_map(|&v| self.items.fields[&FieldOwner::Variant(v)].clone())
                .collect(),
            GenericOwner::TypeAlias(id) => vec![self.alias_ty(id)],
            _ => return,
        };
        for ty in tys {
            self.mark_params(ty, &mut used);
        }
        let is_alias = matches!(owner, GenericOwner::TypeAlias(_));
        for (index, param) in hir_params(hir, owner).params.iter().enumerate() {
            let what = match param.kind {
                GenericParamKind::Type { .. } => "type",
                // Rust lets type aliases leave lifetimes unused.
                GenericParamKind::Lifetime { .. } if !is_alias => "lifetime",
                _ => continue,
            };
            if !used[generics.own_start as usize + index] {
                let name = if what == "lifetime" {
                    format!("'{}", param.name)
                } else {
                    param.name.to_string()
                };
                let message = format!("{what} parameter `{name}` is never used");
                self.error(file, param.range, Code::UnusedParam, message);
            }
        }
    }

    /// Marks every parameter `ty` mentions.
    fn mark_params(&self, ty: Ty, used: &mut [bool]) {
        let mark_region = |region: Region, used: &mut [bool]| {
            if let Region::Param(ParamIndex(index)) = region
                && let Some(slot) = used.get_mut(index as usize)
            {
                *slot = true;
            }
        };
        match self.interner.kind(ty) {
            TyKind::Param(ParamIndex(index)) => {
                if let Some(slot) = used.get_mut(*index as usize) {
                    *slot = true;
                }
            }
            TyKind::Adt(_, substs) | TyKind::FnDef(_, substs) => {
                for arg in self.interner.args(*substs) {
                    match arg {
                        GenericArg::Ty(ty) => self.mark_params(*ty, used),
                        GenericArg::Region(region) => mark_region(*region, used),
                        GenericArg::Const(_) => {}
                    }
                }
            }
            TyKind::Ref(region, inner, _) => {
                mark_region(*region, used);
                self.mark_params(*inner, used);
            }
            TyKind::RawPtr(inner, _) | TyKind::Slice(inner) | TyKind::Array(inner, _) => {
                self.mark_params(*inner, used);
            }
            TyKind::Tuple(list) => {
                for &ty in self.interner.tys(*list) {
                    self.mark_params(ty, used);
                }
            }
            TyKind::FnPtr(sig) => {
                for &ty in self.interner.tys(sig.inputs_and_output) {
                    self.mark_params(ty, used);
                }
            }
            _ => {}
        }
    }
}
