//! Duplicate names inside items: enum variants, fields, associated items,
//! and generic parameters (including ones that shadow the parameters of the
//! enclosing trait or impl, which Rust also forbids).

use std::collections::BTreeSet;

use bluing_base::{Code, FileId, TextRange};

use super::Resolver;
use crate::Name;
use crate::hir::{AssocItem, Container, Fields, GenericParamKind, GenericParams};

impl<'a> Resolver<'a> {
    pub(super) fn check_duplicates(&mut self) {
        let hir = self.hir;
        for (_, s) in hir.structs.iter() {
            self.check_generics(s.loc.file, &s.generics, None);
            self.check_fields(s.loc.file, &s.fields);
        }
        for (_, e) in hir.enums.iter() {
            self.check_generics(e.loc.file, &e.generics, None);
            let variants = e.variants.iter().map(|&v| {
                let variant = &hir.variants[v];
                (&variant.name, variant.loc.range)
            });
            self.check_names(e.loc.file, variants, "variant");
            for &v in &e.variants {
                self.check_fields(e.loc.file, &hir.variants[v].fields);
            }
        }
        for (_, t) in hir.traits.iter() {
            self.check_generics(t.loc.file, &t.generics, None);
            self.check_assoc_items(t.loc.file, &t.items);
        }
        for (_, imp) in hir.impls.iter() {
            self.check_generics(imp.loc.file, &imp.generics, None);
            self.check_assoc_items(imp.loc.file, &imp.items);
        }
        for (_, f) in hir.functions.iter() {
            let outer = self.container_generics(f.container);
            self.check_generics(f.loc.file, &f.generics, outer);
        }
        for (_, alias) in hir.type_aliases.iter() {
            let outer = self.container_generics(alias.container);
            self.check_generics(alias.loc.file, &alias.generics, outer);
        }
    }

    fn container_generics(&self, container: Container) -> Option<&'a GenericParams> {
        let hir = self.hir;
        match container {
            Container::Module(_) => None,
            Container::Trait(id) => Some(&hir.traits[id].generics),
            Container::Impl(id) => Some(&hir.impls[id].generics),
        }
    }

    /// Reports every name after the first occurrence.
    fn check_names<'n>(
        &mut self,
        file: FileId,
        names: impl Iterator<Item = (&'n Name, TextRange)>,
        what: &str,
    ) {
        let mut seen = BTreeSet::new();
        for (name, range) in names {
            if !seen.insert(name) {
                let message = format!("{what} `{name}` is defined more than once");
                self.error(file, range, Code::Duplicate, message);
            }
        }
    }

    fn check_fields(&mut self, file: FileId, fields: &Fields) {
        if let Fields::Record(fields) = fields {
            self.check_names(file, fields.iter().map(|f| (&f.name, f.range)), "field");
        }
    }

    /// Associated types share one namespace, functions and constants another.
    fn check_assoc_items(&mut self, file: FileId, items: &[AssocItem]) {
        let hir = self.hir;
        let mut types = Vec::new();
        let mut values = Vec::new();
        for &item in items {
            match item {
                AssocItem::TypeAlias(id) => {
                    let alias = &hir.type_aliases[id];
                    types.push((&alias.name, alias.loc.range));
                }
                AssocItem::Fn(id) => {
                    let f = &hir.functions[id];
                    values.push((&f.name, f.loc.range));
                }
                AssocItem::Const(id) => {
                    let c = &hir.consts[id];
                    if let Some(name) = &c.name {
                        values.push((name, c.loc.range));
                    }
                }
            }
        }
        self.check_names(file, types.into_iter(), "associated item");
        self.check_names(file, values.into_iter(), "associated item");
    }

    /// Generic parameter names must be unique within a list, and may not
    /// reuse a name from the enclosing trait or impl.
    fn check_generics(
        &mut self,
        file: FileId,
        generics: &GenericParams,
        outer: Option<&GenericParams>,
    ) {
        let is_lifetime =
            |kind: &GenericParamKind| matches!(kind, GenericParamKind::Lifetime { .. });
        let mut seen: BTreeSet<(bool, &Name)> = BTreeSet::new();
        for param in &generics.params {
            let key = (is_lifetime(&param.kind), &param.name);
            let shadows = outer.is_some_and(|outer| {
                outer
                    .params
                    .iter()
                    .any(|o| o.name == param.name && is_lifetime(&o.kind) == key.0)
            });
            if !seen.insert(key) {
                let message = format!(
                    "generic parameter `{}` is declared more than once",
                    param.name
                );
                self.error(file, param.range, Code::Duplicate, message);
            } else if shadows {
                let message = format!(
                    "generic parameter `{}` is already declared by the enclosing trait or impl",
                    param.name
                );
                self.error(file, param.range, Code::Duplicate, message);
            }
        }
    }
}
