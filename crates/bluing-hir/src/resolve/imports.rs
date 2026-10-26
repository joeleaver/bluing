//! Import resolution, run to a fixed point. See "Name resolution" in DESIGN.md.

use bluing_base::{Code, TextRange};

use super::env::Lookup;
use super::{Binding, BindingSource, Namespace, Res, Resolver, ScopeDef, Visibility};
use crate::hir::{Import, ImportKind, ModPath, PathKind};
use crate::{DefId, EnumId, ImportId, ModuleId, Name};

/// A resolved glob import: everything visible in `source` is imported into
/// `importing`, at most as visible as `vis`.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GlobLink {
    pub importing: ModuleId,
    pub source: GlobSource,
    pub import: ImportId,
    pub vis: Visibility,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GlobSource {
    Module(ModuleId),
    Enum(EnumId),
}

/// The outcome of trying to resolve part of an import.
enum Step<T> {
    Found(T),
    /// The answer depends on imports not resolved yet.
    Undetermined,
    /// An error was reported.
    Failed,
}

impl Resolver<'_> {
    pub(super) fn resolve_imports(&mut self) {
        loop {
            let mut progress = false;
            for id in self.pending.clone() {
                if self.try_import(id, false) {
                    self.pending.retain(|&pending| pending != id);
                    self.propagate_globs();
                    progress = true;
                }
            }
            if !progress {
                break;
            }
        }
        // No import can make progress: whatever is left is resolved as far as
        // possible, treating undetermined names as missing.
        while let Some(&id) = self.pending.first() {
            self.try_import(id, true);
            self.pending.remove(0);
            self.propagate_globs();
        }
    }

    /// Tries to resolve one import. Returns `false` if it has to wait.
    fn try_import(&mut self, id: ImportId, force: bool) -> bool {
        self.resolving = Some(id);
        let done = self.try_import_inner(id, force);
        self.resolving = None;
        done
    }

    fn try_import_inner(&mut self, id: ImportId, force: bool) -> bool {
        let hir = self.hir;
        let import = &hir.imports[id];
        match &import.kind {
            ImportKind::Glob => match self.walk(import, import.path.segments.len(), force) {
                Step::Undetermined => false,
                Step::Failed => true,
                Step::Found(container) => {
                    let source = match container {
                        DefId::Module(module) => GlobSource::Module(module),
                        DefId::Enum(e) => GlobSource::Enum(e),
                        _ => {
                            let message = "glob imports need a module or an enum";
                            self.error(import.file, import.range, Code::WrongKind, message);
                            return true;
                        }
                    };
                    let vis = self.import_vis[id];
                    self.glob_links.push(GlobLink {
                        importing: import.module,
                        source,
                        import: id,
                        vis,
                    });
                    true
                }
            },
            ImportKind::Single { name, is_self } => {
                let namespaces: &[Namespace] = if *is_self {
                    &[Namespace::Types]
                } else {
                    &Namespace::ALL
                };
                let found = match self.resolve_single(import, namespaces, force) {
                    Step::Found(found) => found,
                    Step::Undetermined => return false,
                    Step::Failed => return true,
                };
                self.bind_import(id, name.as_ref(), found);
                true
            }
        }
    }

    /// Resolves the target of a single import in each namespace.
    fn resolve_single(
        &mut self,
        import: &Import,
        namespaces: &[Namespace],
        force: bool,
    ) -> Step<Vec<(Namespace, Binding)>> {
        let path = &import.path;
        let Some((target, target_range)) = path.segments.last() else {
            // `use crate as name;` and the like import a module itself.
            return match self.walk(import, 0, force) {
                Step::Found(def) => Step::Found(vec![(Namespace::Types, self.item_binding(def))]),
                Step::Undetermined => Step::Undetermined,
                Step::Failed => Step::Failed,
            };
        };
        let mut found = Vec::new();
        if path.segments.len() == 1 && path.kind == PathKind::Plain {
            // `use name;`: the name itself is looked up lexically.
            for &ns in namespaces {
                match self
                    .env()
                    .lookup_lexical(import.module, target, ns, force, true)
                {
                    Lookup::Found(Res::Def(def)) => found.push((ns, self.item_binding(def))),
                    Lookup::Found(Res::PrimTy(prim)) => found.push((
                        ns,
                        Binding {
                            def: ScopeDef::PrimTy(prim),
                            vis: Visibility::Public,
                            source: BindingSource::Item,
                        },
                    )),
                    Lookup::Found(_) | Lookup::NotFound => {}
                    Lookup::Ambiguous => return self.ambiguous(import.file, *target_range, target),
                    Lookup::Undetermined => return Step::Undetermined,
                }
            }
        } else {
            let container = match self.walk(import, path.segments.len() - 1, force) {
                Step::Found(def) => def,
                Step::Undetermined => return Step::Undetermined,
                Step::Failed => return Step::Failed,
            };
            for &ns in namespaces {
                match self.lookup_in_container(import, container, target, *target_range, ns, force)
                {
                    Step::Found(Some(binding)) => found.push((ns, binding)),
                    Step::Found(None) => {}
                    Step::Undetermined => return Step::Undetermined,
                    Step::Failed => return Step::Failed,
                }
            }
        }
        if found.is_empty() {
            let message = format!(
                "unresolved import: no `{target}` in `{}`",
                display_prefix(path)
            );
            self.error(import.file, *target_range, Code::Unresolved, message);
            return Step::Failed;
        }
        let accessible: Vec<_> = found
            .iter()
            .copied()
            .filter(|(_, binding)| binding.vis.is_accessible_from(self.hir, import.module))
            .collect();
        if accessible.is_empty() {
            let message = format!("`{target}` is private");
            self.error(import.file, *target_range, Code::Private, message);
            return Step::Failed;
        }
        Step::Found(accessible)
    }

    /// Looks the last segment of an import up in its container. `Found(None)`
    /// means the container has no such name in this namespace.
    fn lookup_in_container(
        &mut self,
        import: &Import,
        container: DefId,
        name: &Name,
        range: TextRange,
        ns: Namespace,
        force: bool,
    ) -> Step<Option<Binding>> {
        match container {
            DefId::Module(module) => match self.env().lookup_in_module(module, name, ns, force) {
                Lookup::Found(binding) => Step::Found(Some(binding)),
                Lookup::NotFound => Step::Found(None),
                Lookup::Undetermined => Step::Undetermined,
                Lookup::Ambiguous => self.ambiguous(import.file, range, name),
            },
            DefId::Enum(e) => Step::Found(self.env().variant_binding(e, name, ns)),
            other => self.not_a_container(import, other, range),
        }
    }

    /// Resolves the start of an import's path and its first `upto` segments
    /// as a chain of modules (and, last, possibly an enum).
    fn walk(&mut self, import: &Import, upto: usize, force: bool) -> Step<DefId> {
        let path = &import.path;
        let module = import.module;
        let (mut current, first) = match path.kind {
            PathKind::Crate => (DefId::Module(self.hir.crate_root(module)), 0),
            PathKind::SelfModule => (DefId::Module(module), 0),
            PathKind::Super(n) => {
                match (0..n).try_fold(module, |m, _| self.hir.modules[m].parent) {
                    Some(ancestor) => (DefId::Module(ancestor), 0),
                    None => {
                        let message = "there are too many leading `super` keywords";
                        self.error(import.file, path.range, Code::Unresolved, message);
                        return Step::Failed;
                    }
                }
            }
            PathKind::Global => {
                let (name, range) = &path.segments[0];
                match self.env().extern_crate(module, name) {
                    Some(root) => (DefId::Module(root), 1),
                    None => {
                        let message = format!("there is no crate named `{name}`");
                        self.error(import.file, *range, Code::Unresolved, message);
                        return Step::Failed;
                    }
                }
            }
            PathKind::Plain if upto == 0 => (DefId::Module(module), 0),
            PathKind::Plain => {
                let (name, range) = &path.segments[0];
                match self
                    .env()
                    .lookup_lexical(module, name, Namespace::Types, force, true)
                {
                    Lookup::Found(Res::Def(def)) => (def, 1),
                    Lookup::Found(_) => {
                        let message = format!("`{name}` is a primitive type, not a module");
                        self.error(import.file, *range, Code::Unresolved, message);
                        return Step::Failed;
                    }
                    Lookup::NotFound => {
                        let message = format!("unresolved import: cannot find `{name}`");
                        self.error(import.file, *range, Code::Unresolved, message);
                        return Step::Failed;
                    }
                    Lookup::Ambiguous => return self.ambiguous(import.file, *range, name),
                    Lookup::Undetermined => return Step::Undetermined,
                }
            }
        };
        for (name, range) in path.segments.iter().take(upto).skip(first) {
            let binding = match current {
                DefId::Module(m) => {
                    match self
                        .env()
                        .lookup_in_module(m, name, Namespace::Types, force)
                    {
                        Lookup::Found(binding) => binding,
                        Lookup::NotFound => {
                            let message = format!("unresolved import: no `{name}` in this module");
                            self.error(import.file, *range, Code::Unresolved, message);
                            return Step::Failed;
                        }
                        Lookup::Ambiguous => return self.ambiguous(import.file, *range, name),
                        Lookup::Undetermined => return Step::Undetermined,
                    }
                }
                DefId::Enum(e) => match self.env().variant_binding(e, name, Namespace::Types) {
                    Some(binding) => binding,
                    None => {
                        let message = format!("no variant `{name}` in this enum");
                        self.error(import.file, *range, Code::Unresolved, message);
                        return Step::Failed;
                    }
                },
                other => return self.not_a_container(import, other, *range),
            };
            if !binding.vis.is_accessible_from(self.hir, module) {
                let message = format!("`{name}` is private");
                self.error(import.file, *range, Code::Private, message);
                return Step::Failed;
            }
            current = match binding.def {
                ScopeDef::Def(def) => def,
                ScopeDef::PrimTy(_) => {
                    let message = format!("`{name}` is a primitive type, not a module");
                    self.error(import.file, *range, Code::Unresolved, message);
                    return Step::Failed;
                }
            };
        }
        Step::Found(current)
    }

    fn not_a_container<T>(&mut self, import: &Import, def: DefId, range: TextRange) -> Step<T> {
        let (code, message) = match def {
            DefId::Trait(_) => (
                Code::Unsupported,
                "importing items of a trait with `use` is not supported",
            ),
            _ => (Code::Unresolved, "this is not a module or an enum"),
        };
        self.error(import.file, range, code, message);
        Step::Failed
    }

    fn ambiguous<T>(
        &mut self,
        file: bluing_base::FileId,
        range: TextRange,
        name: &Name,
    ) -> Step<T> {
        let message =
            format!("`{name}` is ambiguous: it is provided by more than one import or scope");
        self.error(file, range, Code::Ambiguous, message);
        Step::Failed
    }

    /// A binding for a definition found through the lexical scope or as a
    /// module itself, with the definition's own visibility.
    fn item_binding(&self, def: DefId) -> Binding {
        let vis = match def {
            // Crate roots are public to the crates that can name them.
            DefId::Module(m) if self.hir.modules[m].parent.is_none() => Visibility::Public,
            _ => self.out.def_visibilities[&def],
        };
        Binding {
            def: ScopeDef::Def(def),
            vis,
            source: BindingSource::Item,
        }
    }

    fn bind_import(&mut self, id: ImportId, name: Option<&Name>, found: Vec<(Namespace, Binding)>) {
        let import = &self.hir.imports[id];
        let vis = self.import_vis[id];
        let mut too_wide = false;
        let mut duplicate = false;
        for (ns, target) in found {
            too_wide |= !target.vis.is_at_least(vis, self.hir);
            let binding = Binding {
                def: target.def,
                vis,
                source: BindingSource::Import(id),
            };
            match name {
                None => {
                    if matches!(target.def, ScopeDef::Def(DefId::Trait(_))) {
                        self.out.scopes[import.module].add_unnamed_trait(self.hir, binding);
                    }
                }
                Some(name) => {
                    duplicate |= self.out.scopes[import.module]
                        .define(ns, name.clone(), binding)
                        .is_err();
                }
            }
        }
        // Each problem is reported once, even if the import binds the name
        // in both namespaces.
        if too_wide {
            let shown = name.map_or("this item".to_owned(), |n| format!("`{n}`"));
            let message =
                format!("{shown} is less visible than this import and cannot be re-exported");
            self.error(import.file, import.range, Code::Visibility, message);
        }
        if duplicate && let Some(name) = name {
            let message = format!("the name `{name}` is defined multiple times");
            self.error(import.file, import.range, Code::Duplicate, message);
        }
    }

    /// Copies names along glob links until nothing changes.
    pub(super) fn propagate_globs(&mut self) {
        loop {
            let mut changed = false;
            for link in self.glob_links.clone() {
                for (ns, name, binding) in self.glob_entries(link) {
                    let binding = Binding {
                        def: binding.def,
                        vis: binding.vis.min(link.vis, self.hir),
                        source: BindingSource::Glob(link.import),
                    };
                    changed |=
                        self.out.scopes[link.importing].add_glob(self.hir, ns, name, binding);
                }
                if let GlobSource::Module(source) = link.source {
                    for binding in self.out.scopes[source].unnamed_traits.clone() {
                        if binding.vis.is_accessible_from(self.hir, link.importing) {
                            let binding = Binding {
                                vis: binding.vis.min(link.vis, self.hir),
                                source: BindingSource::Glob(link.import),
                                ..binding
                            };
                            changed |= self.out.scopes[link.importing]
                                .add_unnamed_trait(self.hir, binding);
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
    }

    /// Everything a glob link imports: the names in the source visible from
    /// the importing module.
    fn glob_entries(&self, link: GlobLink) -> Vec<(Namespace, Name, Binding)> {
        let mut entries = Vec::new();
        match link.source {
            GlobSource::Module(source) => {
                let scope = &self.out.scopes[source];
                for ns in Namespace::ALL {
                    for (name, bindings) in scope.iter(ns) {
                        for binding in bindings.explicit.iter().chain(&bindings.globs) {
                            if binding.vis.is_accessible_from(self.hir, link.importing) {
                                entries.push((ns, name.clone(), *binding));
                            }
                        }
                    }
                }
            }
            GlobSource::Enum(e) => {
                for &variant in &self.hir.enums[e].variants {
                    let name = &self.hir.variants[variant].name;
                    for ns in Namespace::ALL {
                        if let Some(binding) = self.env().variant_binding(e, name, ns) {
                            entries.push((ns, name.clone(), binding));
                        }
                    }
                }
            }
        }
        entries
    }
}

/// The path of an import without its last segment, for messages.
fn display_prefix(path: &ModPath) -> String {
    let mut parts: Vec<String> = match path.kind {
        PathKind::Plain | PathKind::Global => Vec::new(),
        PathKind::Crate => vec!["crate".to_owned()],
        PathKind::SelfModule => vec!["self".to_owned()],
        PathKind::Super(n) => vec!["super".to_owned(); n as usize],
    };
    let prefix_len = path.segments.len().saturating_sub(1);
    parts.extend(
        path.segments[..prefix_len]
            .iter()
            .map(|(name, _)| name.to_string()),
    );
    if parts.is_empty() {
        "this module".to_owned()
    } else {
        parts.join("::")
    }
}
