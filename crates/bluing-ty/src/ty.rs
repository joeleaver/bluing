//! The type representation. Types, type lists and generic argument lists are
//! interned, so they are small `Copy` handles compared by identity.

use bluing_hir::{EnumId, FnId, StructId, VariantId};
use rustc_hash::FxHashMap;

/// An interned type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Ty(u32);

/// An interned list of types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TyList(u32);

/// An interned list of generic arguments, one per generic parameter of an
/// item (including its parent's), in parameter order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Substs(u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AdtId {
    Struct(StructId),
    Enum(EnumId),
}

/// Something that can be called like a function: a function, or the
/// constructor of a tuple struct or tuple variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CallableId {
    Fn(FnId),
    StructCtor(StructId),
    VariantCtor(VariantId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Mutability {
    Not,
    Mut,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum IntTy {
    I8,
    I16,
    I32,
    I64,
    I128,
    Isize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum UintTy {
    U8,
    U16,
    U32,
    U64,
    U128,
    Usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FloatTy {
    F32,
    F64,
}

/// A generic parameter, by its index among all the generic parameters in
/// scope (the parent item's first, then a trait's implicit `Self`, then the
/// item's own).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ParamIndex(pub u32);

/// A lifetime as far as types are concerned. Type checking does not infer
/// lifetimes inside bodies (they are [`Region::Erased`]); borrow checking
/// does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Region {
    Static,
    /// A lifetime parameter of the item.
    Param(ParamIndex),
    /// An elided or `'_` lifetime in a signature: a distinct, unnamed
    /// lifetime, numbered within the signature.
    Anon(u32),
    /// A lifetime inside a body, left for borrow checking.
    Erased,
    Error,
}

/// The value of a constant generic argument: so far, array lengths.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Const {
    Value(u128),
    Param(ParamIndex),
    Infer(u32),
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GenericArg {
    Ty(Ty),
    Region(Region),
    Const(Const),
}

/// An inference variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InferTy {
    /// Any type.
    Ty(u32),
    /// An integer type, from an unsuffixed integer literal.
    Int(u32),
    /// A float type, from an unsuffixed float literal.
    Float(u32),
}

/// A function signature: parameter types and the return type, last.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FnSig {
    pub inputs_and_output: TyList,
    pub is_unsafe: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum TyKind {
    Bool,
    Char,
    Str,
    Never,
    Int(IntTy),
    Uint(UintTy),
    Float(FloatTy),
    Adt(AdtId, Substs),
    Ref(Region, Ty, Mutability),
    RawPtr(Ty, Mutability),
    Array(Ty, Const),
    Slice(Ty),
    Tuple(TyList),
    /// The type of a function item or constructor, unique to it.
    FnDef(CallableId, Substs),
    FnPtr(FnSig),
    Param(ParamIndex),
    Infer(InferTy),
    /// A type that could not be determined; a diagnostic was reported.
    Error,
}

/// Owns every interned type and list.
#[derive(Default)]
pub struct Interner {
    tys: Vec<TyKind>,
    ty_map: FxHashMap<TyKind, Ty>,
    ty_lists: Vec<Vec<Ty>>,
    ty_list_map: FxHashMap<Vec<Ty>, TyList>,
    substs: Vec<Vec<GenericArg>>,
    substs_map: FxHashMap<Vec<GenericArg>, Substs>,
}

impl Interner {
    pub fn intern(&mut self, kind: TyKind) -> Ty {
        if let Some(&ty) = self.ty_map.get(&kind) {
            return ty;
        }
        let ty = Ty(u32::try_from(self.tys.len()).expect("fewer than u32::MAX types"));
        self.tys.push(kind.clone());
        self.ty_map.insert(kind, ty);
        ty
    }

    pub fn kind(&self, ty: Ty) -> &TyKind {
        &self.tys[ty.0 as usize]
    }

    pub fn ty_list(&mut self, tys: Vec<Ty>) -> TyList {
        if let Some(&list) = self.ty_list_map.get(&tys) {
            return list;
        }
        let list = TyList(u32::try_from(self.ty_lists.len()).expect("fewer than u32::MAX lists"));
        self.ty_lists.push(tys.clone());
        self.ty_list_map.insert(tys, list);
        list
    }

    pub fn tys(&self, list: TyList) -> &[Ty] {
        &self.ty_lists[list.0 as usize]
    }

    pub fn substs(&mut self, args: Vec<GenericArg>) -> Substs {
        if let Some(&substs) = self.substs_map.get(&args) {
            return substs;
        }
        let substs = Substs(u32::try_from(self.substs.len()).expect("fewer than u32::MAX lists"));
        self.substs.push(args.clone());
        self.substs_map.insert(args, substs);
        substs
    }

    pub fn args(&self, substs: Substs) -> &[GenericArg] {
        &self.substs[substs.0 as usize]
    }

    pub fn unit(&mut self) -> Ty {
        let empty = self.ty_list(Vec::new());
        self.intern(TyKind::Tuple(empty))
    }

    pub fn error(&mut self) -> Ty {
        self.intern(TyKind::Error)
    }

    pub fn never(&mut self) -> Ty {
        self.intern(TyKind::Never)
    }

    pub fn bool(&mut self) -> Ty {
        self.intern(TyKind::Bool)
    }

    pub fn usize(&mut self) -> Ty {
        self.intern(TyKind::Uint(UintTy::Usize))
    }

    pub fn empty_substs(&mut self) -> Substs {
        self.substs(Vec::new())
    }

    /// The parameter and return types of a signature.
    pub fn sig_parts(&self, sig: FnSig) -> (&[Ty], Ty) {
        let all = self.tys(sig.inputs_and_output);
        let (output, inputs) = all.split_last().expect("a signature has a return type");
        (inputs, *output)
    }

    pub fn is_unit(&self, ty: Ty) -> bool {
        matches!(self.kind(ty), TyKind::Tuple(list) if self.tys(*list).is_empty())
    }
}
