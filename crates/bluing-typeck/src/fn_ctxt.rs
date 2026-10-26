//! The state of checking one body, and the helpers every part of the checker
//! shares: errors, unification, coercion and expectations.

use bluing_base::{Code, Diagnostic, TextRange};
use bluing_hir::body::{Body, BodyOwner, BreakTarget, ExprId, LocalId};
use bluing_hir::hir::Container;
use bluing_hir::{GenericOwner, ModuleId};
use bluing_ty::TyCx;
use bluing_ty::generics::{Generics, generics_of};
use bluing_ty::ty::{Ty, TyKind};

use crate::coerce::CoerceMany;
use crate::infer::InferCtxt;
use crate::results::TypeckResults;

/// What the context of an expression expects its type to be. It guides
/// inference (an empty array, a literal) without being a requirement; the
/// requirement is checked where the value is used.
#[derive(Clone, Copy, Debug)]
pub enum Expectation {
    None,
    Has(Ty),
}

/// A loop or labeled block that `break` can leave.
pub struct Breakable {
    pub target: BreakTarget,
    /// Collects the values `break` gives a `loop` or labeled block. `None`
    /// for `while`, whose `break` takes no value.
    pub coerce: Option<CoerceMany>,
    /// Whether any `break` targets it.
    pub may_break: bool,
}

pub struct FnCtxt<'cx, 'a> {
    pub cx: &'cx mut TyCx<'a>,
    pub body: &'cx Body,
    /// The module the body is in, for visibility.
    pub module: ModuleId,
    pub generics: Generics,
    pub infer: InferCtxt,
    pub results: TypeckResults,
    /// The function's return type; `None` in a constant's initializer.
    pub ret_ty: Option<Ty>,
    pub breakables: Vec<Breakable>,
    /// Whether the code is inside an `unsafe` block or function.
    pub in_unsafe: bool,
    /// Whether the code checked so far always diverges (returns, breaks,
    /// loops forever) before reaching the current point.
    pub diverges: bool,
    /// Integer literals with their magnitude and whether they are negated,
    /// to check they fit their final type.
    pub int_literals: Vec<(ExprId, u128, bool)>,
    /// Float literals with their digits, for the same check.
    pub float_literals: Vec<(ExprId, String)>,
    /// Negated expressions whose type was an unresolved integer variable,
    /// checked once types are final.
    pub deferred_negations: Vec<ExprId>,
}

impl<'cx, 'a> FnCtxt<'cx, 'a> {
    pub fn new(cx: &'cx mut TyCx<'a>, body: &'cx Body) -> Self {
        let hir = cx.hir;
        let (module, generics) = match body.owner {
            BodyOwner::Fn(id) => {
                let module = container_module(hir, hir.functions[id].container);
                (module, generics_of(hir, GenericOwner::Fn(id)))
            }
            BodyOwner::Const(id) => {
                let container = hir.consts[id].container;
                let generics = match container {
                    Container::Module(_) => Generics::default(),
                    Container::Trait(t) => generics_of(hir, GenericOwner::Trait(t)),
                    Container::Impl(i) => generics_of(hir, GenericOwner::Impl(i)),
                };
                (container_module(hir, container), generics)
            }
            BodyOwner::Static(id) => (hir.statics[id].module, Generics::default()),
        };
        FnCtxt {
            cx,
            body,
            module,
            generics,
            infer: InferCtxt::default(),
            results: TypeckResults::default(),
            ret_ty: None,
            breakables: Vec::new(),
            in_unsafe: false,
            diverges: false,
            int_literals: Vec::new(),
            float_literals: Vec::new(),
            deferred_negations: Vec::new(),
        }
    }

    pub fn error(&mut self, range: TextRange, code: Code, message: impl Into<String>) {
        self.cx
            .diagnostics
            .push(Diagnostic::new(self.body.file, range, code, message));
    }

    pub fn expr_range(&self, expr: ExprId) -> TextRange {
        self.body.expr_ranges.get(expr).copied().unwrap_or_default()
    }

    /// Where to report a value that does not coerce: a block's value is its
    /// last expression.
    fn coercion_range(&self, expr: ExprId) -> TextRange {
        match &self.body.exprs[expr] {
            bluing_hir::body::Expr::Block(block) => match block.tail {
                Some(tail) => self.coercion_range(tail),
                None => self.expr_range(expr),
            },
            _ => self.expr_range(expr),
        }
    }

    pub fn new_var(&mut self) -> Ty {
        self.infer.new_ty_var(&mut self.cx.interner)
    }

    pub fn shallow(&mut self, ty: Ty) -> Ty {
        self.infer.shallow_resolve(&mut self.cx.interner, ty)
    }

    pub fn kind(&mut self, ty: Ty) -> TyKind {
        let ty = self.shallow(ty);
        self.cx.interner.kind(ty).clone()
    }

    pub fn resolve(&mut self, ty: Ty) -> Ty {
        self.infer.resolve(&mut self.cx.interner, ty)
    }

