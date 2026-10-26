//! A read-only view of the module system for looking names up.
//!
//! While imports are still being resolved, a lookup may be *undetermined*:
//! the module still has an import that could bind the name, so the answer
//! might change. Callers pass `force` once no import can make progress, which
//! turns undetermined into not found. Once resolution has finished
//! ([`NameEnv::finished`]) every lookup is determined.

use std::collections::BTreeMap;

use la_arena::ArenaMap;

use super::imports::{GlobLink, GlobSource};
use super::{
    Binding, BindingSource, ModuleScope, Namespace, PrimTy, Res, Resolutions, ScopeDef, Visibility,
};
use crate::hir::{Fields, Hir, ImportKind};
use crate::{CrateGraph, CrateId, DefId, EnumId, ImportId, ModuleId, Name};

pub(crate) enum Lookup<T> {
    Found(T),
    /// More than one definition, and nothing to prefer one.
    Ambiguous,
    NotFound,
    Undetermined,
}

pub(crate) struct NameEnv<'a> {
    pub hir: &'a Hir,
    pub graph: &'a CrateGraph,
    pub crate_roots: &'a ArenaMap<CrateId, ModuleId>,
    pub scopes: &'a ArenaMap<ModuleId, ModuleScope>,
    pub preludes: &'a ArenaMap<CrateId, ModuleId>,
    pub def_visibilities: &'a BTreeMap<DefId, Visibility>,
    /// `None` once import resolution has finished.
    pub progress: Option<ImportProgress<'a>>,
}

/// The state of import resolution, which decides whether a lookup is
/// determined yet.
#[derive(Clone, Copy)]
pub(crate) struct ImportProgress<'a> {
    /// Imports not resolved yet.
    pub pending: &'a [ImportId],
    /// The import being resolved, which cannot bind itself.
    pub resolving: Option<ImportId>,
    pub glob_links: &'a [GlobLink],
}

impl<'a> NameEnv<'a> {
    /// The environment after name resolution has finished.
    pub fn finished(hir: &'a Hir, graph: &'a CrateGraph, resolutions: &'a Resolutions) -> Self {
        NameEnv {
            hir,
            graph,
            crate_roots: &resolutions.crate_roots,
            scopes: &resolutions.scopes,
            preludes: &resolutions.preludes,
            def_visibilities: &resolutions.def_visibilities,
            progress: None,
        }
    }

    pub fn crate_of(&self, module: ModuleId) -> CrateId {
        self.hir.modules[module].krate
    }

    /// The root of the crate called `name` that `module`'s crate depends on.
    pub fn extern_crate(&self, module: ModuleId, name: &Name) -> Option<ModuleId> {
        let krate = self.crate_of(module);
        self.graph[krate]
            .deps
            .iter()
            .find(|&&dep| &self.graph[dep].name == name)
            .and_then(|&dep| self.crate_roots.get(dep).copied())
    }

    /// Looks `name` up in the scope of `module` (its items and imports).
    pub fn lookup_in_module(
        &self,
        module: ModuleId,
        name: &Name,
        ns: Namespace,
        force: bool,
    ) -> Lookup<Binding> {
        let bindings = self.scopes[module].get(ns, name);
        if let Some(binding) = bindings.and_then(|b| b.explicit) {
            return Lookup::Found(binding);
        }
        if !force && (self.has_pending_single(module, name) || !self.is_settled(module)) {
            return Lookup::Undetermined;
        }
        match bindings.map_or(&[][..], |b| b.globs.as_slice()) {
            [] => Lookup::NotFound,
            [binding] => Lookup::Found(*binding),
            _ => Lookup::Ambiguous,
        }
    }

