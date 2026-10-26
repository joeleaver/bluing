//! Inference variables and unification.
//!
//! Type variables can be anything. Integer and float variables come from
//! unsuffixed literals and can only become integer or float types; if
//! nothing decides them they fall back to `i32` and `f64`, as in Rust.

use bluing_ty::fold::{TyFolder, super_fold_ty};
use bluing_ty::ty::{
    Const, FloatTy, GenericArg, InferTy, IntTy, Interner, Substs, Ty, TyKind, UintTy,
};
use ena::unify::{InPlace, InPlaceUnificationTable, NoError, Snapshot, UnifyKey, UnifyValue};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TyVid(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct IntVid(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FloatVid(u32);

/// The value of a type variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TyVarValue {
    Unknown,
    Known(Ty),
}

/// The integer type an integer variable became.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntKind {
    Int(IntTy),
    Uint(UintTy),
}

macro_rules! unify_key {
    ($key:ident, $value:ty, $tag:literal) => {
        impl UnifyKey for $key {
            type Value = $value;
            fn index(&self) -> u32 {
                self.0
            }
            fn from_index(index: u32) -> Self {
                $key(index)
            }
            fn tag() -> &'static str {
                $tag
            }
        }
    };
}

unify_key!(TyVid, TyVarValue, "TyVid");
unify_key!(IntVid, Option<IntKind>, "IntVid");
unify_key!(FloatVid, Option<FloatValue>, "FloatVid");

impl UnifyValue for TyVarValue {
    type Error = NoError;

    /// Only unknown variables are ever unioned; a known one is resolved
    /// before unification looks at it.
    fn unify_values(a: &Self, b: &Self) -> Result<Self, NoError> {
        Ok(match (a, b) {
            (TyVarValue::Unknown, other) | (other, TyVarValue::Unknown) => *other,
            (TyVarValue::Known(a), TyVarValue::Known(_)) => TyVarValue::Known(*a),
        })
    }
}

/// The float type a float variable became.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FloatValue(pub FloatTy);

impl UnifyValue for FloatValue {
    type Error = NoError;

    fn unify_values(a: &Self, _: &Self) -> Result<Self, NoError> {
        Ok(*a)
    }
}

impl UnifyValue for IntKind {
    type Error = NoError;

    fn unify_values(a: &Self, _: &Self) -> Result<Self, NoError> {
        Ok(*a)
    }
}

/// Two types that cannot be made equal.
#[derive(Debug)]
pub struct Mismatch;

struct InferSnapshot {
    tys: Snapshot<InPlace<TyVid>>,
    ints: Snapshot<InPlace<IntVid>>,
    floats: Snapshot<InPlace<FloatVid>>,
}

#[derive(Default)]
pub struct InferCtxt {
    tys: InPlaceUnificationTable<TyVid>,
    ints: InPlaceUnificationTable<IntVid>,
    floats: InPlaceUnificationTable<FloatVid>,
}

impl InferCtxt {
    pub fn new_ty_var(&mut self, interner: &mut Interner) -> Ty {
        let vid = self.tys.new_key(TyVarValue::Unknown);
        interner.intern(TyKind::Infer(InferTy::Ty(vid.0)))
    }

    pub fn new_int_var(&mut self, interner: &mut Interner) -> Ty {
        let vid = self.ints.new_key(None);
        interner.intern(TyKind::Infer(InferTy::Int(vid.0)))
    }

    pub fn new_float_var(&mut self, interner: &mut Interner) -> Ty {
        let vid = self.floats.new_key(None);
        interner.intern(TyKind::Infer(InferTy::Float(vid.0)))
    }

    /// Follows variables to what they are known to be. An unknown variable is
    /// returned as the root of its group, so equal variables compare equal.
    pub fn shallow_resolve(&mut self, interner: &mut Interner, ty: Ty) -> Ty {
        match *interner.kind(ty) {
            TyKind::Infer(InferTy::Ty(index)) => match self.tys.probe_value(TyVid(index)) {
                TyVarValue::Known(known) => self.shallow_resolve(interner, known),
                TyVarValue::Unknown => {
                    let root = self.tys.find(TyVid(index));
                    interner.intern(TyKind::Infer(InferTy::Ty(root.0)))
                }
            },
            TyKind::Infer(InferTy::Int(index)) => match self.ints.probe_value(IntVid(index)) {
                Some(IntKind::Int(int)) => interner.intern(TyKind::Int(int)),
                Some(IntKind::Uint(uint)) => interner.intern(TyKind::Uint(uint)),
                None => {
                    let root = self.ints.find(IntVid(index));
                    interner.intern(TyKind::Infer(InferTy::Int(root.0)))
                }
            },
            TyKind::Infer(InferTy::Float(index)) => {
                match self.floats.probe_value(FloatVid(index)) {
                    Some(FloatValue(float)) => interner.intern(TyKind::Float(float)),
                    None => {
                        let root = self.floats.find(FloatVid(index));
                        interner.intern(TyKind::Infer(InferTy::Float(root.0)))
                    }
                }
            }
            _ => ty,
        }
    }

