//! Facts about the target. Bluing compiles to wasm32, where `usize` and
//! `isize` are 32 bits wide.

use crate::ty::{IntTy, TyKind, UintTy};

/// The width in bits of `usize` and `isize`.
pub const POINTER_BITS: u32 = 32;

/// Whether an integer literal with magnitude `value` fits the integer type
/// `kind`. `negated` is set for a literal that is the operand of unary `-`,
/// which may be one larger (`-128i8`).
pub fn int_fits_signed(kind: &TyKind, value: u128, negated: bool) -> bool {
    let (bits, signed) = match kind {
        TyKind::Int(int) => (int_bits(*int), true),
        TyKind::Uint(uint) => (uint_bits(*uint), false),
        _ => return true,
    };
    let max = if signed {
        let max = (1u128 << (bits - 1)) - 1;
        if negated { max + 1 } else { max }
    } else if bits == 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    };
    value <= max
}

/// [`int_fits_signed`] for a literal that is not negated.
pub fn int_fits(kind: &TyKind, value: u128) -> bool {
    int_fits_signed(kind, value, false)
}

pub fn int_bits(int: IntTy) -> u32 {
    match int {
        IntTy::I8 => 8,
        IntTy::I16 => 16,
        IntTy::I32 => 32,
        IntTy::I64 => 64,
        IntTy::I128 => 128,
        IntTy::Isize => POINTER_BITS,
    }
}

pub fn uint_bits(uint: UintTy) -> u32 {
    match uint {
        UintTy::U8 => 8,
        UintTy::U16 => 16,
        UintTy::U32 => 32,
        UintTy::U64 => 64,
        UintTy::U128 => 128,
        UintTy::Usize => POINTER_BITS,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_ranges() {
        let u8_kind = TyKind::Uint(UintTy::U8);
        assert!(int_fits(&u8_kind, 255));
        assert!(!int_fits(&u8_kind, 256));
        let i8_kind = TyKind::Int(IntTy::I8);
        assert!(int_fits(&i8_kind, 127));
        assert!(!int_fits(&i8_kind, 128));
        assert!(int_fits_signed(&i8_kind, 128, true));
        assert!(!int_fits_signed(&i8_kind, 129, true));
        let usize_kind = TyKind::Uint(UintTy::Usize);
        assert!(int_fits(&usize_kind, u128::from(u32::MAX)));
        assert!(!int_fits(&usize_kind, u128::from(u32::MAX) + 1));
        assert!(int_fits(&TyKind::Uint(UintTy::U128), u128::MAX));
    }
}
