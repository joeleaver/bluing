//! Typed ids for everything the HIR stores. Ids are indices into the arenas of
//! one [`crate::Hir`], shared by all crates of a compilation.

use la_arena::Idx;

use crate::crate_graph::CrateData;
use crate::hir::{
    Const, Enum, Function, Impl, Import, Module, Static, Struct, Trait, TypeAlias, Variant,
};

pub type CrateId = Idx<CrateData>;
pub type ModuleId = Idx<Module>;
pub type StructId = Idx<Struct>;
pub type EnumId = Idx<Enum>;
pub type VariantId = Idx<Variant>;
pub type FnId = Idx<Function>;
pub type TraitId = Idx<Trait>;
pub type ImplId = Idx<Impl>;
pub type TypeAliasId = Idx<TypeAlias>;
pub type ConstId = Idx<Const>;
pub type StaticId = Idx<Static>;
pub type ImportId = Idx<Import>;

/// A definition that can be named by a path.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DefId {
    Module(ModuleId),
    Struct(StructId),
    Enum(EnumId),
    Variant(VariantId),
    Fn(FnId),
    Trait(TraitId),
    TypeAlias(TypeAliasId),
    Const(ConstId),
    Static(StaticId),
}

/// An item that declares generic parameters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum GenericOwner {
    Struct(StructId),
    Enum(EnumId),
    Fn(FnId),
    Trait(TraitId),
    Impl(ImplId),
    TypeAlias(TypeAliasId),
}

/// One generic parameter: the `index`th parameter of `owner`, counting
/// lifetime, type and const parameters together in declaration order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GenericParamId {
    pub owner: GenericOwner,
    pub index: u32,
}

/// Identifies one path written in a signature; resolutions are keyed by it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PathId(pub u32);

/// Identifies one lifetime written in a signature.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct LifetimeId(pub u32);
