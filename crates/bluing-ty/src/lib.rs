//! Types, generics and item signatures of the Bluing compiler.
//!
//! [`collect`] lowers the types in every item's signature, checking what
//! needs types to check: lifetime elision, generic arguments, where `_` and
//! `!` may appear, unused parameters, and impl headers.

mod args;
mod coherence;
mod collect;
mod display;
pub mod fold;
pub mod generics;
pub mod layout;
pub mod lower;
mod params;
pub mod ty;

pub use coherence::{SimplifiedType, TraitRef};
pub use collect::ItemTypes;
pub use display::TyDisplay;

use std::collections::BTreeMap;

use bluing_base::Diagnostic;
use bluing_hir::{
    Analysis, CrateGraph, Files, GenericParamId, Hir, ImplId, Resolutions, TypeAliasId,
};
use la_arena::ArenaMap;

use crate::generics::Generics;
use crate::ty::{Interner, Ty};

/// The types of a compilation: the interner, every item's signature, and the
/// index of inherent impls.
pub struct TyCx<'a> {
    pub hir: &'a Hir,
    pub graph: &'a CrateGraph,
    pub files: &'a Files,
    pub resolutions: &'a Resolutions,
    pub interner: Interner,
    pub items: ItemTypes,
    pub impl_trait_refs: ArenaMap<ImplId, TraitRef>,
    pub inherent_impls: BTreeMap<SimplifiedType, Vec<ImplId>>,
    alias_state: ArenaMap<TypeAliasId, collect::AliasState>,
    default_state: BTreeMap<GenericParamId, collect::Lazy<ty::GenericArg>>,
    /// Diagnostics reported so far.
    pub diagnostics: Vec<Diagnostic>,
}

/// Lowers every item's signature.
pub fn collect<'a>(analysis: &'a Analysis, graph: &'a CrateGraph) -> TyCx<'a> {
    let mut cx = TyCx {
        hir: &analysis.hir,
        graph,
        files: &analysis.files,
        resolutions: &analysis.resolutions,
        interner: Interner::default(),
        items: ItemTypes::default(),
        impl_trait_refs: ArenaMap::default(),
        inherent_impls: BTreeMap::new(),
        alias_state: ArenaMap::default(),
        default_state: BTreeMap::new(),
        diagnostics: Vec::new(),
    };
    cx.collect_items();
    cx.check_impls();
    cx
}

impl TyCx<'_> {
    /// `ty` printed for a diagnostic, naming parameters from `generics`.
    pub fn display(&self, ty: Ty, generics: Option<&Generics>) -> String {
        TyDisplay {
            interner: &self.interner,
            hir: self.hir,
            generics,
            ty,
        }
        .to_string()
    }
}
