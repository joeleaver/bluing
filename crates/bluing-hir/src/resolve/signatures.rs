//! Resolves every signature: walks each item with the lexical scope (generic
//! parameters and `Self`) its paths are written in.

use super::types::LexScope;
use super::{Resolver, SelfTyOf};
use crate::hir::{Container, Fields, GenericParams, SelfParamKind};
use crate::{FileId, GenericOwner, ModuleId};

impl<'a> Resolver<'a> {
    pub(super) fn resolve_signatures(&mut self) {
        let hir = self.hir;
        for (id, s) in hir.structs.iter() {
            let scope = scope(
                s.module,
                s.loc.file,
                vec![(GenericOwner::Struct(id), &s.generics)],
                Some(SelfTyOf::Struct(id)),
            );
            self.resolve_generics(&scope, &s.generics);
            self.resolve_fields(&scope, &s.fields);
        }
        for (id, e) in hir.enums.iter() {
            let scope = scope(
                e.module,
                e.loc.file,
                vec![(GenericOwner::Enum(id), &e.generics)],
                Some(SelfTyOf::Enum(id)),
            );
            self.resolve_generics(&scope, &e.generics);
            for &variant in &e.variants {
                self.resolve_fields(&scope, &hir.variants[variant].fields);
            }
        }
        for (id, t) in hir.traits.iter() {
            let scope = scope(
                t.module,
                t.loc.file,
                vec![(GenericOwner::Trait(id), &t.generics)],
                Some(SelfTyOf::Trait(id)),
            );
            self.resolve_generics(&scope, &t.generics);
            for bound in &t.supertraits {
                self.resolve_bound(&scope, bound);
            }
        }
        for (id, imp) in hir.impls.iter() {
            let mut scope = scope(
                imp.module,
                imp.loc.file,
                vec![(GenericOwner::Impl(id), &imp.generics)],
                Some(SelfTyOf::Impl(id)),
            );
            self.resolve_generics(&scope, &imp.generics);
            if let Some(trait_ref) = &imp.trait_ref {
                self.resolve_trait_ref(&scope, trait_ref);
            }
            scope.self_ty = None;
            scope.in_impl_self_type = true;
            self.resolve_type(&scope, &imp.self_ty);
        }
        for (id, f) in hir.functions.iter() {
            let mut scope = self.container_scope(f.container, f.loc.file);
            scope.generics.push((GenericOwner::Fn(id), &f.generics));
            self.resolve_generics(&scope, &f.generics);
            if let Some(self_param) = &f.self_param {
                match &self_param.kind {
                    SelfParamKind::Value { .. } => {}
                    SelfParamKind::Ref { lifetime, .. } => {
                        if let Some(lifetime) = lifetime {
                            self.resolve_lifetime(&scope, lifetime);
                        }
                    }
                    SelfParamKind::Typed { ty, .. } => self.resolve_type(&scope, ty),
                }
            }
            for param in &f.params {
                self.resolve_type(&scope, &param.ty);
            }
            if let Some(ret) = &f.ret {
                self.resolve_type(&scope, ret);
            }
        }
        for (id, alias) in hir.type_aliases.iter() {
            let mut scope = self.container_scope(alias.container, alias.loc.file);
            scope
                .generics
                .push((GenericOwner::TypeAlias(id), &alias.generics));
            self.resolve_generics(&scope, &alias.generics);
            for bound in &alias.bounds {
                self.resolve_bound(&scope, bound);
            }
            if let Some(ty) = &alias.ty {
                self.resolve_type(&scope, ty);
            }
        }
        for (_, c) in hir.consts.iter() {
            let scope = self.container_scope(c.container, c.loc.file);
            self.resolve_type(&scope, &c.ty);
        }
        for (_, s) in hir.statics.iter() {
            let scope = scope(s.module, s.loc.file, Vec::new(), None);
            self.resolve_type(&scope, &s.ty);
        }
    }

    fn resolve_fields(&mut self, scope: &LexScope<'_>, fields: &Fields) {
        if let Fields::Record(fields) | Fields::Tuple(fields) = fields {
            for field in fields {
                self.resolve_type(scope, &field.ty);
            }
        }
    }

    /// The scope inside a module, trait or impl: an associated item sees its
    /// container's generic parameters and `Self`.
    fn container_scope(&self, container: Container, file: FileId) -> LexScope<'a> {
        let hir = self.hir;
        match container {
            Container::Module(module) => scope(module, file, Vec::new(), None),
            Container::Trait(id) => {
                let t = &hir.traits[id];
                scope(
                    t.module,
                    file,
                    vec![(GenericOwner::Trait(id), &t.generics)],
                    Some(SelfTyOf::Trait(id)),
                )
            }
            Container::Impl(id) => {
                let imp = &hir.impls[id];
                scope(
                    imp.module,
                    file,
                    vec![(GenericOwner::Impl(id), &imp.generics)],
                    Some(SelfTyOf::Impl(id)),
                )
            }
        }
    }
}

fn scope<'h>(
    module: ModuleId,
    file: FileId,
    generics: Vec<(GenericOwner, &'h GenericParams)>,
    self_ty: Option<SelfTyOf>,
) -> LexScope<'h> {
    LexScope {
        module,
        file,
        generics,
        self_ty,
        in_impl_self_type: false,
    }
}
