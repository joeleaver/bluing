//! Lowering HIR types to types, with the checks that need them: lifetime
//! elision, generic argument counts, and where `_` and `!` may appear.

use bluing_base::{Code, Diagnostic, FileId, TextRange};
use bluing_hir::body::Body;
use bluing_hir::hir::{LifetimeRef, Path, TypeRef, TypeRefKind};
use bluing_hir::resolve::{LifetimeRes, PathRes, PrimTy, Res, SelfTyOf};
use bluing_hir::{DefId, GenericOwner, Resolutions};

use crate::TyCx;
use crate::generics::{Generics, generics_of, param_index};
use crate::ty::{
    AdtId, FloatTy, FnSig, IntTy, Interner, Mutability, ParamIndex, Region, Ty, TyKind, UintTy,
};

/// Where the resolutions of the paths being lowered are.
#[derive(Clone, Copy)]
pub enum PathSource<'a> {
    Signature(&'a Resolutions),
    Body(&'a Body),
}

impl PathSource<'_> {
    pub fn path(&self, path: &Path) -> Option<PathRes> {
        match self {
            PathSource::Signature(res) => res.path(path.id),
            PathSource::Body(body) => body.path_res(path),
        }
    }

    pub fn lifetime(&self, lifetime: &LifetimeRef) -> Option<LifetimeRes> {
        match self {
            PathSource::Signature(res) => res.lifetime(lifetime.id),
            PathSource::Body(body) => body
                .lifetimes
                .get(lifetime.id.0 as usize)
                .copied()
                .flatten(),
        }
    }
}

/// What an elided lifetime (`&T`, `'_`, or a path omitting its lifetime
/// arguments) means here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionMode {
    /// An error: struct fields, type aliases.
    Forbidden,
    /// `'static`: constants and statics.
    Static,
    /// Function parameters: each is a new anonymous lifetime.
    FnInputs,
    /// A function's return type: the elided lifetime chosen by the elision
    /// rules, or an error if they choose none.
    FnOutput(Option<Region>),
    /// Impl headers: each is a new anonymous lifetime.
    Fresh,
    /// Inside bodies, where lifetimes are left for borrow checking.
    Erased,
}

/// What `_` means here.
pub enum Placeholder<'a> {
    /// An error, in item signatures. Names the kind of signature.
    Forbidden(&'static str),
    /// A new inference variable, in bodies.
    Infer(&'a mut dyn FnMut(&mut Interner) -> Ty),
}

/// The context a type is lowered in.
pub struct LowerCtx<'a> {
    pub file: FileId,
    pub paths: PathSource<'a>,
    /// The generic parameters in scope, for constants like array lengths.
    pub generics: Option<&'a Generics>,
    /// The self type of the enclosing impl.
    pub self_ty: Option<Ty>,
    pub regions: RegionMode,
    pub placeholder: Placeholder<'a>,
    /// The lifetimes that appear in a function's parameters.
    pub input_regions: Vec<Region>,
    pub next_anon: u32,
}

impl<'a> LowerCtx<'a> {
    pub fn new(
        file: FileId,
        paths: PathSource<'a>,
        generics: Option<&'a Generics>,
        regions: RegionMode,
        placeholder: Placeholder<'a>,
    ) -> Self {
        LowerCtx {
            file,
            paths,
            generics,
            self_ty: None,
            regions,
            placeholder,
            input_regions: Vec::new(),
            next_anon: 0,
        }
    }
}