    /// Looks up the first segment of a path written in `module`, through the
    /// module's scope, the extern prelude, the standard prelude and the
    /// primitive types. Generic parameters, `Self` and local variables are
    /// handled by the caller.
    pub fn lookup_lexical(
        &self,
        module: ModuleId,
        name: &Name,
        ns: Namespace,
        force: bool,
        in_use_path: bool,
    ) -> Lookup<Res> {
        let bindings = self.scopes[module].get(ns, name);
        if let Some(binding) = bindings.and_then(|b| b.explicit) {
            return Lookup::Found(binding.def.res());
        }
        if !force && (self.has_pending_single(module, name) || !self.is_settled(module)) {
            return Lookup::Undetermined;
        }
        let globs = bindings.map_or(&[][..], |b| b.globs.as_slice());
        if globs.is_empty() {
            return self.lookup_outer(module, name, ns, force);
        }
        if in_use_path {
            // In a `use` path, a glob does not shadow the outer scopes; a
            // name that both provide is ambiguous.
            match self.lookup_outer(module, name, ns, force) {
                Lookup::NotFound => {}
                Lookup::Undetermined => return Lookup::Undetermined,
                Lookup::Found(_) | Lookup::Ambiguous => return Lookup::Ambiguous,
            }
        }
        match globs {
            [binding] => Lookup::Found(binding.def.res()),
            _ => Lookup::Ambiguous,
        }
    }

    /// The scopes outside a module: extern crates, the standard prelude,
    /// primitive types.
    fn lookup_outer(
        &self,
        module: ModuleId,
        name: &Name,
        ns: Namespace,
        force: bool,
    ) -> Lookup<Res> {
        if ns == Namespace::Types
            && let Some(root) = self.extern_crate(module, name)
        {
            return Lookup::Found(Res::Def(DefId::Module(root)));
        }
        if let Some(&prelude) = self.preludes.get(self.crate_of(module)) {
            match self.lookup_in_module(prelude, name, ns, force) {
                Lookup::Found(binding) if binding.vis.is_accessible_from(self.hir, module) => {
                    return Lookup::Found(binding.def.res());
                }
                Lookup::Found(_) | Lookup::NotFound => {}
                Lookup::Ambiguous => return Lookup::Ambiguous,
                Lookup::Undetermined => return Lookup::Undetermined,
            }
        }
        if ns == Namespace::Types
            && let Some(prim) = PrimTy::from_name(name)
        {
            return Lookup::Found(Res::PrimTy(prim));
        }
        Lookup::NotFound
    }

    /// A variant of `e` named `name` in namespace `ns`: every variant is in
    /// the type namespace, tuple and unit variants are also in the value one.
    pub fn variant_binding(&self, e: EnumId, name: &Name, ns: Namespace) -> Option<Binding> {
        let variant = self.hir.enums[e]
            .variants
            .iter()
            .copied()
            .find(|&v| &self.hir.variants[v].name == name)?;
        let is_value = !matches!(self.hir.variants[variant].fields, Fields::Record(_));
        (ns == Namespace::Types || is_value).then(|| Binding {
            def: ScopeDef::Def(DefId::Variant(variant)),
            vis: self.def_visibilities[&DefId::Variant(variant)],
            source: BindingSource::Item,
        })
    }

    /// Whether `module` has an unresolved single import that binds `name`.
    /// The import being resolved does not count: it cannot bind itself.
    fn has_pending_single(&self, module: ModuleId, name: &Name) -> bool {
        let Some(progress) = self.progress else {
            return false;
        };
        progress.pending.iter().any(|&id| {
            let import = &self.hir.imports[id];
            Some(id) != progress.resolving
                && import.module == module
                && matches!(&import.kind, ImportKind::Single { name: Some(n), .. } if n == name)
        })
    }

    /// Whether no import can still add names to `module`: neither the module
    /// nor any module it glob-imports from (transitively) has unresolved
    /// imports (other than the one being resolved).
    fn is_settled(&self, module: ModuleId) -> bool {
        let Some(progress) = self.progress else {
            return true;
        };
        let mut stack = vec![module];
        let mut seen = vec![module];
        while let Some(current) = stack.pop() {
            let pending_here = progress.pending.iter().any(|&id| {
                Some(id) != progress.resolving && self.hir.imports[id].module == current
            });
            if pending_here {
                return false;
            }
            for link in progress
                .glob_links
                .iter()
                .filter(|link| link.importing == current)
            {
                if let GlobSource::Module(source) = link.source
                    && !seen.contains(&source)
                {
                    seen.push(source);
                    stack.push(source);
                }
            }
        }
        true
    }
}
