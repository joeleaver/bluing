use bluing_hir::body::{ExprId, LocalId, PatId};
use bluing_hir::{ConstId, FnId};
use bluing_ty::ty::{Mutability, Ty};
use la_arena::ArenaMap;

/// What type checking learned about one body. Every type is fully resolved.
#[derive(Debug, Default)]
pub struct TypeckResults {
    pub expr_tys: ArenaMap<ExprId, Ty>,
    pub pat_tys: ArenaMap<PatId, Ty>,
    pub local_tys: ArenaMap<LocalId, Ty>,
    /// The function each method call calls.
    pub method_calls: ArenaMap<ExprId, FnId>,
    /// What each type-relative path (`Type::item`) names.
    pub assoc_values: ArenaMap<ExprId, AssocValue>,
    /// The index of the field each field access reads.
    pub field_indices: ArenaMap<ExprId, usize>,
    /// The implicit conversions applied to an expression's value.
    pub adjustments: ArenaMap<ExprId, Adjustment>,
}

/// An associated item named through a type: `Point::new`, `Self::ORIGIN`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssocValue {
    Fn(FnId),
    Const(ConstId),
}

/// Implicit conversions of an expression's value: automatic dereferences
/// (method receivers, field access), then an automatic borrow (method
/// receivers), or the coercion of `!` to the expected type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Adjustment {
    pub derefs: u32,
    pub autoref: Option<Mutability>,
    pub never_to_any: bool,
}
