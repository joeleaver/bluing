//! Printing types for diagnostics, the way Rust writes them.

use std::fmt;

use bluing_hir::Hir;

use crate::generics::Generics;
use crate::ty::{
    AdtId, CallableId, Const, FloatTy, GenericArg, InferTy, IntTy, Interner, Mutability, Region,
    Ty, TyKind, UintTy,
};

/// A type ready to print. `generics` names the generic parameters in scope;
/// without it they print by index.
pub struct TyDisplay<'a> {
    pub interner: &'a Interner,
    pub hir: &'a Hir,
    pub generics: Option<&'a Generics>,
    pub ty: Ty,
}

impl TyDisplay<'_> {
    fn with(&self, ty: Ty) -> TyDisplay<'_> {
        TyDisplay {
            interner: self.interner,
            hir: self.hir,
            generics: self.generics,
            ty,
        }
    }

    fn param(&self, f: &mut fmt::Formatter<'_>, index: u32) -> fmt::Result {
        match self
            .generics
            .and_then(|g| g.name(crate::ty::ParamIndex(index)))
        {
            Some(name) => write!(f, "{name}"),
            None => write!(f, "<param {index}>"),
        }
    }

    fn region(&self, f: &mut fmt::Formatter<'_>, region: Region) -> fmt::Result {
        match region {
            Region::Static => write!(f, "'static "),
            Region::Param(index) => {
                write!(f, "'")?;
                self.param(f, index.0)?;
                write!(f, " ")
            }
            Region::Anon(_) | Region::Erased | Region::Error => Ok(()),
        }
    }

    fn const_value(&self, f: &mut fmt::Formatter<'_>, value: Const) -> fmt::Result {
        match value {
            Const::Value(value) => write!(f, "{value}"),
            Const::Param(index) => self.param(f, index.0),
            Const::Infer(_) | Const::Error => write!(f, "_"),
        }
    }

    fn list(&self, f: &mut fmt::Formatter<'_>, tys: &[Ty]) -> fmt::Result {
        for (i, &ty) in tys.iter().enumerate() {
            if i > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}", self.with(ty))?;
        }
        Ok(())
    }
}

impl fmt::Display for TyDisplay<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.interner.kind(self.ty) {
            TyKind::Bool => write!(f, "bool"),
            TyKind::Char => write!(f, "char"),
            TyKind::Str => write!(f, "str"),
            TyKind::Never => write!(f, "!"),
            TyKind::Int(int) => write!(f, "{}", int_name(*int)),
            TyKind::Uint(uint) => write!(f, "{}", uint_name(*uint)),
            TyKind::Float(FloatTy::F32) => write!(f, "f32"),
            TyKind::Float(FloatTy::F64) => write!(f, "f64"),
            TyKind::Adt(adt, substs) => {
                let name = match adt {
                    AdtId::Struct(id) => &self.hir.structs[*id].name,
                    AdtId::Enum(id) => &self.hir.enums[*id].name,
                };
                write!(f, "{name}")?;
                // Lifetimes are not shown, as in most of rustc's messages.
                let shown: Vec<&GenericArg> = self
                    .interner
                    .args(*substs)
                    .iter()
                    .filter(|arg| !matches!(arg, GenericArg::Region(_)))
                    .collect();
                if !shown.is_empty() {
                    write!(f, "<")?;
                    for (i, arg) in shown.into_iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        match arg {
                            GenericArg::Ty(ty) => write!(f, "{}", self.with(*ty))?,
                            GenericArg::Const(value) => self.const_value(f, *value)?,
                            GenericArg::Region(_) => {}
                        }
                    }
                    write!(f, ">")?;
                }
                Ok(())
            }
            TyKind::Ref(region, inner, mutability) => {
                write!(f, "&")?;
                self.region(f, *region)?;
                if *mutability == Mutability::Mut {
                    write!(f, "mut ")?;
                }
                write!(f, "{}", self.with(*inner))
            }
            TyKind::RawPtr(inner, mutability) => {
                let kind = if *mutability == Mutability::Mut {
                    "mut"
                } else {
                    "const"
                };
                write!(f, "*{kind} {}", self.with(*inner))
            }
            TyKind::Array(elem, len) => {
                write!(f, "[{}; ", self.with(*elem))?;
                self.const_value(f, *len)?;
                write!(f, "]")
            }
            TyKind::Slice(elem) => write!(f, "[{}]", self.with(*elem)),
            TyKind::Tuple(list) => {
                let tys = self.interner.tys(*list);
                write!(f, "(")?;
                self.list(f, tys)?;
                if tys.len() == 1 {
                    write!(f, ",")?;
                }
                write!(f, ")")
            }
            TyKind::FnDef(callable, _) => {
                let name = match callable {
                    CallableId::Fn(id) => &self.hir.functions[*id].name,
                    CallableId::StructCtor(id) => &self.hir.structs[*id].name,
                    CallableId::VariantCtor(id) => &self.hir.variants[*id].name,
                };
                write!(f, "fn item `{name}`")
            }
            TyKind::FnPtr(sig) => {
                let (inputs, output) = self.interner.sig_parts(*sig);
                if sig.is_unsafe {
                    write!(f, "unsafe ")?;
                }
                write!(f, "fn(")?;
                self.list(f, inputs)?;
                write!(f, ")")?;
                if !self.interner.is_unit(output) {
                    write!(f, " -> {}", self.with(output))?;
                }
                Ok(())
            }
            TyKind::Param(index) => self.param(f, index.0),
            TyKind::Infer(InferTy::Ty(_)) => write!(f, "_"),
            TyKind::Infer(InferTy::Int(_)) => write!(f, "{{integer}}"),
            TyKind::Infer(InferTy::Float(_)) => write!(f, "{{float}}"),
            TyKind::Error => write!(f, "{{type error}}"),
        }
    }
}

pub fn int_name(int: IntTy) -> &'static str {
    match int {
        IntTy::I8 => "i8",
        IntTy::I16 => "i16",
        IntTy::I32 => "i32",
        IntTy::I64 => "i64",
        IntTy::I128 => "i128",
        IntTy::Isize => "isize",
    }
}

pub fn uint_name(uint: UintTy) -> &'static str {
    match uint {
        UintTy::U8 => "u8",
        UintTy::U16 => "u16",
        UintTy::U32 => "u32",
        UintTy::U64 => "u64",
        UintTy::U128 => "u128",
        UintTy::Usize => "usize",
    }
}