impl TyCx<'_> {
    pub(crate) fn error(
        &mut self,
        file: FileId,
        range: TextRange,
        code: Code,
        message: impl Into<String>,
    ) {
        self.diagnostics
            .push(Diagnostic::new(file, range, code, message));
    }

    /// Lowers a function's return type, where `!` is allowed.
    pub fn lower_ret_ty(&mut self, cx: &mut LowerCtx<'_>, ty: Option<&TypeRef>) -> Ty {
        match ty {
            None => self.interner.unit(),
            Some(TypeRef {
                kind: TypeRefKind::Never,
                ..
            }) => self.interner.never(),
            Some(ty) => self.lower_ty(cx, ty),
        }
    }

    pub fn lower_ty(&mut self, cx: &mut LowerCtx<'_>, ty: &TypeRef) -> Ty {
        let range = ty.range;
        let kind = match &ty.kind {
            TypeRefKind::Path(path) => return self.lower_path_ty(cx, path),
            TypeRefKind::Reference {
                lifetime,
                mutable,
                inner,
            } => {
                let region = self.region(cx, lifetime.as_ref(), range);
                let inner = self.lower_ty(cx, inner);
                TyKind::Ref(region, inner, mutability(*mutable))
            }
            TypeRefKind::RawPtr { mutable, inner } => {
                TyKind::RawPtr(self.lower_ty(cx, inner), mutability(*mutable))
            }
            TypeRefKind::Slice(elem) => TyKind::Slice(self.lower_ty(cx, elem)),
            TypeRefKind::Array { elem, len } => {
                let elem = self.lower_ty(cx, elem);
                let len = match len {
                    Some(len) => {
                        let usize_ty = self.usize_ty();
                        self.lower_const_expr(cx, len, usize_ty)
                    }
                    None => crate::ty::Const::Error,
                };
                TyKind::Array(elem, len)
            }
            TypeRefKind::Tuple(tys) => {
                let tys = tys.iter().map(|ty| self.lower_ty(cx, ty)).collect();
                TyKind::Tuple(self.interner.ty_list(tys))
            }
            TypeRefKind::Never => {
                // Only a whole return type may be `!`; elsewhere it is the
                // unstable never type.
                let message = "the never type `!` can only be used as a return type";
                self.error(cx.file, range, Code::Unsupported, message);
                TyKind::Error
            }
            TypeRefKind::FnPtr {
                params,
                ret,
                is_unsafe,
            } => return self.lower_fn_ptr(cx, params, ret, *is_unsafe),
            TypeRefKind::ImplTrait(_) => {
                let message = "`impl Trait` types are not supported yet";
                self.error(cx.file, range, Code::NotYetSupported, message);
                TyKind::Error
            }
            TypeRefKind::DynTrait(_) => {
                let message = "`dyn Trait` types are not supported yet";
                self.error(cx.file, range, Code::NotYetSupported, message);
                TyKind::Error
            }
            TypeRefKind::Infer => match &mut cx.placeholder {
                Placeholder::Infer(fresh) => return fresh(&mut self.interner),
                Placeholder::Forbidden(what) => {
                    let message =
                        format!("the placeholder `_` is not allowed in the signatures of {what}");
                    self.error(cx.file, range, Code::Placeholder, message);
                    TyKind::Error
                }
            },
            TypeRefKind::Error => TyKind::Error,
        };
        self.interner.intern(kind)
    }

    /// A function pointer type has its own elision scope.
    fn lower_fn_ptr(
        &mut self,
        cx: &mut LowerCtx<'_>,
        params: &[TypeRef],
        ret: &TypeRef,
        is_unsafe: bool,
    ) -> Ty {
        let outer_regions = std::mem::replace(&mut cx.regions, RegionMode::FnInputs);
        let outer_inputs = std::mem::take(&mut cx.input_regions);
        let mut tys: Vec<Ty> = params.iter().map(|ty| self.lower_ty(cx, ty)).collect();
        cx.regions = RegionMode::FnOutput(single_region(&cx.input_regions));
        let output = match &ret.kind {
            TypeRefKind::Never => self.interner.never(),
            _ => self.lower_ty(cx, ret),
        };
        tys.push(output);
        cx.regions = outer_regions;
        cx.input_regions = outer_inputs;
        let sig = FnSig {
            inputs_and_output: self.interner.ty_list(tys),
            is_unsafe,
        };
        self.interner.intern(TyKind::FnPtr(sig))
    }

    /// The region of a written lifetime, or of an elided one at `range`.
    pub fn region(
        &mut self,
        cx: &mut LowerCtx<'_>,
        lifetime: Option<&LifetimeRef>,
        range: TextRange,
    ) -> Region {
        let region = match lifetime.map(|lt| (lt, cx.paths.lifetime(lt))) {
            None => return self.elided_region(cx, range),
            Some((lt, Some(LifetimeRes::Anonymous))) => return self.elided_region(cx, lt.range),
            Some((_, Some(LifetimeRes::Static))) => Region::Static,
            Some((_, Some(LifetimeRes::Param(id)))) => Region::Param(param_index(self.hir, id)),
            Some((_, Some(LifetimeRes::Error) | None)) => Region::Error,
        };
        if cx.regions == RegionMode::FnInputs {
            cx.input_regions.push(region);
        }
        region
    }

    pub fn elided_region(&mut self, cx: &mut LowerCtx<'_>, range: TextRange) -> Region {
        match cx.regions {
            RegionMode::Forbidden => {
                self.error(
                    cx.file,
                    range,
                    Code::MissingLifetime,
                    "missing lifetime specifier",
                );
                Region::Error
            }
            RegionMode::Static => Region::Static,
            RegionMode::FnInputs => {
                let region = Region::Anon(cx.next_anon);
                cx.next_anon += 1;
                cx.input_regions.push(region);
                region
            }
            RegionMode::FnOutput(Some(region)) => region,
            RegionMode::FnOutput(None) => {
                let message = "missing lifetime specifier: the return type borrows from a \
                               parameter, but the signature does not say which one";
                self.error(cx.file, range, Code::MissingLifetime, message);
                Region::Error
            }
            RegionMode::Fresh => {
                let region = Region::Anon(cx.next_anon);
                cx.next_anon += 1;
                region
            }
            RegionMode::Erased => Region::Erased,
        }
    }

    fn lower_path_ty(&mut self, cx: &mut LowerCtx<'_>, path: &Path) -> Ty {
        let Some(res) = cx.paths.path(path) else {
            return self.interner.error();
        };
        if res.unresolved_segments > 0 {
            let message = "associated types are not supported yet";
            self.error(cx.file, path.range, Code::NotYetSupported, message);
            return self.interner.error();
        }
        let Some((last, init)) = path.segments.split_last() else {
            return self.interner.error();
        };
        for segment in init.iter().filter(|segment| segment.args.is_some()) {
            let message = format!("generic arguments are not allowed on `{}`", segment.name);
            self.error(cx.file, segment.range, Code::GenericArgs, message);
        }
        let no_args = |this: &mut Self, what: &str| {
            if last.args.is_some() {
                let message = format!("generic arguments are not allowed on {what}");
                this.error(cx.file, last.range, Code::GenericArgs, message);
            }
        };
        let kind = match res.base {
            Res::PrimTy(prim) => {
                no_args(self, "a primitive type");
                prim_kind(prim)
            }
            Res::Def(DefId::Struct(id)) => {
                let substs = self.generic_args(
                    cx,
                    GenericOwner::Struct(id),
                    last.args.as_ref(),
                    last.range,
                    Vec::new(),
                );
                TyKind::Adt(AdtId::Struct(id), substs)
            }
            Res::Def(DefId::Enum(id)) => {
                let substs = self.generic_args(
                    cx,
                    GenericOwner::Enum(id),
                    last.args.as_ref(),
                    last.range,
                    Vec::new(),
                );
                TyKind::Adt(AdtId::Enum(id), substs)
            }
            Res::Def(DefId::TypeAlias(id)) => {
                let substs = self.generic_args(
                    cx,
                    GenericOwner::TypeAlias(id),
                    last.args.as_ref(),
                    last.range,
                    Vec::new(),
                );
                let ty = self.alias_ty(id);
                return crate::fold::subst(&mut self.interner, ty, substs);
            }
            Res::SelfTy(self_ty) => {
                no_args(self, "`Self`");
                match self_ty {
                    SelfTyOf::Struct(id) => {
                        let substs = self.identity_substs(GenericOwner::Struct(id));
                        TyKind::Adt(AdtId::Struct(id), substs)
                    }
                    SelfTyOf::Enum(id) => {
                        let substs = self.identity_substs(GenericOwner::Enum(id));
                        TyKind::Adt(AdtId::Enum(id), substs)
                    }
                    SelfTyOf::Impl(id) => {
                        return cx.self_ty.unwrap_or_else(|| self.impl_self_ty(id));
                    }
                    // A trait's `Self` is its implicit first parameter.
                    SelfTyOf::Trait(_) => TyKind::Param(ParamIndex(0)),
                }
            }
            Res::GenericParam(id) => {
                no_args(self, "a type parameter");
                TyKind::Param(param_index(self.hir, id))
            }
            // Anything else was reported as the wrong kind during resolution.
            Res::Def(_) | Res::QualifiedSelf | Res::Error => TyKind::Error,
        };
        self.interner.intern(kind)
    }

    /// The identity arguments of an item: each parameter as itself.
    pub fn identity_substs(&mut self, owner: GenericOwner) -> crate::ty::Substs {
        let generics = generics_of(self.hir, owner);
        let args = generics
            .params
            .iter()
            .enumerate()
            .map(|(index, param)| {
                let index = ParamIndex(index as u32);
                match param.kind {
                    crate::generics::ParamKind::Lifetime => {
                        crate::ty::GenericArg::Region(Region::Param(index))
                    }
                    crate::generics::ParamKind::Type { .. } => {
                        crate::ty::GenericArg::Ty(self.interner.intern(TyKind::Param(index)))
                    }
                    crate::generics::ParamKind::Const { .. } => {
                        crate::ty::GenericArg::Const(crate::ty::Const::Param(index))
                    }
                }
            })
            .collect();
        self.interner.substs(args)
    }
}