    /// `ty` with every known variable replaced by its value.
    pub fn resolve(&mut self, interner: &mut Interner, ty: Ty) -> Ty {
        Resolver {
            infer: self,
            interner,
        }
        .fold_ty(ty)
    }

    /// Makes `a` and `b` equal, binding variables as needed. Lifetimes are
    /// ignored: inside bodies they are left for borrow checking. If the types
    /// cannot be made equal, no variable is changed.
    pub fn unify(&mut self, interner: &mut Interner, a: Ty, b: Ty) -> Result<(), Mismatch> {
        let snapshot = self.snapshot();
        let result = self.unify_inner(interner, a, b);
        match result {
            Ok(()) => self.commit(snapshot),
            Err(_) => self.rollback_to(snapshot),
        }
        result
    }

    /// Whether `a` and `b` could be made equal, changing nothing.
    pub fn can_unify(&mut self, interner: &mut Interner, a: Ty, b: Ty) -> bool {
        let snapshot = self.snapshot();
        let result = self.unify_inner(interner, a, b);
        self.rollback_to(snapshot);
        result.is_ok()
    }

    fn snapshot(&mut self) -> InferSnapshot {
        InferSnapshot {
            tys: self.tys.snapshot(),
            ints: self.ints.snapshot(),
            floats: self.floats.snapshot(),
        }
    }

    fn commit(&mut self, snapshot: InferSnapshot) {
        self.tys.commit(snapshot.tys);
        self.ints.commit(snapshot.ints);
        self.floats.commit(snapshot.floats);
    }

    fn rollback_to(&mut self, snapshot: InferSnapshot) {
        self.tys.rollback_to(snapshot.tys);
        self.ints.rollback_to(snapshot.ints);
        self.floats.rollback_to(snapshot.floats);
    }

    fn unify_inner(&mut self, interner: &mut Interner, a: Ty, b: Ty) -> Result<(), Mismatch> {
        let a = self.shallow_resolve(interner, a);
        let b = self.shallow_resolve(interner, b);
        if a == b {
            return Ok(());
        }
        match (interner.kind(a).clone(), interner.kind(b).clone()) {
            (TyKind::Error, _) | (_, TyKind::Error) => Ok(()),
            (TyKind::Infer(InferTy::Ty(v)), TyKind::Infer(InferTy::Ty(w))) => {
                self.tys.union(TyVid(v), TyVid(w));
                Ok(())
            }
            (TyKind::Infer(InferTy::Ty(v)), _) => self.bind(interner, v, b),
            (_, TyKind::Infer(InferTy::Ty(w))) => self.bind(interner, w, a),
            (TyKind::Infer(InferTy::Int(v)), TyKind::Infer(InferTy::Int(w))) => {
                self.ints.union(IntVid(v), IntVid(w));
                Ok(())
            }
            (TyKind::Infer(InferTy::Int(v)), other) | (other, TyKind::Infer(InferTy::Int(v))) => {
                let kind = match other {
                    TyKind::Int(int) => IntKind::Int(int),
                    TyKind::Uint(uint) => IntKind::Uint(uint),
                    _ => return Err(Mismatch),
                };
                self.ints.union_value(IntVid(v), Some(kind));
                Ok(())
            }
            (TyKind::Infer(InferTy::Float(v)), TyKind::Infer(InferTy::Float(w))) => {
                self.floats.union(FloatVid(v), FloatVid(w));
                Ok(())
            }
            (TyKind::Infer(InferTy::Float(v)), TyKind::Float(float))
            | (TyKind::Float(float), TyKind::Infer(InferTy::Float(v))) => {
                self.floats
                    .union_value(FloatVid(v), Some(FloatValue(float)));
                Ok(())
            }
            (TyKind::Adt(x, xs), TyKind::Adt(y, ys)) if x == y => {
                self.unify_substs(interner, xs, ys)
            }
            (TyKind::FnDef(x, xs), TyKind::FnDef(y, ys)) if x == y => {
                self.unify_substs(interner, xs, ys)
            }
            (TyKind::Ref(_, x, xm), TyKind::Ref(_, y, ym)) if xm == ym => {
                self.unify_inner(interner, x, y)
            }
            (TyKind::RawPtr(x, xm), TyKind::RawPtr(y, ym)) if xm == ym => {
                self.unify_inner(interner, x, y)
            }
            (TyKind::Slice(x), TyKind::Slice(y)) => self.unify_inner(interner, x, y),
            (TyKind::Array(x, xn), TyKind::Array(y, yn)) => {
                self.unify_inner(interner, x, y)?;
                unify_consts(xn, yn)
            }
            (TyKind::Tuple(xs), TyKind::Tuple(ys)) => {
                let (xs, ys) = (interner.tys(xs).to_vec(), interner.tys(ys).to_vec());
                self.unify_lists(interner, &xs, &ys)
            }
            (TyKind::FnPtr(x), TyKind::FnPtr(y)) if x.is_unsafe == y.is_unsafe => {
                let xs = interner.tys(x.inputs_and_output).to_vec();
                let ys = interner.tys(y.inputs_and_output).to_vec();
                self.unify_lists(interner, &xs, &ys)
            }
            _ => Err(Mismatch),
        }
    }

