//! Items as lowered from syntax: names, visibilities, generics and
//! signatures. Bodies are kept as syntax pointers for a later milestone.

use bluing_base::{FileId, TextRange};
use bluing_syntax::ra_ap_syntax::{AstPtr, ast};

use super::types::{ConstExprPtr, InFile, LifetimeRef, Path, PathKind, TypeBound, TypeRef};
use crate::{
    ConstId, EnumId, FnId, ImplId, ImportId, ModuleId, Name, StaticId, StructId, TraitId,
    TypeAliasId, VariantId,
};

/// Where an item was written: its file and the range of its name (or of the
/// whole item when it has no name).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loc {
    pub file: FileId,
    pub range: TextRange,
}

/// A visibility as written, before it is resolved against the module tree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RawVisibility {
    /// No qualifier: private to the containing module.
    Private,
    Public,
    /// `pub(crate)`
    Crate,
    /// `pub(super)`
    Super,
    /// `pub(self)`
    SelfModule,
    /// `pub(in path)`
    In(ModPath),
}

/// A path that only goes through modules: `use` paths and `pub(in ...)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModPath {
    pub kind: PathKind,
    pub segments: Vec<(Name, TextRange)>,
    pub range: TextRange,
}

/// What a module contains, in source order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModuleItem {
    Module(ModuleId),
    Struct(StructId),
    Enum(EnumId),
    Fn(FnId),
    Trait(TraitId),
    Impl(ImplId),
    TypeAlias(TypeAliasId),
    Const(ConstId),
    Static(StaticId),
    Import(ImportId),
}

/// The item a function, constant or type alias belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Container {
    Module(ModuleId),
    Trait(TraitId),
    Impl(ImplId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssocItem {
    Fn(FnId),
    Const(ConstId),
    TypeAlias(TypeAliasId),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GenericParams {
    /// Lifetime, type and const parameters in declaration order.
    pub params: Vec<GenericParam>,
    pub where_predicates: Vec<WherePredicate>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericParam {
    pub name: Name,
    pub range: TextRange,
    pub kind: GenericParamKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenericParamKind {
    Lifetime {
        bounds: Vec<LifetimeRef>,
    },
    Type {
        bounds: Vec<TypeBound>,
        default: Option<TypeRef>,
    },
    Const {
        ty: TypeRef,
        default: Option<ConstExprPtr>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WherePredicate {
    Type {
        ty: TypeRef,
        bounds: Vec<TypeBound>,
    },
    Lifetime {
        lifetime: LifetimeRef,
        bounds: Vec<LifetimeRef>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fields {
    Record(Vec<Field>),
    Tuple(Vec<Field>),
    Unit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Field {
    /// Tuple fields are named by their index: `0`, `1`, ...
    pub name: Name,
    pub range: TextRange,
    pub vis: RawVisibility,
    pub ty: TypeRef,
}

#[derive(Debug)]
pub struct Struct {
    pub name: Name,
    pub loc: Loc,
    pub module: ModuleId,
    pub vis: RawVisibility,
    pub generics: GenericParams,
    pub fields: Fields,
    pub derives: Vec<Name>,
}

#[derive(Debug)]
pub struct Enum {
    pub name: Name,
    pub loc: Loc,
    pub module: ModuleId,
    pub vis: RawVisibility,
    pub generics: GenericParams,
    pub variants: Vec<VariantId>,
    pub derives: Vec<Name>,
}

#[derive(Debug)]
pub struct Variant {
    pub name: Name,
    pub loc: Loc,
    pub parent: EnumId,
    pub fields: Fields,
    pub discriminant: Option<ConstExprPtr>,
}

#[derive(Debug)]
pub struct Function {
    pub name: Name,
    pub loc: Loc,
    pub container: Container,
    pub vis: RawVisibility,
    pub generics: GenericParams,
    pub self_param: Option<SelfParam>,
    pub params: Vec<Param>,
    /// `None` means the unit type.
    pub ret: Option<TypeRef>,
    pub is_const: bool,
    pub is_async: bool,
    pub is_unsafe: bool,
    pub body: Option<InFile<AstPtr<ast::BlockExpr>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelfParam {
    pub range: TextRange,
    pub kind: SelfParamKind,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelfParamKind {
    /// `self` or `mut self`
    Value { mutable: bool },
    /// `&self`, `&'a mut self`, ...
    Ref {
        lifetime: Option<LifetimeRef>,
        mutable: bool,
    },
    /// `self: Type` or `mut self: Type`
    Typed { ty: TypeRef, mutable: bool },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    pub ty: TypeRef,
    pub pat: Option<InFile<AstPtr<ast::Pat>>>,
}

#[derive(Debug)]
pub struct Trait {
    pub name: Name,
    pub loc: Loc,
    pub module: ModuleId,
    pub vis: RawVisibility,
    pub generics: GenericParams,
    pub supertraits: Vec<TypeBound>,
    pub items: Vec<AssocItem>,
    pub is_unsafe: bool,
}

#[derive(Debug)]
pub struct Impl {
    /// The range of the `impl` header.
    pub loc: Loc,
    pub module: ModuleId,
    pub generics: GenericParams,
    pub trait_ref: Option<Path>,
    pub self_ty: TypeRef,
    pub items: Vec<AssocItem>,
    pub is_unsafe: bool,
}

#[derive(Debug)]
pub struct TypeAlias {
    pub name: Name,
    pub loc: Loc,
    pub container: Container,
    pub vis: RawVisibility,
    pub generics: GenericParams,
    /// Bounds on an associated type: `type Item: Clone;`
    pub bounds: Vec<TypeBound>,
    /// `None` for an associated type declared without a value in a trait.
    pub ty: Option<TypeRef>,
}

#[derive(Debug)]
pub struct Const {
    /// `None` for `const _: T = ...;`
    pub name: Option<Name>,
    pub loc: Loc,
    pub container: Container,
    pub vis: RawVisibility,
    pub ty: TypeRef,
    pub body: Option<ConstExprPtr>,
}

#[derive(Debug)]
pub struct Static {
    pub name: Name,
    pub loc: Loc,
    pub module: ModuleId,
    pub vis: RawVisibility,
    pub is_mut: bool,
    pub ty: TypeRef,
    pub body: Option<ConstExprPtr>,
}

/// One import from a `use` declaration: `use a::{b, c::*}` produces two.
#[derive(Debug)]
pub struct Import {
    pub module: ModuleId,
    pub file: FileId,
    /// The range of this import's own use tree.
    pub range: TextRange,
    pub path: ModPath,
    pub kind: ImportKind,
    pub vis: RawVisibility,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportKind {
    /// Binds `name`, or nothing for `as _`. A `self` import
    /// (`use a::{self}`) only imports into the type namespace.
    Single { name: Option<Name>, is_self: bool },
    /// `*`
    Glob,
}
