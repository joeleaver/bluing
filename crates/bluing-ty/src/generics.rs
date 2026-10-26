//! The generic parameters of each item, numbered the way types refer to them:
//! the parent item's parameters first (the impl or trait of a method), then a
//! trait's implicit `Self`, then the item's own.

use bluing_hir::hir::{Container, GenericParamKind, GenericParams};
use bluing_hir::{GenericOwner, GenericParamId, Hir, Name};

use crate::ty::ParamIndex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamKind {
    Lifetime,
    Type { has_default: bool },
    Const { has_default: bool },
}

#[derive(Clone, Debug)]
pub struct ParamDef {
    pub name: Name,
    pub kind: ParamKind,
}

/// The generic parameters in scope for an item.
#[derive(Clone, Debug)]
pub struct Generics {
    /// Every parameter in index order: the parent's, `Self`, the item's own.
    pub params: Vec<ParamDef>,
    /// Where the item's own parameters start.
    pub own_start: u32,
}

impl Generics {
    pub fn own(&self) -> &[ParamDef] {
        &self.params[self.own_start as usize..]
    }

    pub fn name(&self, index: ParamIndex) -> Option<&Name> {
        self.params.get(index.0 as usize).map(|param| &param.name)
    }
}

/// The parent of an item's generics, if it has one.
pub fn parent(hir: &Hir, owner: GenericOwner) -> Option<GenericOwner> {
    let container = match owner {
        GenericOwner::Fn(id) => hir.functions[id].container,
        GenericOwner::TypeAlias(id) => hir.type_aliases[id].container,
        GenericOwner::Struct(_)
        | GenericOwner::Enum(_)
        | GenericOwner::Trait(_)
        | GenericOwner::Impl(_) => {
            return None;
        }
    };
    match container {
        Container::Module(_) => None,
        Container::Trait(id) => Some(GenericOwner::Trait(id)),
        Container::Impl(id) => Some(GenericOwner::Impl(id)),
    }
}

/// The item's own parameter list in the HIR.
pub fn hir_params(hir: &Hir, owner: GenericOwner) -> &GenericParams {
    match owner {
        GenericOwner::Struct(id) => &hir.structs[id].generics,
        GenericOwner::Enum(id) => &hir.enums[id].generics,
        GenericOwner::Fn(id) => &hir.functions[id].generics,
        GenericOwner::Trait(id) => &hir.traits[id].generics,
        GenericOwner::Impl(id) => &hir.impls[id].generics,
        GenericOwner::TypeAlias(id) => &hir.type_aliases[id].generics,
    }
}

pub fn generics_of(hir: &Hir, owner: GenericOwner) -> Generics {
    let mut params = match parent(hir, owner) {
        Some(parent) => generics_of(hir, parent).params,
        None => Vec::new(),
    };
    if matches!(owner, GenericOwner::Trait(_)) {
        params.push(ParamDef {
            name: Name::new("Self"),
            kind: ParamKind::Type { has_default: false },
        });
    }
    let own_start = params.len() as u32;
    params.extend(hir_params(hir, owner).params.iter().map(|param| ParamDef {
        name: param.name.clone(),
        kind: match &param.kind {
            GenericParamKind::Lifetime { .. } => ParamKind::Lifetime,
            GenericParamKind::Type { default, .. } => ParamKind::Type {
                has_default: default.is_some(),
            },
            GenericParamKind::Const { default, .. } => ParamKind::Const {
                has_default: default.is_some(),
            },
        },
    }));
    Generics { params, own_start }
}

/// The index of a parameter among all the parameters in scope of its owner.
pub fn param_index(hir: &Hir, id: GenericParamId) -> ParamIndex {
    ParamIndex(generics_of(hir, id.owner).own_start + id.index)
}
