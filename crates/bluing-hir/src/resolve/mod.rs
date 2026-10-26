//! Name resolution: module scopes, imports, visibilities, and every path and
//! lifetime in item signatures. The rules are described in DESIGN.md under
//! "Name resolution".

mod duplicates;
mod env;
mod imports;
mod paths;
mod res;
mod scope;
mod signatures;
mod types;
mod visibility;

pub use res::{LifetimeRes, PathRes, PrimTy, Res, SelfTyOf};
pub use scope::{Binding, BindingSource, ModuleScope, NameBindings, Namespace, ScopeDef};
pub use visibility::Visibility;

pub(crate) use env::{Lookup, NameEnv};
pub(crate) use signatures::{container_scope, scope};
pub(crate) use types::{LexScope, TypeResolver};

use std::collections::BTreeMap;

use bluing_base::{Code, Diagnostic, FileId, TextRange};
use la_arena::ArenaMap;

use crate::hir::{AssocItem, Fields, Hir, ModuleItem, RawVisibility};
use crate::{
    CrateGraph, CrateId, DefId, ImportId, LifetimeId, ModuleId, Name, PathId, StructId, VariantId,
};

/// The owner of a list of fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FieldOwner {
    Struct(StructId),
    Variant(VariantId),
}

/// Everything name resolution produces.
#[derive(Debug, Default)]
pub struct Resolutions {
    pub crate_roots: ArenaMap<CrateId, ModuleId>,
    pub scopes: ArenaMap<ModuleId, ModuleScope>,
    /// The visibility of every nameable definition and associated item.
    pub def_visibilities: BTreeMap<DefId, Visibility>,
    pub field_visibilities: BTreeMap<FieldOwner, Vec<Visibility>>,
    /// The standard prelude module of each script crate.
    pub preludes: ArenaMap<CrateId, ModuleId>,
    paths: Vec<Option<PathRes>>,
    lifetimes: Vec<Option<LifetimeRes>>,
}

impl Resolutions {
    /// The resolution of a path written in a signature.
    pub fn path(&self, id: PathId) -> Option<PathRes> {
        self.paths.get(id.0 as usize).copied().flatten()
    }

    /// The resolution of a lifetime written in a signature.
    pub fn lifetime(&self, id: LifetimeId) -> Option<LifetimeRes> {
        self.lifetimes.get(id.0 as usize).copied().flatten()
    }
}

pub(crate) fn resolve(
    hir: &Hir,
    graph: &CrateGraph,
    crate_roots: ArenaMap<CrateId, ModuleId>,
) -> (Resolutions, Vec<Diagnostic>) {
    let mut resolver = Resolver {
        hir,
        graph,
        import_vis: ArenaMap::default(),
        pending: hir.imports.iter().map(|(id, _)| id).collect(),
        resolving: None,
        glob_links: Vec::new(),
        diagnostics: Vec::new(),
        out: Resolutions {
            crate_roots,
            paths: vec![None; hir.path_count as usize],
            lifetimes: vec![None; hir.lifetime_count as usize],
            ..Resolutions::default()
        },
    };
    resolver.find_preludes();
    resolver.define_items();
    resolver.check_duplicates();
    resolver.resolve_imports();
    resolver.resolve_signatures();
    (resolver.out, resolver.diagnostics)
}

struct Resolver<'a> {
    hir: &'a Hir,
    graph: &'a CrateGraph,
    /// The resolved visibility of each import.
    import_vis: ArenaMap<ImportId, Visibility>,
    /// Imports not resolved yet, in source order.
    pending: Vec<ImportId>,
    /// The import currently being resolved.
    resolving: Option<ImportId>,
    glob_links: Vec<imports::GlobLink>,
    diagnostics: Vec<Diagnostic>,
    out: Resolutions,
}

