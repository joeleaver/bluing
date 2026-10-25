//! Looking names up in modules and in lexical scope.
//!
//! While imports are still being resolved, a lookup may be *undetermined*:
//! the module still has an import that could bind the name, so the answer
//! might change. Callers pass `force` once no import can make progress, which
//! turns undetermined into not found.

use super::{Binding, Namespace, PrimTy, Res, Resolver};
use crate::hir::ImportKind;
use crate::{DefId, ModuleId, Name};

pub(super) enum Lookup<T> {
    Found(T),
    /// More than one definition, and nothing to prefer one.
    Ambiguous,
    NotFound,
    Undetermined,
}

impl Resolver<'_> {
    /// Looks `name` up in the scope of `module` (its items and imports).
    pub(super) fn lookup_in_module(
        &self,
        module: ModuleId,
        name: &Name,
        ns: Namespace,
        force: bool,
    ) -> Lookup<Binding> {
        let bindings = self.out.scopes[module].get(ns, name);
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
    /// primitive types. Generic parameters and `Self` are handled by the
    /// caller, since only signatures can see them.
    pub(super) fn lookup_lexical(
        &self,
        module: ModuleId,
        name: &Name,
        ns: Namespace,
        force: bool,
        in_use_path: bool,
    ) -> Lookup<Res> {
        let bindings = self.out.scopes[module].get(ns, name);
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

    /// Whether `module` has an unresolved single import that binds `name`.
    /// The import being resolved does not count: it cannot bind itself.
    fn has_pending_single(&self, module: ModuleId, name: &Name) -> bool {
        self.pending.iter().any(|&id| {
            let import = &self.hir.imports[id];
            Some(id) != self.resolving
                && import.module == module
                && matches!(&import.kind, ImportKind::Single { name: Some(n), .. } if n == name)
        })
    }

    /// Whether no import can still add names to `module`: neither the module
    /// nor any module it glob-imports from (transitively) has unresolved
    /// imports (other than the one being resolved).
    pub(super) fn is_settled(&self, module: ModuleId) -> bool {
        let mut stack = vec![module];
        let mut seen = vec![module];
        while let Some(current) = stack.pop() {
            let pending_here = self
                .pending
                .iter()
                .any(|&id| Some(id) != self.resolving && self.hir.imports[id].module == current);
            if pending_here {
                return false;
            }
            for link in self
                .glob_links
                .iter()
                .filter(|link| link.importing == current)
            {
                if let super::imports::GlobSource::Module(source) = link.source
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