    fn unify_lists(
        &mut self,
        interner: &mut Interner,
        xs: &[Ty],
        ys: &[Ty],
    ) -> Result<(), Mismatch> {
        if xs.len() != ys.len() {
            return Err(Mismatch);
        }
        for (&x, &y) in xs.iter().zip(ys) {
            self.unify_inner(interner, x, y)?;
        }
        Ok(())
    }

    fn unify_substs(
        &mut self,
        interner: &mut Interner,
        xs: Substs,
        ys: Substs,
    ) -> Result<(), Mismatch> {
        let (xs, ys) = (interner.args(xs).to_vec(), interner.args(ys).to_vec());
        for (x, y) in xs.into_iter().zip(ys) {
            match (x, y) {
                (GenericArg::Ty(x), GenericArg::Ty(y)) => self.unify_inner(interner, x, y)?,
                (GenericArg::Const(x), GenericArg::Const(y)) => unify_consts(x, y)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn bind(&mut self, interner: &mut Interner, var: u32, ty: Ty) -> Result<(), Mismatch> {
        if self.occurs(interner, var, ty) {
            return Err(Mismatch);
        }
        self.tys.union_value(TyVid(var), TyVarValue::Known(ty));
        Ok(())
    }

    /// Whether type variable `var` appears in `ty`, which would make binding
    /// it an infinite type. Resolving leaves only root variables in `ty`.
    fn occurs(&mut self, interner: &mut Interner, var: u32, ty: Ty) -> bool {
        let root = self.tys.find(TyVid(var));
        let ty = self.resolve(interner, ty);
        let mut found = false;
        visit(interner, ty, &mut |kind| {
            if let TyKind::Infer(InferTy::Ty(other)) = kind {
                found |= *other == root.0;
            }
        });
        found
    }

    /// Unknown integer and float variables become `i32` and `f64`.
    pub fn fall_back(&mut self, interner: &mut Interner, ty: Ty) -> Ty {
        let resolved = self.shallow_resolve(interner, ty);
        match *interner.kind(resolved) {
            TyKind::Infer(InferTy::Int(index)) => {
                self.ints
                    .union_value(IntVid(index), Some(IntKind::Int(IntTy::I32)));
                interner.intern(TyKind::Int(IntTy::I32))
            }
            TyKind::Infer(InferTy::Float(index)) => {
                self.floats
                    .union_value(FloatVid(index), Some(FloatValue(FloatTy::F64)));
                interner.intern(TyKind::Float(FloatTy::F64))
            }
            _ => ty,
        }
    }
}

fn unify_consts(x: Const, y: Const) -> Result<(), Mismatch> {
    match (x, y) {
        (Const::Error, _) | (_, Const::Error) => Ok(()),
        (x, y) if x == y => Ok(()),
        _ => Err(Mismatch),
    }
}

/// Calls `f` on `ty` and every type inside it.
pub fn visit(interner: &Interner, ty: Ty, f: &mut impl FnMut(&TyKind)) {
    let kind = interner.kind(ty);
    f(kind);
    match kind {
        TyKind::Adt(_, substs) | TyKind::FnDef(_, substs) => {
            for arg in interner.args(*substs) {
                if let GenericArg::Ty(ty) = arg {
                    visit(interner, *ty, f);
                }
            }
        }
        TyKind::Ref(_, inner, _)
        | TyKind::RawPtr(inner, _)
        | TyKind::Slice(inner)
        | TyKind::Array(inner, _) => {
            visit(interner, *inner, f);
        }
        TyKind::Tuple(list) => {
            for &ty in interner.tys(*list) {
                visit(interner, ty, f);
            }
        }
        TyKind::FnPtr(sig) => {
            for &ty in interner.tys(sig.inputs_and_output) {
                visit(interner, ty, f);
            }
        }
        _ => {}
    }
}

struct Resolver<'a> {
    infer: &'a mut InferCtxt,
    interner: &'a mut Interner,
}

impl TyFolder for Resolver<'_> {
    fn interner(&mut self) -> &mut Interner {
        self.interner
    }

    fn fold_ty(&mut self, ty: Ty) -> Ty {
        let resolved = self.infer.shallow_resolve(self.interner, ty);
        if matches!(self.interner.kind(resolved), TyKind::Infer(_)) {
            return resolved;
        }
        super_fold_ty(self, resolved)
    }
}