impl Resolver<'_> {
    fn error(&mut self, file: FileId, range: TextRange, code: Code, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::new(file, range, code, message));
    }

    /// The module system as it stands, for looking names up.
    fn env(&self) -> NameEnv<'_> {
        NameEnv {
            hir: self.hir,
            graph: self.graph,
            crate_roots: &self.out.crate_roots,
            scopes: &self.out.scopes,
            preludes: &self.out.preludes,
            def_visibilities: &self.out.def_visibilities,
            progress: Some(env::ImportProgress {
                pending: &self.pending,
                resolving: self.resolving,
                glob_links: &self.glob_links,
            }),
        }
    }

    /// Script crates get `std::prelude::rust_2024` as their prelude.
    fn find_preludes(&mut self) {
        for (krate, data) in self.graph.crates() {
            if data.kind == crate::CrateKind::Library {
                continue;
            }
            let std = data
                .deps
                .iter()
                .find(|&&dep| self.graph[dep].name.as_str() == "std")
                .and_then(|&dep| self.out.crate_roots.get(dep).copied());
            let prelude = std
                .and_then(|root| self.child_module(root, "prelude"))
                .and_then(|prelude| self.child_module(prelude, "rust_2024"));
            if let Some(prelude) = prelude {
                self.out.preludes.insert(krate, prelude);
            }
        }
    }

    fn child_module(&self, module: ModuleId, name: &str) -> Option<ModuleId> {
        self.hir.modules[module]
            .children
            .iter()
            .copied()
            .find(|&child| self.hir.modules[child].name.as_ref().map(Name::as_str) == Some(name))
    }

    fn resolve_vis(&mut self, module: ModuleId, raw: &RawVisibility, file: FileId) -> Visibility {
        visibility::resolve(self.hir, module, raw, file, &mut self.diagnostics)
    }

    /// Binds every item in its module's scope and records its visibility.
    fn define_items(&mut self) {
        let hir = self.hir;
        for (module, data) in hir.modules.iter() {
            self.out.scopes.insert(module, ModuleScope::default());
            for &item in &data.items {
                self.define_item(module, item);
            }
        }
    }

    fn define_item(&mut self, module: ModuleId, item: ModuleItem) {
        let hir = self.hir;
        let (name, def, raw_vis, loc, namespaces): (_, _, _, _, &[Namespace]) = match item {
            ModuleItem::Module(id) => {
                let m = &hir.modules[id];
                let decl = m.decl.expect("child modules have a declaration");
                (
                    m.name.clone(),
                    DefId::Module(id),
                    &m.vis,
                    decl,
                    &[Namespace::Types],
                )
            }
            ModuleItem::Struct(id) => {
                let s = &hir.structs[id];
                let vis = self.resolve_vis(module, &s.vis, s.loc.file);
                self.record_fields(module, FieldOwner::Struct(id), &s.fields, s.loc.file);
                self.out.def_visibilities.insert(DefId::Struct(id), vis);
                let mut bound =
                    self.bind(module, &s.name, DefId::Struct(id), vis, Namespace::Types);
                if !matches!(s.fields, Fields::Record(_)) {
                    // The constructor is only as visible as the least visible field.
                    let ctor_vis = self.out.field_visibilities[&FieldOwner::Struct(id)]
                        .iter()
                        .fold(vis, |acc, &field| acc.min(field, hir));
                    bound &= self.bind(
                        module,
                        &s.name,
                        DefId::Struct(id),
                        ctor_vis,
                        Namespace::Values,
                    );
                }
                if !bound {
                    self.duplicate(s.loc, &s.name);
                }
                return;
            }
            ModuleItem::Enum(id) => {
                let e = &hir.enums[id];
                let vis = self.resolve_vis(module, &e.vis, e.loc.file);
                for &variant in &e.variants {
                    self.out
                        .def_visibilities
                        .insert(DefId::Variant(variant), vis);
                    let v = &hir.variants[variant];
                    self.record_fields(module, FieldOwner::Variant(variant), &v.fields, v.loc.file);
                }
                self.out.def_visibilities.insert(DefId::Enum(id), vis);
                if !self.bind(module, &e.name, DefId::Enum(id), vis, Namespace::Types) {
                    self.duplicate(e.loc, &e.name);
                }
                return;
            }
            ModuleItem::Fn(id) => {
                let f = &hir.functions[id];
                (
                    Some(f.name.clone()),
                    DefId::Fn(id),
                    &f.vis,
                    f.loc,
                    &[Namespace::Values],
                )
            }
            ModuleItem::Trait(id) => {
                let t = &hir.traits[id];
                let vis = self.resolve_vis(module, &t.vis, t.loc.file);
                for &assoc in &t.items {
                    // Trait items share the trait's visibility.
                    self.out.def_visibilities.insert(assoc_def(assoc), vis);
                }
                self.out.def_visibilities.insert(DefId::Trait(id), vis);
                if !self.bind(module, &t.name, DefId::Trait(id), vis, Namespace::Types) {
                    self.duplicate(t.loc, &t.name);
                }
                return;
            }
            ModuleItem::Impl(id) => {
                let imp = &hir.impls[id];
                for &assoc in &imp.items {
                    let (raw, file) = assoc_vis(hir, assoc);
                    let vis = match imp.trait_ref {
                        // Trait impl items are as visible as the trait;
                        // qualifiers on them were already rejected.
                        Some(_) => Visibility::Public,
                        None => self.resolve_vis(module, raw, file),
                    };
                    self.out.def_visibilities.insert(assoc_def(assoc), vis);
                }
                return;
            }
            ModuleItem::TypeAlias(id) => {
                let t = &hir.type_aliases[id];
                (
                    Some(t.name.clone()),
                    DefId::TypeAlias(id),
                    &t.vis,
                    t.loc,
                    &[Namespace::Types],
                )
            }
            ModuleItem::Const(id) => {
                let c = &hir.consts[id];
                (
                    c.name.clone(),
                    DefId::Const(id),
                    &c.vis,
                    c.loc,
                    &[Namespace::Values],
                )
            }
            ModuleItem::Static(id) => {
                let s = &hir.statics[id];
                (
                    Some(s.name.clone()),
                    DefId::Static(id),
                    &s.vis,
                    s.loc,
                    &[Namespace::Values],
                )
            }
            ModuleItem::Import(id) => {
                let import = &hir.imports[id];
                let vis = self.resolve_vis(module, &import.vis, import.file);
                self.import_vis.insert(id, vis);
                return;
            }
        };
        // A module's visibility is resolved in its parent, where it is written.
        let vis = self.resolve_vis(module, raw_vis, loc.file);
        self.out.def_visibilities.insert(def, vis);
        if let Some(name) = name {
            let mut bound = true;
            for &ns in namespaces {
                bound &= self.bind(module, &name, def, vis, ns);
            }
            if !bound {
                self.duplicate(loc, &name);
            }
        }
    }

    fn record_fields(
        &mut self,
        module: ModuleId,
        owner: FieldOwner,
        fields: &Fields,
        file: FileId,
    ) {
        let fields = match fields {
            Fields::Record(fields) | Fields::Tuple(fields) => fields.as_slice(),
            Fields::Unit => &[],
        };
        let visibilities = match owner {
            // Variant fields are always as visible as the enum.
            FieldOwner::Variant(variant) => {
                let vis = self.out.def_visibilities[&DefId::Variant(variant)];
                vec![vis; fields.len()]
            }
            FieldOwner::Struct(_) => fields
                .iter()
                .map(|field| self.resolve_vis(module, &field.vis, file))
                .collect(),
        };
        self.out.field_visibilities.insert(owner, visibilities);
    }

    /// Binds an item in one namespace. Returns `false` if the name is already
    /// taken there; the caller reports that once per item.
    fn bind(
        &mut self,
        module: ModuleId,
        name: &Name,
        def: DefId,
        vis: Visibility,
        ns: Namespace,
    ) -> bool {
        let binding = Binding {
            def: ScopeDef::Def(def),
            vis,
            source: BindingSource::Item,
        };
        self.out.scopes[module]
            .define(ns, name.clone(), binding)
            .is_ok()
    }

    fn duplicate(&mut self, loc: crate::hir::Loc, name: &Name) {
        let message = format!("the name `{name}` is defined multiple times");
        self.error(loc.file, loc.range, Code::Duplicate, message);
    }
}

fn assoc_def(item: AssocItem) -> DefId {
    match item {
        AssocItem::Fn(id) => DefId::Fn(id),
        AssocItem::Const(id) => DefId::Const(id),
        AssocItem::TypeAlias(id) => DefId::TypeAlias(id),
    }
}

fn assoc_vis(hir: &Hir, item: AssocItem) -> (&RawVisibility, FileId) {
    match item {
        AssocItem::Fn(id) => (&hir.functions[id].vis, hir.functions[id].loc.file),
        AssocItem::Const(id) => (&hir.consts[id].vis, hir.consts[id].loc.file),
        AssocItem::TypeAlias(id) => (&hir.type_aliases[id].vis, hir.type_aliases[id].loc.file),
    }
}
