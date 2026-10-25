//! Lowering of traits, impls, type aliases, constants and statics: the items
//! that can appear inside traits and impls.

use bluing_base::Code;
use bluing_syntax::ra_ap_syntax::{
    AstNode,
    ast::{self, HasAttrs, HasName, HasTypeBounds, HasVisibility},
};

use super::{Collector, ModCtx, VisRule, range_of};
use crate::hir::{
    AssocItem, Const, Container, Impl, Loc, ModuleItem, Static, Trait, TypeAlias, TypeRef,
};
use crate::{ConstId, DefId, ImplId, Name, StaticId, TraitId, TypeAliasId};

impl Collector<'_> {
    pub(super) fn trait_(&mut self, ctx: &ModCtx<'_>, it: &ast::Trait) -> Option<TraitId> {
        let name = it.name()?;
        let file = ctx.file;
        if it.auto_token().is_some() || it.eq_token().is_some() || it.impl_restriction().is_some() {
            let message = "auto traits, trait aliases and impl restrictions are not supported \
                           in Bluing";
            self.error(file, range_of(it), Code::Unsupported, message);
        }
        let vis = self.visibility(file, it.visibility(), VisRule::Allowed);
        let generics = self.types(file).generics(it);
        let supertraits = self.types(file).bounds(it.type_bound_list());
        let id = self.hir.traits.alloc(Trait {
            name: Name::new(name.text()),
            loc: Loc {
                file,
                range: range_of(&name),
            },
            module: ctx.module,
            vis,
            generics,
            supertraits,
            items: Vec::new(),
            is_unsafe: it.unsafe_token().is_some(),
        });
        self.lang(ctx, it, Some(DefId::Trait(id)));
        let items = self.assoc_items(
            ctx,
            it.assoc_item_list(),
            Container::Trait(id),
            VisRule::Forbidden,
        );
        self.hir.traits[id].items = items;
        Some(id)
    }

    pub(super) fn impl_(&mut self, ctx: &ModCtx<'_>, it: &ast::Impl) -> Option<ImplId> {
        let file = ctx.file;
        let header = it
            .impl_token()
            .map_or(range_of(it), |token| token.text_range());
        if it.excl_token().is_some() || it.default_token().is_some() || it.const_token().is_some() {
            let message = "negative, `default` and `const` impls are not supported in Bluing";
            self.error(file, header, Code::Unsupported, message);
        }
        self.visibility(file, it.visibility(), VisRule::Forbidden);
        let generics = self.types(file).generics(it);
        let trait_ref = match it.trait_() {
            Some(ast::Type::PathType(ty)) => {
                ty.path().and_then(|path| self.types(file).path(&path))
            }
            Some(ty) => {
                self.error(file, range_of(&ty), Code::WrongKind, "expected a trait");
                None
            }
            None => None,
        };
        let self_ty = match it.self_ty() {
            Some(ty) => self.types(file).ty(&ty),
            None => TypeRef::Error,
        };
        let is_trait_impl = it.trait_().is_some();
        let id = self.hir.impls.alloc(Impl {
            loc: Loc {
                file,
                range: header,
            },
            module: ctx.module,
            generics,
            trait_ref,
            self_ty,
            items: Vec::new(),
            is_unsafe: it.unsafe_token().is_some(),
        });
        self.lang(ctx, it, None);
        // Trait impl items take the trait's visibility. The parser reports a
        // qualifier on them itself, except in impls with attributes.
        let item_rule = if !is_trait_impl {
            VisRule::Allowed
        } else if it.attrs().next().is_none() {
            VisRule::ForbiddenReportedByParser
        } else {
            VisRule::Forbidden
        };
        let items = self.assoc_items(ctx, it.assoc_item_list(), Container::Impl(id), item_rule);
        self.hir.impls[id].items = items;
        Some(id)
    }

    fn assoc_items(
        &mut self,
        ctx: &ModCtx<'_>,
        list: Option<ast::AssocItemList>,
        container: Container,
        vis_rule: VisRule,
    ) -> Vec<AssocItem> {
        let mut items = Vec::new();
        for item in list.into_iter().flat_map(|list| list.assoc_items()) {
            let lowered = match &item {
                ast::AssocItem::Fn(it) => self
                    .function(ctx, it, container, vis_rule)
                    .map(AssocItem::Fn),
                ast::AssocItem::Const(it) => self
                    .const_(ctx, it, container, vis_rule)
                    .map(AssocItem::Const),
                ast::AssocItem::TypeAlias(it) => self
                    .type_alias(ctx, it, container, vis_rule)
                    .map(AssocItem::TypeAlias),
                ast::AssocItem::MacroCall(call) => {
                    let message = "macro calls in item position are not supported yet";
                    self.error(ctx.file, range_of(call), Code::NotYetSupported, message);
                    None
                }
            };
            items.extend(lowered);
        }
        items
    }

    pub(super) fn module_type_alias(
        &mut self,
        ctx: &ModCtx<'_>,
        it: &ast::TypeAlias,
    ) -> Option<ModuleItem> {
        self.type_alias(ctx, it, Container::Module(ctx.module), VisRule::Allowed)
            .map(ModuleItem::TypeAlias)
    }

    fn type_alias(
        &mut self,
        ctx: &ModCtx<'_>,
        it: &ast::TypeAlias,
        container: Container,
        vis_rule: VisRule,
    ) -> Option<TypeAliasId> {
        let name = it.name()?;
        let file = ctx.file;
        if it.default_token().is_some() {
            let message = "`default type` is not supported in Bluing";
            self.error(file, range_of(it), Code::Unsupported, message);
        }
        let vis = self.visibility(file, it.visibility(), vis_rule);
        let generics = self.types(file).generics(it);
        let bounds = self.types(file).bounds(it.type_bound_list());
        let ty = it.ty().map(|ty| self.types(file).ty(&ty));
        let in_trait = matches!(container, Container::Trait(_));
        if !matches!(container, Container::Module(_)) && !generics.params.is_empty() {
            let message = "generic associated types are not supported yet";
            self.error(file, range_of(&name), Code::NotYetSupported, message);
        }
        if !in_trait && ty.is_none() {
            let message = "type alias without a type";
            self.error(file, range_of(&name), Code::Syntax, message);
        }
        if !in_trait && !bounds.is_empty() {
            let message = "bounds on type aliases are only allowed for associated types in traits";
            self.error(file, range_of(&name), Code::Syntax, message);
        }
        let id = self.hir.type_aliases.alloc(TypeAlias {
            name: Name::new(name.text()),
            loc: Loc {
                file,
                range: range_of(&name),
            },
            container,
            vis,
            generics,
            bounds,
            ty,
        });
        self.lang(ctx, it, Some(DefId::TypeAlias(id)));
        Some(id)
    }

    pub(super) fn module_const(&mut self, ctx: &ModCtx<'_>, it: &ast::Const) -> Option<ModuleItem> {
        self.const_(ctx, it, Container::Module(ctx.module), VisRule::Allowed)
            .map(ModuleItem::Const)
    }

    fn const_(
        &mut self,
        ctx: &ModCtx<'_>,
        it: &ast::Const,
        container: Container,
        vis_rule: VisRule,
    ) -> Option<ConstId> {
        let file = ctx.file;
        let name = it.name();
        if name.is_none() && it.underscore_token().is_none() {
            // A syntax error the parser already reported.
            return None;
        }
        if it.default_token().is_some() {
            let message = "`default const` is not supported in Bluing";
            self.error(file, range_of(it), Code::Unsupported, message);
        }
        let range = name.as_ref().map_or(range_of(it), range_of);
        let vis = self.visibility(file, it.visibility(), vis_rule);
        let ty = self.types(file).ty_opt(it.ty());
        let body = it.body();
        if body.is_none() && !matches!(container, Container::Trait(_)) {
            self.error(file, range, Code::Syntax, "constant without a value");
        }
        if let Some(body) = &body {
            self.report_nested_items(file, body.syntax());
        }
        let body = self.types(file).expr(body);
        let id = self.hir.consts.alloc(Const {
            name: name.map(|name| Name::new(name.text())),
            loc: Loc { file, range },
            container,
            vis,
            ty,
            body,
        });
        self.lang(ctx, it, Some(DefId::Const(id)));
        Some(id)
    }

    pub(super) fn static_(&mut self, ctx: &ModCtx<'_>, it: &ast::Static) -> Option<StaticId> {
        let name = it.name()?;
        let file = ctx.file;
        let vis = self.visibility(file, it.visibility(), VisRule::Allowed);
        let ty = self.types(file).ty_opt(it.ty());
        let body = it.body();
        match &body {
            Some(body) => self.report_nested_items(file, body.syntax()),
            None => self.error(
                file,
                range_of(&name),
                Code::Syntax,
                "static without a value",
            ),
        }
        let body = self.types(file).expr(body);
        let id = self.hir.statics.alloc(Static {
            name: Name::new(name.text()),
            loc: Loc {
                file,
                range: range_of(&name),
            },
            module: ctx.module,
            vis,
            is_mut: it.mut_token().is_some(),
            ty,
            body,
        });
        self.lang(ctx, it, Some(DefId::Static(id)));
        Some(id)
    }
}