    pub fn unify(&mut self, a: Ty, b: Ty) -> bool {
        self.infer.unify(&mut self.cx.interner, a, b).is_ok()
    }

    pub fn intern(&mut self, kind: TyKind) -> Ty {
        self.cx.interner.intern(kind)
    }

    /// `ty` printed for a diagnostic, with what inference knows filled in.
    pub fn display(&mut self, ty: Ty) -> String {
        let ty = self.resolve(ty);
        self.cx.display(ty, Some(&self.generics))
    }

    /// Requires `actual` to equal `expected`, reporting a mismatch at `range`.
    pub fn demand_eq(&mut self, range: TextRange, expected: Ty, actual: Ty) -> bool {
        if self.unify(expected, actual) {
            return true;
        }
        let message = format!(
            "mismatched types: expected `{}`, found `{}`",
            self.display(expected),
            self.display(actual)
        );
        self.error(range, Code::TypeMismatch, message);
        false
    }

    /// Converts the value of `expr`, of type `actual`, to `expected` at a
    /// coercion site. So far the only coercion is from `!` to any type;
    /// otherwise the types must be equal. Where one of the coercions of
    /// milestone 3d would apply, that is reported as not supported yet rather
    /// than as a mismatch.
    pub fn coerce(&mut self, expr: ExprId, actual: Ty, expected: Ty) -> bool {
        if matches!(self.kind(actual), TyKind::Never) {
            self.results
                .adjustments
                .entry(expr)
                .or_default()
                .never_to_any = true;
            return true;
        }
        if self.unify(expected, actual) {
            return true;
        }
        let range = self.coercion_range(expr);
        if let Some(what) = self.later_coercion(actual, expected) {
            let message = format!("{what} coercions are not supported yet");
            self.error(range, Code::NotYetSupported, message);
            return false;
        }
        self.demand_eq(range, expected, actual)
    }

    /// The kind of milestone 3d coercion that could convert `actual` to
    /// `expected`, if any.
    fn later_coercion(&mut self, actual: Ty, expected: Ty) -> Option<&'static str> {
        use bluing_ty::ty::Mutability::{Mut, Not};
        let can_unify =
            |fcx: &mut Self, a: Ty, b: Ty| fcx.infer.can_unify(&mut fcx.cx.interner, a, b);
        match (self.kind(actual), self.kind(expected)) {
            (TyKind::Ref(_, a, from), TyKind::Ref(_, b, to)) => {
                let allowed = from == to || to == Not;
                if from == Mut && to == Not && can_unify(self, a, b) {
                    return Some("`&mut` to `&`");
                }
                match (self.kind(a), self.kind(b)) {
                    (TyKind::Array(x, _), TyKind::Slice(y)) if allowed && can_unify(self, x, y) => {
                        Some("unsizing")
                    }
                    // The pointee may dereference to the expected one.
                    (TyKind::Ref(..) | TyKind::Adt(..) | TyKind::Param(_), _) if allowed => {
                        Some("deref")
                    }
                    _ => None,
                }
            }
            (TyKind::Ref(_, a, from), TyKind::RawPtr(b, to))
                if (from == to || to == Not) && can_unify(self, a, b) =>
            {
                Some("reference-to-pointer")
            }
            (TyKind::RawPtr(a, Mut), TyKind::RawPtr(b, Not)) if can_unify(self, a, b) => {
                Some("pointer")
            }
            (TyKind::FnDef(callable, substs), TyKind::FnPtr(sig)) => {
                let (inputs, output) = self.callable_sig(callable, substs);
                let (ptr_inputs, ptr_output) = self.cx.interner.sig_parts(sig);
                let (ptr_inputs, ptr_output) = (ptr_inputs.to_vec(), ptr_output);
                let matches = inputs.len() == ptr_inputs.len()
                    && inputs
                        .iter()
                        .zip(&ptr_inputs)
                        .all(|(&a, &b)| can_unify(self, a, b))
                    && can_unify(self, output, ptr_output);
                matches.then_some("function item to function pointer")
            }
            _ => None,
        }
    }

    pub fn local_ty(&mut self, local: LocalId) -> Ty {
        match self.results.local_tys.get(local) {
            Some(&ty) => ty,
            // A local whose pattern failed to check.
            None => self.cx.interner.error(),
        }
    }

    /// A signature type as seen inside the body: its lifetimes are left for
    /// borrow checking.
    pub fn erase_regions(&mut self, ty: Ty) -> Ty {
        bluing_ty::fold::erase_regions(&mut self.cx.interner, ty)
    }
}

impl Expectation {
    pub fn ty(self) -> Option<Ty> {
        match self {
            Expectation::None => None,
            Expectation::Has(ty) => Some(ty),
        }
    }
}

fn container_module(hir: &bluing_hir::Hir, container: Container) -> ModuleId {
    match container {
        Container::Module(module) => module,
        Container::Trait(id) => hir.traits[id].module,
        Container::Impl(id) => hir.impls[id].module,
    }
}
