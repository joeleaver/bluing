//! The names each module defines and imports.

use std::collections::BTreeMap;

use super::{PrimTy, Res, Visibility};
use crate::{DefId, Hir, ImportId, Name};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Namespace {
    Types,
    Values,
}

impl Namespace {
    pub const ALL: [Namespace; 2] = [Namespace::Types, Namespace::Values];
}

/// What a name in a module scope refers to. Primitive types can be bound
/// by imports: `use u8 as Byte;`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeDef {
    Def(DefId),
    PrimTy(PrimTy),
}

impl ScopeDef {
    pub fn res(self) -> Res {
        match self {
            ScopeDef::Def(def) => Res::Def(def),
            ScopeDef::PrimTy(prim) => Res::PrimTy(prim),
        }
    }
}

/// One name bound in a module.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Binding {
    pub def: ScopeDef,
    pub vis: Visibility,
    pub source: BindingSource,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BindingSource {
    /// An item defined in the module.
    Item,
    /// A single import.
    Import(ImportId),
    /// A glob import.
    Glob(ImportId),
}

/// Everything bound to one name in one namespace of a module.
#[derive(Clone, Debug, Default)]
pub struct NameBindings {
    /// An item or a single import. At most one; a second is a duplicate
    /// definition error.
    pub explicit: Option<Binding>,
    /// Glob-imported bindings, one per distinct definition. They are shadowed
    /// by `explicit`, and more than one is ambiguous where the name is used.
    pub globs: Vec<Binding>,
}

#[derive(Clone, Debug, Default)]
pub struct ModuleScope {
    types: BTreeMap<Name, NameBindings>,
    values: BTreeMap<Name, NameBindings>,
    /// Traits imported as `_`: in scope for method calls, but not nameable.
    pub unnamed_traits: Vec<Binding>,
}

impl ModuleScope {
    pub fn get(&self, ns: Namespace, name: &Name) -> Option<&NameBindings> {
        self.ns(ns).get(name)
    }

    pub fn iter(&self, ns: Namespace) -> impl Iterator<Item = (&Name, &NameBindings)> {
        self.ns(ns).iter()
    }

    fn ns(&self, ns: Namespace) -> &BTreeMap<Name, NameBindings> {
        match ns {
            Namespace::Types => &self.types,
            Namespace::Values => &self.values,
        }
    }

    fn ns_mut(&mut self, ns: Namespace) -> &mut BTreeMap<Name, NameBindings> {
        match ns {
            Namespace::Types => &mut self.types,
            Namespace::Values => &mut self.values,
        }
    }

    /// Defines an explicit binding. Returns the existing explicit binding
    /// instead if there is one, which is a duplicate definition.
    pub(super) fn define(
        &mut self,
        ns: Namespace,
        name: Name,
        binding: Binding,
    ) -> Result<(), Binding> {
        let entry = self.ns_mut(ns).entry(name).or_default();
        match entry.explicit {
            Some(existing) => Err(existing),
            None => {
                entry.explicit = Some(binding);
                Ok(())
            }
        }
    }

    /// Adds a glob binding. Returns whether anything changed: a new
    /// definition, or a wider visibility for one already present.
    pub(super) fn add_glob(
        &mut self,
        hir: &Hir,
        ns: Namespace,
        name: Name,
        binding: Binding,
    ) -> bool {
        let entry = self.ns_mut(ns).entry(name).or_default();
        match entry.globs.iter_mut().find(|b| b.def == binding.def) {
            Some(existing)
                if binding.vis.is_at_least(existing.vis, hir) && binding.vis != existing.vis =>
            {
                existing.vis = binding.vis;
                true
            }
            Some(_) => false,
            None => {
                entry.globs.push(binding);
                true
            }
        }
    }

    pub(super) fn add_unnamed_trait(&mut self, hir: &Hir, binding: Binding) -> bool {
        match self
            .unnamed_traits
            .iter_mut()
            .find(|b| b.def == binding.def)
        {
            Some(existing)
                if binding.vis.is_at_least(existing.vis, hir) && binding.vis != existing.vis =>
            {
                existing.vis = binding.vis;
                true
            }
            Some(_) => false,
            None => {
                self.unnamed_traits.push(binding);
                true
            }
        }
    }
}
