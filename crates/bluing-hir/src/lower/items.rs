//! Lowering of structs, enums, functions and visibilities.

use bluing_base::{Code, FileId};
use bluing_syntax::ra_ap_syntax::{
    AstNode, AstPtr, SyntaxNode, WalkEvent,
    ast::{self, HasName, HasVisibility},
};

use super::{Collector, ModCtx, VisRule, attrs, range_of};
use crate::hir::{
    Container, Enum, Field, Fields, Function, InFile, Loc, Param, RawVisibility, SelfParam,
    SelfParamKind, Struct, Variant,
};
use crate::{DefId, EnumId, FnId, Name, StructId};

impl Collector<'_> {
    /// Lowers a visibility according to `rule`.
    pub(super) fn visibility(
        &mut self,
        file: FileId,
        vis: Option<ast::Visibility>,
        rule: VisRule,
    ) -> RawVisibility {
        let Some(vis) = vis else {
            return RawVisibility::Private;
        };
        match rule {
            VisRule::Allowed => {}
            VisRule::Forbidden => {
                let message = "visibility qualifiers are not permitted here";
                self.error(file, range_of(&vis), Code::Visibility, message);
                return RawVisibility::Private;
            }
            VisRule::ForbiddenReportedByParser => return RawVisibility::Private,
        }
        match vis.kind() {
            ast::VisibilityKind::Pub => RawVisibility::Public,
            ast::VisibilityKind::PubCrate => RawVisibility::Crate,
            ast::VisibilityKind::PubSuper => RawVisibility::Super,
            ast::VisibilityKind::PubSelf => RawVisibility::SelfModule,
            ast::VisibilityKind::In(path) => {
                let has_in = vis
                    .visibility_inner()
                    .is_some_and(|inner| inner.in_token().is_some());
                if !has_in {
                    // `pub(a)`, which the parser already reports. Treat the
                    // item as public so its uses don't report privacy errors.
                    return RawVisibility::Public;
                }
                match self.mod_path(file, &path) {
                    Some(path) if path.kind != crate::hir::PathKind::Plain => {
                        RawVisibility::In(path)
                    }
                    Some(path) => {
                        let message = "paths in `pub(in ...)` must start with `crate`, `self` \
                                       or `super`";
                        self.error(file, path.range, Code::Visibility, message);
                        RawVisibility::Public
                    }
                    None => RawVisibility::Public,
                }
            }
        }
    }

    pub(super) fn struct_(&mut self, ctx: &ModCtx<'_>, it: &ast::Struct) -> Option<StructId> {
        let name = it.name()?;
        let vis = self.visibility(ctx.file, it.visibility(), VisRule::Allowed);
        let generics = self.types(ctx.file).generics(it);
        let fields = self.fields(ctx.file, it.field_list());
        let id = self.hir.structs.alloc(Struct {
            name: Name::new(name.text()),
            loc: Loc {
                file: ctx.file,
                range: range_of(&name),
            },
            module: ctx.module,
            vis,
            generics,
            fields,
            derives: attrs::derives(it),
        });
        self.lang(ctx, it, Some(DefId::Struct(id)));
        Some(id)
    }

    fn fields(&mut self, file: FileId, list: Option<ast::FieldList>) -> Fields {
        match list {
            None => Fields::Unit,
            Some(ast::FieldList::RecordFieldList(list)) => {
                let mut fields = Vec::new();
                for field in list.fields() {
                    if field.default_val().is_some() || field.mut_restriction().is_some() {
                        let message = "field defaults and mutability restrictions are not \
                                       supported in Bluing";
                        self.error(file, range_of(&field), Code::Unsupported, message);
                    }
                    let Some(name) = field.name() else { continue };
                    let vis = self.visibility(file, field.visibility(), VisRule::Allowed);
                    let ty = self.types(file).ty_opt(field.ty());
                    fields.push(Field {
                        name: Name::new(name.text()),
                        range: range_of(&name),
                        vis,
                        ty,
                    });
                }
                Fields::Record(fields)
            }
            Some(ast::FieldList::TupleFieldList(list)) => {
                let mut fields = Vec::new();
                for (index, field) in list.fields().enumerate() {
                    if field.mut_restriction().is_some() {
                        let message = "mutability restrictions are not supported in Bluing";
                        self.error(file, range_of(&field), Code::Unsupported, message);
                    }
                    let vis = self.visibility(file, field.visibility(), VisRule::Allowed);
                    let ty = self.types(file).ty_opt(field.ty());
                    fields.push(Field {
                        name: Name::new(&index.to_string()),
                        range: range_of(&field),
                        vis,
                        ty,
                    });
                }
                Fields::Tuple(fields)
            }
        }
    }

    pub(super) fn enum_(&mut self, ctx: &ModCtx<'_>, it: &ast::Enum) -> Option<EnumId> {
        let name = it.name()?;
        let vis = self.visibility(ctx.file, it.visibility(), VisRule::Allowed);
        let generics = self.types(ctx.file).generics(it);
        let id = self.hir.enums.alloc(Enum {
            name: Name::new(name.text()),
            loc: Loc {
                file: ctx.file,
                range: range_of(&name),
            },
            module: ctx.module,
            vis,
            generics,
            variants: Vec::new(),
            derives: attrs::derives(it),
        });
        self.lang(ctx, it, Some(DefId::Enum(id)));
        for variant in it
            .variant_list()
            .into_iter()
            .flat_map(|list| list.variants())
        {
            let Some(variant_name) = variant.name() else {
                continue;
            };
            // Variants share the enum's visibility and cannot have their own.
            self.visibility(ctx.file, variant.visibility(), VisRule::Forbidden);
            let fields = self.fields(ctx.file, variant.field_list());
            let discriminant = variant.const_arg().and_then(|arg| arg.expr());
            if let Some(expr) = &discriminant {
                self.report_nested_items(ctx.file, expr.syntax());
            }
            let discriminant = self.types(ctx.file).expr(discriminant);
            let variant_id = self.hir.variants.alloc(Variant {
                name: Name::new(variant_name.text()),
                loc: Loc {
                    file: ctx.file,
                    range: range_of(&variant_name),
                },
                parent: id,
                fields,
                discriminant,
            });
            self.hir.enums[id].variants.push(variant_id);
            self.lang(ctx, &variant, Some(DefId::Variant(variant_id)));
        }
        Some(id)
    }

    pub(super) fn module_fn(&mut self, ctx: &ModCtx<'_>, it: &ast::Fn) -> Option<FnId> {
        self.function(ctx, it, Container::Module(ctx.module), VisRule::Allowed)
    }

    /// Lowers a function in any container.
    pub(super) fn function(
        &mut self,
        ctx: &ModCtx<'_>,
        it: &ast::Fn,
        container: Container,
        vis_rule: VisRule,
    ) -> Option<FnId> {
        let name = it.name()?;
        let file = ctx.file;
        if it.default_token().is_some() || it.gen_token().is_some() {
            let message = "`default fn` and `gen fn` are not supported in Bluing";
            self.error(file, range_of(it), Code::Unsupported, message);
        }
        let vis = self.visibility(file, it.visibility(), vis_rule);
        let generics = self.types(file).generics(it);
        let param_list = it.param_list();
        let self_param = param_list
            .as_ref()
            .and_then(|list| list.self_param())
            .map(|param| self.self_param(file, &param));
        if self_param.is_some() && matches!(container, Container::Module(_)) {
            let message = "`self` parameters are only allowed in associated functions";
            self.error(file, range_of(&name), Code::Syntax, message);
        }
        let params = param_list
            .iter()
            .flat_map(|list| list.params())
            .map(|param| Param {
                ty: self.types(file).ty_opt(param.ty()),
                pat: param.pat().map(|pat| InFile {
                    file,
                    value: AstPtr::new(&pat),
                }),
            })
            .collect();
        let ret = it.ret_type().map(|ret| self.types(file).ty_opt(ret.ty()));
        let body = it.body();
        match (&body, container) {
            (None, Container::Module(_)) => {
                let message = "free function without a body";
                self.error(file, range_of(&name), Code::Syntax, message);
            }
            (None, Container::Impl(_)) => {
                let message = "associated function in `impl` without a body";
                self.error(file, range_of(&name), Code::Syntax, message);
            }
            _ => {}
        }
        if let Some(body) = &body {
            self.report_nested_items(file, body.syntax());
        }
        let id = self.hir.functions.alloc(Function {
            name: Name::new(name.text()),
            loc: Loc {
                file,
                range: range_of(&name),
            },
            container,
            vis,
            generics,
            self_param,
            params,
            ret,
            is_const: it.const_token().is_some(),
            is_async: it.async_token().is_some(),
            is_unsafe: it.unsafe_token().is_some(),
            body: body.map(|body| InFile {
                file,
                value: AstPtr::new(&body),
            }),
        });
        self.lang(ctx, it, Some(DefId::Fn(id)));
        Some(id)
    }

    fn self_param(&mut self, file: FileId, param: &ast::SelfParam) -> SelfParam {
        let mutable = param.mut_token().is_some();
        let kind = if let Some(ty) = param.ty() {
            SelfParamKind::Typed {
                ty: self.types(file).ty(&ty),
                mutable,
            }
        } else if param.amp_token().is_some() {
            SelfParamKind::Ref {
                lifetime: param.lifetime().map(|lt| self.types(file).lifetime(&lt)),
                mutable,
            }
        } else {
            SelfParamKind::Value { mutable }
        };
        SelfParam {
            range: range_of(param),
            kind,
        }
    }

    /// Items inside function bodies and other expressions are not supported
    /// yet. Reports each outermost one under `node`.
    pub(super) fn report_nested_items(&mut self, file: FileId, node: &SyntaxNode) {
        let mut preorder = node.preorder();
        while let Some(event) = preorder.next() {
            let WalkEvent::Enter(child) = event else {
                continue;
            };
            let Some(item) = ast::Item::cast(child) else {
                continue;
            };
            match item {
                // Expression macro calls share the item macro call node and are
                // handled with bodies; inline assembly, `macro_rules!` and
                // `extern` blocks were already rejected by the subset check.
                ast::Item::MacroCall(_)
                | ast::Item::AsmExpr(_)
                | ast::Item::MacroRules(_)
                | ast::Item::ExternBlock(_) => {}
                item => {
                    let message = "items inside function bodies and expressions are not \
                                   supported yet";
                    self.error(file, range_of(&item), Code::NotYetSupported, message);
                    preorder.skip_subtree();
                }
            }
        }
    }
}
