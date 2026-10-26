//! Literals and their suffixes.

use bluing_base::Code;
use bluing_syntax::ra_ap_syntax::ast;

#[derive(Clone, Debug, PartialEq)]
pub enum Literal {
    Bool(bool),
    Char(char),
    /// `b'a'`
    Byte(u8),
    Int {
        value: u128,
        suffix: Option<IntSuffix>,
    },
    Float {
        /// The digits as written, without `_` separators or suffix, so later
        /// stages can round them exactly to `f32` or `f64`.
        text: String,
        suffix: Option<FloatSuffix>,
    },
    Str(String),
    /// `b"..."`
    ByteStr(Vec<u8>),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IntSuffix {
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatSuffix {
    F32,
    F64,
}

impl IntSuffix {
    fn from_str(suffix: &str) -> Option<IntSuffix> {
        Some(match suffix {
            "i8" => IntSuffix::I8,
            "i16" => IntSuffix::I16,
            "i32" => IntSuffix::I32,
            "i64" => IntSuffix::I64,
            "i128" => IntSuffix::I128,
            "isize" => IntSuffix::Isize,
            "u8" => IntSuffix::U8,
            "u16" => IntSuffix::U16,
            "u32" => IntSuffix::U32,
            "u64" => IntSuffix::U64,
            "u128" => IntSuffix::U128,
            "usize" => IntSuffix::Usize,
            _ => return None,
        })
    }
}

impl FloatSuffix {
    fn from_str(suffix: &str) -> Option<FloatSuffix> {
        match suffix {
            "f32" => Some(FloatSuffix::F32),
            "f64" => Some(FloatSuffix::F64),
            _ => None,
        }
    }
}

/// Why a literal could not be lowered. `Reported` means the parser already
/// reported the problem (an invalid escape, for example).
pub enum LiteralError {
    Reported,
    Error(Code, String),
}

/// Lowers a literal, for callers outside bodies (constants in types).
pub fn lower_literal(literal: &ast::Literal) -> Result<Literal, LiteralError> {
    lower(literal)
}

pub(super) fn lower(literal: &ast::Literal) -> Result<Literal, LiteralError> {
    match literal.kind() {
        ast::LiteralKind::Bool(value) => Ok(Literal::Bool(value)),
        ast::LiteralKind::Char(c) => c
            .value()
            .map(Literal::Char)
            .map_err(|_| LiteralError::Reported),
        ast::LiteralKind::Byte(b) => b
            .value()
            .map(Literal::Byte)
            .map_err(|_| LiteralError::Reported),
        ast::LiteralKind::String(s) => s
            .value()
            .map(|value| Literal::Str(value.into_owned()))
            .map_err(|_| LiteralError::Reported),
        ast::LiteralKind::ByteString(s) => s
            .value()
            .map(|value| Literal::ByteStr(value.into_owned()))
            .map_err(|_| LiteralError::Reported),
        ast::LiteralKind::CString(_) => Err(LiteralError::Error(
            Code::NotYetSupported,
            "C string literals are not supported yet".to_owned(),
        )),
        ast::LiteralKind::IntNumber(number) => {
            let suffix = number.suffix();
            if let Some(float) = suffix.and_then(FloatSuffix::from_str) {
                // `1f32` is a float literal written without a fraction. Only
                // decimal digits can take a float suffix.
                let (prefix, digits, _) = number.split_into_parts();
                if !prefix.is_empty() {
                    return Err(LiteralError::Error(
                        Code::Syntax,
                        "binary, octal and hexadecimal literals cannot have a float suffix"
                            .to_owned(),
                    ));
                }
                return Ok(Literal::Float {
                    text: digits.replace('_', ""),
                    suffix: Some(float),
                });
            }
            let suffix = match suffix {
                None => None,
                Some(text) => match IntSuffix::from_str(text) {
                    Some(suffix) => Some(suffix),
                    None => {
                        let message = format!("invalid suffix `{text}` for an integer literal");
                        return Err(LiteralError::Error(Code::Syntax, message));
                    }
                },
            };
            match number.value() {
                Ok(value) => Ok(Literal::Int { value, suffix }),
                Err(_) => Err(LiteralError::Error(
                    Code::Syntax,
                    "integer literal is too large".to_owned(),
                )),
            }
        }
        ast::LiteralKind::FloatNumber(number) => {
            let (digits, suffix) = number.split_into_parts();
            let suffix = match suffix {
                "" => None,
                text => match FloatSuffix::from_str(text) {
                    Some(suffix) => Some(suffix),
                    None => {
                        let message = format!("invalid suffix `{text}` for a float literal");
                        return Err(LiteralError::Error(Code::Syntax, message));
                    }
                },
            };
            Ok(Literal::Float {
                text: digits.replace('_', ""),
                suffix,
            })
        }
    }
}
