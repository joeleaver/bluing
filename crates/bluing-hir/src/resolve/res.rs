//! What paths and lifetimes resolve to.

use crate::{DefId, EnumId, GenericParamId, ImplId, Name, StructId, TraitId};

/// The resolution of a path, or of its first part.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Res {
    Def(DefId),
    PrimTy(PrimTy),
    /// `Self` inside a struct or enum definition, a trait, or an impl.
    SelfTy(SelfTyOf),
    GenericParam(GenericParamId),
    /// The `<T as Trait>` of a qualified path; type checking resolves the
    /// rest.
    QualifiedSelf,
    /// The path did not resolve; a diagnostic was reported.
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfTyOf {
    Struct(StructId),
    Enum(EnumId),
    Trait(TraitId),
    Impl(ImplId),
}

/// A path resolved as far as the module system can go. When
/// `unresolved_segments` is not zero, the last that many segments name
/// associated items (`T::Item`, `Self::Output`) for type checking to resolve.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PathRes {
    pub base: Res,
    pub unresolved_segments: usize,
}

impl PathRes {
    pub fn full(base: Res) -> Self {
        PathRes {
            base,
            unresolved_segments: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LifetimeRes {
    Param(GenericParamId),
    Static,
    /// `'_`
    Anonymous,
    /// The lifetime is not declared; a diagnostic was reported.
    Error,
}

/// The primitive types, which are in scope everywhere unless an item of the
/// same name shadows them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PrimTy {
    Bool,
    Char,
    Str,
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
    F32,
    F64,
}

impl PrimTy {
    pub fn from_name(name: &Name) -> Option<PrimTy> {
        Some(match name.as_str() {
            "bool" => PrimTy::Bool,
            "char" => PrimTy::Char,
            "str" => PrimTy::Str,
            "i8" => PrimTy::I8,
            "i16" => PrimTy::I16,
            "i32" => PrimTy::I32,
            "i64" => PrimTy::I64,
            "i128" => PrimTy::I128,
            "isize" => PrimTy::Isize,
            "u8" => PrimTy::U8,
            "u16" => PrimTy::U16,
            "u32" => PrimTy::U32,
            "u64" => PrimTy::U64,
            "u128" => PrimTy::U128,
            "usize" => PrimTy::Usize,
            "f32" => PrimTy::F32,
            "f64" => PrimTy::F64,
            _ => return None,
        })
    }
}