/// The lifetime of a function's parameters if they have exactly one lifetime
/// position. Rust counts positions, not distinct lifetimes: two uses of the
/// same named lifetime leave an elided return lifetime ambiguous.
pub(crate) fn single_region(regions: &[Region]) -> Option<Region> {
    match regions {
        [region] => Some(*region),
        _ => None,
    }
}

fn mutability(mutable: bool) -> Mutability {
    if mutable {
        Mutability::Mut
    } else {
        Mutability::Not
    }
}

fn prim_kind(prim: PrimTy) -> TyKind {
    match prim {
        PrimTy::Bool => TyKind::Bool,
        PrimTy::Char => TyKind::Char,
        PrimTy::Str => TyKind::Str,
        PrimTy::I8 => TyKind::Int(IntTy::I8),
        PrimTy::I16 => TyKind::Int(IntTy::I16),
        PrimTy::I32 => TyKind::Int(IntTy::I32),
        PrimTy::I64 => TyKind::Int(IntTy::I64),
        PrimTy::I128 => TyKind::Int(IntTy::I128),
        PrimTy::Isize => TyKind::Int(IntTy::Isize),
        PrimTy::U8 => TyKind::Uint(UintTy::U8),
        PrimTy::U16 => TyKind::Uint(UintTy::U16),
        PrimTy::U32 => TyKind::Uint(UintTy::U32),
        PrimTy::U64 => TyKind::Uint(UintTy::U64),
        PrimTy::U128 => TyKind::Uint(UintTy::U128),
        PrimTy::Usize => TyKind::Uint(UintTy::Usize),
        PrimTy::F32 => TyKind::Float(FloatTy::F32),
        PrimTy::F64 => TyKind::Float(FloatTy::F64),
    }
}
