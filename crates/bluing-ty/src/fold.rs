//! Rebuilding types: substituting generic arguments for parameters, and any
//! other structural transformation.

use crate::ty::{Const, FnSig, GenericArg, Interner, Region, Substs, Ty, TyKind};

/// Transforms types bottom-up. Implementors override the parts they change;
/// the defaults rebuild the type unchanged.
pub trait TyFolder {
    fn interner(&mut self) -> &mut Interner;

    fn fold_ty(&mut self, ty: Ty) -> Ty {
        super_fold_ty(self, ty)
    }

    fn fold_region(&mut self, region: Region) -> Region {
        region
    }

    fn fold_const(&mut self, value: Const) -> Const {
        value
    }
}

/// Folds the parts of `ty` and rebuilds it.
pub fn super_fold_ty<F: TyFolder + ?Sized>(folder: &mut F, ty: Ty) -> Ty {
    let kind = folder.interner().kind(ty).clone();
    let folded = match kind {
        TyKind::Bool
        | TyKind::Char
        | TyKind::Str
        | TyKind::Never
        | TyKind::Int(_)
        | TyKind::Uint(_)
        | TyKind::Float(_)
        | TyKind::Param(_)
        | TyKind::Infer(_)
        | TyKind::Error => return ty,
        TyKind::Adt(id, substs) => TyKind::Adt(id, fold_substs(folder, substs)),
        TyKind::Ref(region, inner, mutability) => TyKind::Ref(
            folder.fold_region(region),
            folder.fold_ty(inner),
            mutability,
        ),
        TyKind::RawPtr(inner, mutability) => TyKind::RawPtr(folder.fold_ty(inner), mutability),
        TyKind::Array(elem, len) => TyKind::Array(folder.fold_ty(elem), folder.fold_const(len)),
        TyKind::Slice(elem) => TyKind::Slice(folder.fold_ty(elem)),
        TyKind::Tuple(list) => {
            let tys: Vec<Ty> = folder.interner().tys(list).to_vec();
            let tys = tys.into_iter().map(|ty| folder.fold_ty(ty)).collect();
            TyKind::Tuple(folder.interner().ty_list(tys))
        }
        TyKind::FnDef(id, substs) => TyKind::FnDef(id, fold_substs(folder, substs)),
        TyKind::FnPtr(sig) => TyKind::FnPtr(fold_sig(folder, sig)),
    };
    folder.interner().intern(folded)
}

pub fn fold_substs<F: TyFolder + ?Sized>(folder: &mut F, substs: Substs) -> Substs {
    let args: Vec<GenericArg> = folder.interner().args(substs).to_vec();
    let args = args
        .into_iter()
        .map(|arg| match arg {
            GenericArg::Ty(ty) => GenericArg::Ty(folder.fold_ty(ty)),
            GenericArg::Region(region) => GenericArg::Region(folder.fold_region(region)),
            GenericArg::Const(value) => GenericArg::Const(folder.fold_const(value)),
        })
        .collect();
    folder.interner().substs(args)
}

pub fn fold_sig<F: TyFolder + ?Sized>(folder: &mut F, sig: FnSig) -> FnSig {
    let tys: Vec<Ty> = folder.interner().tys(sig.inputs_and_output).to_vec();
    let tys = tys.into_iter().map(|ty| folder.fold_ty(ty)).collect();
    FnSig {
        inputs_and_output: folder.interner().ty_list(tys),
        is_unsafe: sig.is_unsafe,
    }
}

/// Replaces generic parameters with the corresponding arguments.
pub struct Subst<'a> {
    pub interner: &'a mut Interner,
    pub args: Vec<GenericArg>,
}

impl<'a> Subst<'a> {
    pub fn new(interner: &'a mut Interner, substs: Substs) -> Self {
        let args = interner.args(substs).to_vec();
        Subst { interner, args }
    }
}

impl TyFolder for Subst<'_> {
    fn interner(&mut self) -> &mut Interner {
        self.interner
    }

    fn fold_ty(&mut self, ty: Ty) -> Ty {
        match *self.interner.kind(ty) {
            TyKind::Param(index) => match self.args.get(index.0 as usize) {
                Some(GenericArg::Ty(arg)) => *arg,
                // Arguments are built to match the parameters, so this means a
                // diagnostic was already reported for them.
                _ => self.interner.error(),
            },
            _ => super_fold_ty(self, ty),
        }
    }

    fn fold_region(&mut self, region: Region) -> Region {
        match region {
            Region::Param(index) => match self.args.get(index.0 as usize) {
                Some(GenericArg::Region(arg)) => *arg,
                _ => Region::Error,
            },
            other => other,
        }
    }

    fn fold_const(&mut self, value: Const) -> Const {
        match value {
            Const::Param(index) => match self.args.get(index.0 as usize) {
                Some(GenericArg::Const(arg)) => *arg,
                _ => Const::Error,
            },
            other => other,
        }
    }
}

/// `ty` with `substs` substituted for its generic parameters.
pub fn subst(interner: &mut Interner, ty: Ty, substs: Substs) -> Ty {
    Subst::new(interner, substs).fold_ty(ty)
}
