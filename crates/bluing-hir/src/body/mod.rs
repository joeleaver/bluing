//! Function bodies and constant initializers, lowered to body HIR with every
//! name resolved.
//!
//! Lowering resolves local variables and loop labels as it goes, since only
//! it knows the block structure. Paths to items are resolved afterwards, with
//! the same machinery as signatures: a local variable can only shadow a path
//! made of a single name, and lowering checks that case itself.

mod control;
mod expr;
mod literal;
mod lower;
mod pat;
mod resolve;

pub use literal::{FloatSuffix, IntSuffix, Literal};

use std::collections::BTreeMap;

use bluing_base::{Diagnostic, FileId, TextRange};
use la_arena::{Arena, ArenaMap, Idx};

use crate::hir::{GenericArgs, Path, TypeRef};
use crate::resolve::{LifetimeRes, PathRes};
use crate::{ConstId, CrateGraph, FnId, Hir, Name, Resolutions, StaticId};

/// The item a body belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BodyOwner {
    Fn(FnId),
    Const(ConstId),
    Static(StaticId),
}

pub type ExprId = Idx<Expr>;
pub type PatId = Idx<Pat>;
pub type LocalId = Idx<Local>;

/// A loop or labeled block that `break` can target (and, for loops,
/// `continue`). Indexes [`Body::break_targets`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BreakTarget(pub u32);

#[derive(Debug)]
pub struct Body {
    pub owner: BodyOwner,
    pub file: FileId,
    pub exprs: Arena<Expr>,
    pub expr_ranges: ArenaMap<ExprId, TextRange>,
    pub pats: Arena<Pat>,
    pub pat_ranges: ArenaMap<PatId, TextRange>,
    pub locals: Arena<Local>,
    /// The parameter patterns of a function, in order, after `self`.
    pub params: Vec<PatId>,
    /// The `self` parameter of a method.
    pub self_local: Option<LocalId>,
    /// The loop or block expression of each break target.
    pub break_targets: Vec<ExprId>,
    /// The function's block, or the constant's initializer.
    pub root: ExprId,
    /// Resolutions of the paths and lifetimes written inside the body. Their
    /// ids count separately from the ids of paths in signatures.
    pub paths: Vec<Option<PathRes>>,
    pub lifetimes: Vec<Option<LifetimeRes>>,
}

impl Body {
    /// The resolution of a path written in this body.
    pub fn path_res(&self, path: &Path) -> Option<PathRes> {
        self.paths.get(path.id.0 as usize).copied().flatten()
    }
}

/// A local variable: a binding in a pattern, or `self`.
#[derive(Debug)]
pub struct Local {
    pub name: Name,
    pub mutable: bool,
    pub range: TextRange,
}

#[derive(Debug)]
pub enum Expr {
    /// An expression that could not be lowered; a diagnostic was reported.
    Missing,
    Literal(Literal),
    Path(PathExpr),
    Block(Block),
    If {
        condition: ExprId,
        then_branch: ExprId,
        else_branch: Option<ExprId>,
    },
    Loop {
        body: ExprId,
        target: BreakTarget,
    },
    While {
        condition: ExprId,
        body: ExprId,
        target: BreakTarget,
    },
    Break {
        target: BreakTarget,
        value: Option<ExprId>,
    },
    Continue {
        target: BreakTarget,
    },
    Return(Option<ExprId>),
    Call {
        callee: ExprId,
        args: Vec<ExprId>,
    },
    MethodCall {
        receiver: ExprId,
        method: Name,
        method_range: TextRange,
        generic_args: Option<GenericArgs>,
        args: Vec<ExprId>,
    },
    /// Named or tuple-index (`0`, `1`, ...) field access.
    Field {
        base: ExprId,
        name: Name,
        name_range: TextRange,
    },
    /// `Path { field: value, ..base }`
    Struct {
        path: Path,
        fields: Vec<FieldInit>,
        base: Option<ExprId>,
    },
    Tuple(Vec<ExprId>),
    Array(Vec<ExprId>),
    /// `[value; count]`, where `count` is an integer literal or a constant
    /// generic parameter.
    Repeat {
        value: ExprId,
        count: ExprId,
    },
    Index {
        base: ExprId,
        index: ExprId,
    },
    Ref {
        mutable: bool,
        expr: ExprId,
    },
    Unary {
        op: UnaryOp,
        expr: ExprId,
    },
    Binary {
        op: BinaryOp,
        lhs: ExprId,
        rhs: ExprId,
    },
    Assign {
        lhs: ExprId,
        rhs: ExprId,
    },
    /// `lhs op= rhs`
    CompoundAssign {
        op: ArithOp,
        lhs: ExprId,
        rhs: ExprId,
    },
}

#[derive(Debug)]
pub enum PathExpr {
    Local(LocalId),
    /// A path to an item; its resolution is in [`Body::paths`].
    Item(Path),
}

#[derive(Debug)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub tail: Option<ExprId>,
    /// Set for a labeled block, which `break` can target.
    pub target: Option<BreakTarget>,
    pub is_unsafe: bool,
}

#[derive(Debug)]
pub enum Stmt {
    Let {
        pat: PatId,
        ty: Option<TypeRef>,
        init: Option<ExprId>,
    },
    Expr {
        expr: ExprId,
        has_semi: bool,
    },
}

#[derive(Debug)]
pub struct FieldInit {
    pub name: Name,
    pub range: TextRange,
    pub value: ExprId,
}

#[derive(Debug)]
pub enum Pat {
    /// A pattern that could not be lowered; a diagnostic was reported.
    Missing,
    Wild,
    Bind {
        local: LocalId,
    },
    Tuple(Vec<PatId>),
    /// `Path { field: pat, .. }`
    Struct {
        path: Path,
        fields: Vec<FieldPat>,
        has_rest: bool,
    },
}

#[derive(Debug)]
pub struct FieldPat {
    pub name: Name,
    pub range: TextRange,
    pub pat: PatId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnaryOp {
    Deref,
    Not,
    Neg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BinaryOp {
    Arith(ArithOp),
    /// `&&` and `||`, which short-circuit.
    Logic(LogicOp),
    Cmp(CmpOp),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ArithOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicOp {
    And,
    Or,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Lowers and resolves every body in the compilation.
pub(crate) fn lower_bodies(
    hir: &Hir,
    graph: &CrateGraph,
    resolutions: &Resolutions,
    files: &crate::Files,
) -> (BTreeMap<BodyOwner, Body>, Vec<Diagnostic>) {
    let mut bodies = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (owner, file, node) in lower::body_sources(hir, files) {
        let mut body = lower::lower(
            hir,
            graph,
            resolutions,
            owner,
            file,
            &node,
            &mut diagnostics,
        );
        resolve::resolve_paths(hir, graph, resolutions, &mut body, &mut diagnostics);
        bodies.insert(owner, body);
    }
    (bodies, diagnostics)
}
