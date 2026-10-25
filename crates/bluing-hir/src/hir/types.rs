//! Types, paths, generic arguments and bounds as written in signatures.

use bluing_base::{FileId, TextRange};
use bluing_syntax::ra_ap_syntax::{AstPtr, ast};

use crate::{LifetimeId, Name, PathId};

/// A syntax node kept for a later milestone (a function body, an array
/// length, a constant initializer), with the file it lives in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InFile<T> {
    pub file: FileId,
    pub value: T,
}

/// An unevaluated constant expression: array lengths, const generic
/// arguments and defaults, enum discriminants.
pub type ConstExprPtr = InFile<AstPtr<ast::Expr>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeRef {
    Path(Path),
    Reference {
        lifetime: Option<LifetimeRef>,
        mutable: bool,
        inner: Box<TypeRef>,
    },
    RawPtr {
        mutable: bool,
        inner: Box<TypeRef>,
    },
    Slice(Box<TypeRef>),
    Array {
        elem: Box<TypeRef>,
        len: Option<ConstExprPtr>,
    },
    /// Includes the unit type `()`.
    Tuple(Vec<TypeRef>),
    Never,
    FnPtr {
        params: Vec<TypeRef>,
        ret: Box<TypeRef>,
        is_unsafe: bool,
    },
    ImplTrait(Vec<TypeBound>),
    DynTrait(Vec<TypeBound>),
    /// `_`
    Infer,
    /// A type that could not be lowered; a diagnostic was already reported.
    Error,
}

/// A path such as `a::b::C<T>`, `crate::x`, `<T as Trait>::Item` or `Self`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Path {
    pub id: PathId,
    pub kind: PathKind,
    /// The `<T as Trait>` of a qualified path.
    pub qself: Option<Box<QualifiedSelf>>,
    pub segments: Vec<PathSegment>,
    pub range: TextRange,
}

/// How a path starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    /// Starts with a name, looked up in the lexical scope.
    Plain,
    /// `crate::`
    Crate,
    /// `self::`
    SelfModule,
    /// `super::` repeated this many times.
    Super(u32),
    /// `::`, which only names extern crates.
    Global,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QualifiedSelf {
    pub ty: TypeRef,
    pub trait_ref: Option<Path>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathSegment {
    pub name: Name,
    pub range: TextRange,
    pub args: Option<GenericArgs>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenericArgs {
    pub args: Vec<GenericArg>,
    pub bindings: Vec<AssocBinding>,
    /// `Fn(A, B) -> C` sugar: `args` is the tuple `(A, B)` and `bindings`
    /// holds `Output = C`.
    pub parenthesized: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenericArg {
    Type(TypeRef),
    Lifetime(LifetimeRef),
    Const(ConstExprPtr),
}

/// `Item = T` or `Item: Bound` inside generic arguments.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssocBinding {
    pub name: Name,
    pub range: TextRange,
    pub args: Option<GenericArgs>,
    pub value: AssocBindingValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssocBindingValue {
    Type(TypeRef),
    Bounds(Vec<TypeBound>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TypeBound {
    /// `Trait` or, with `maybe`, `?Trait`.
    Trait {
        path: Path,
        maybe: bool,
    },
    Lifetime(LifetimeRef),
    /// A bound that could not be lowered; a diagnostic was already reported.
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LifetimeRef {
    pub id: LifetimeId,
    /// Without the leading `'`: `a`, `static`, `_`.
    pub name: Name,
    pub range: TextRange,
}
