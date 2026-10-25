//! Lowering of `use` declarations into one [`Import`] per imported name or
//! glob, and of module-only paths (`use` paths and `pub(in ...)`).

use bluing_base::{Code, FileId};
use bluing_syntax::ra_ap_syntax::ast::{self, HasGenericArgs, HasName, HasVisibility};

use super::{Collector, ModCtx, VisRule, range_of};
use crate::Name;
use crate::hir::{Import, ImportKind, ModPath, ModuleItem, PathKind, RawVisibility};

impl Collector<'_> {
    /// Lowers a path that may only go through modules. Returns `None` after
    /// reporting an error if it has generic arguments, `Self`, or a
    /// qualified (`<T as Trait>`) segment.
    pub(super) fn mod_path(&mut self, file: FileId, path: &ast::Path) -> Option<ModPath> {
        let range = range_of(path);
        let mut kind = PathKind::Plain;
        let mut segments = Vec::new();
        for (index, segment) in path.segments().enumerate() {
            if index == 0 && segment.coloncolon_token().is_some() {
                kind = PathKind::Global;
            }
            let segment_range = range_of(&segment);
            if segment.generic_arg_list().is_some() || segment.parenthesized_arg_list().is_some() {
                let message = "generic arguments are not allowed in this path";
                self.error(file, segment_range, Code::Syntax, message);
                return None;
            }
            let leading = segments.is_empty();
            match segment.kind()? {
                ast::PathSegmentKind::Name(name) => {
                    segments.push((Name::new(name.text()), segment_range));
                }
                ast::PathSegmentKind::CrateKw if leading && index == 0 => kind = PathKind::Crate,
                ast::PathSegmentKind::SelfKw if leading && index == 0 => {
                    kind = PathKind::SelfModule;
                }
                ast::PathSegmentKind::SuperKw if leading => {
                    kind = match kind {
                        PathKind::Plain | PathKind::SelfModule => PathKind::Super(1),
                        PathKind::Super(n) => PathKind::Super(n + 1),
                        PathKind::Crate | PathKind::Global => {
                            let message = "`super` can only be used at the start of a path";
                            self.error(file, segment_range, Code::Unresolved, message);
                            return None;
                        }
                    };
                }
                ast::PathSegmentKind::SelfTypeKw => {
                    let message = "`Self` cannot be used in this path";
                    self.error(file, segment_range, Code::Unresolved, message);
                    return None;
                }
                ast::PathSegmentKind::Type { .. } => {
                    let message = "qualified paths are not allowed here";
                    self.error(file, segment_range, Code::Syntax, message);
                    return None;
                }
                ast::PathSegmentKind::CrateKw
                | ast::PathSegmentKind::SelfKw
                | ast::PathSegmentKind::SuperKw => {
                    let message = format!("`{segment}` can only be used at the start of a path");
                    self.error(file, segment_range, Code::Unresolved, message);
                    return None;
                }
            }
        }
        Some(ModPath {
            kind,
            segments,
            range,
        })
    }

    pub(super) fn use_item(&mut self, ctx: &ModCtx<'_>, item: &ast::Use) {
        let vis = self.visibility(ctx.file, item.visibility(), VisRule::Allowed);
        self.lang(ctx, item, None);
        if let Some(tree) = item.use_tree() {
            let prefix = ModPath {
                kind: PathKind::Plain,
                segments: Vec::new(),
                range: range_of(&tree),
            };
            self.use_tree(ctx, &prefix, &tree, &vis);
        }
    }

    fn use_tree(
        &mut self,
        ctx: &ModCtx<'_>,
        prefix: &ModPath,
        tree: &ast::UseTree,
        vis: &RawVisibility,
    ) {
        let file = ctx.file;
        let range = range_of(tree);
        let own = match tree.path() {
            Some(path) => {
                // `use a::{self}`: a lone `self` inside a list imports the
                // list's prefix itself.
                if is_lone_self(&path)
                    && tree.use_tree_list().is_none()
                    && tree.star_token().is_none()
                {
                    return self.self_import(ctx, prefix, tree, vis);
                }
                match self.mod_path(file, &path) {
                    Some(path) => path,
                    None => return,
                }
            }
            None => ModPath {
                kind: PathKind::Plain,
                segments: Vec::new(),
                range,
            },
        };
        let Some(path) = self.join_paths(file, prefix, own) else {
            return;
        };
        let empty = path.segments.is_empty() && path.kind == PathKind::Plain;
        if tree.star_token().is_some() {
            if empty {
                let message = "a glob import needs a path: write `use path::*;`";
                return self.error(file, range, Code::Unresolved, message);
            }
            self.push_import(ctx, range, path, ImportKind::Glob, vis);
        } else if let Some(list) = tree.use_tree_list() {
            for subtree in list.use_trees() {
                self.use_tree(ctx, &path, &subtree, vis);
            }
        } else if empty {
            // Nothing to import; the parser already reported the missing path.
        } else {
            let name = match tree.rename() {
                Some(rename) => match rename.name() {
                    Some(name) => Some(Name::new(name.text())),
                    None if rename.underscore_token().is_some() => None,
                    // A syntax error the parser already reported.
                    None => return,
                },
                None => match path.segments.last() {
                    Some((name, _)) => Some(name.clone()),
                    None => {
                        let message = "a `crate`, `self` or `super` import needs a name: \
                                       write `use crate as name;`";
                        self.error(file, range, Code::Unresolved, message);
                        return;
                    }
                },
            };
            let kind = ImportKind::Single {
                name,
                is_self: false,
            };
            self.push_import(ctx, range, path, kind, vis);
        }
    }

    fn self_import(
        &mut self,
        ctx: &ModCtx<'_>,
        prefix: &ModPath,
        tree: &ast::UseTree,
        vis: &RawVisibility,
    ) {
        let range = range_of(tree);
        let Some((last, _)) = prefix.segments.last() else {
            let message = "`self` imports are only allowed in a list with a non-empty prefix";
            return self.error(ctx.file, range, Code::Unresolved, message);
        };
        let name = match tree.rename() {
            // `self as _` binds nothing.
            Some(rename) => rename.name().map(|name| Name::new(name.text())),
            None => Some(last.clone()),
        };
        let kind = ImportKind::Single {
            name,
            is_self: true,
        };
        self.push_import(ctx, range, prefix.clone(), kind, vis);
    }

    /// Appends `own` to `prefix`. A nested tree may not restart the path
    /// with `crate`, `self`, `super` or `::`.
    fn join_paths(&mut self, file: FileId, prefix: &ModPath, own: ModPath) -> Option<ModPath> {
        if prefix.segments.is_empty() && prefix.kind == PathKind::Plain {
            return Some(own);
        }
        if own.kind != PathKind::Plain {
            let message = "`crate`, `self`, `super` and `::` can only start a whole import path";
            self.error(file, own.range, Code::Unresolved, message);
            return None;
        }
        let mut segments = prefix.segments.clone();
        segments.extend(own.segments);
        Some(ModPath {
            kind: prefix.kind,
            segments,
            range: own.range,
        })
    }

    fn push_import(
        &mut self,
        ctx: &ModCtx<'_>,
        range: bluing_base::TextRange,
        path: ModPath,
        kind: ImportKind,
        vis: &RawVisibility,
    ) {
        let id = self.hir.imports.alloc(Import {
            module: ctx.module,
            file: ctx.file,
            range,
            path,
            kind,
            vis: vis.clone(),
        });
        self.hir.modules[ctx.module]
            .items
            .push(ModuleItem::Import(id));
    }
}

fn is_lone_self(path: &ast::Path) -> bool {
    path.qualifier().is_none()
        && path
            .segment()
            .and_then(|segment| segment.kind())
            .is_some_and(|kind| matches!(kind, ast::PathSegmentKind::SelfKw))
}
