//! Builds the HIR: loads each crate's files by following `mod` declarations
//! and lowers every item.

mod assoc;
mod attrs;
mod items;
mod types;
mod use_tree;

use std::collections::btree_map::Entry;

use bluing_base::{Code, Diagnostic, FileId, TextRange};
use bluing_syntax::{
    Options,
    ra_ap_syntax::{
        AstNode,
        ast::{self, HasAttrs, HasModuleItem, HasName, HasVisibility},
    },
};
use la_arena::ArenaMap;

use crate::files::{FileData, Files};
use crate::hir::{Hir, Loc, Module, ModuleItem, RawVisibility};
use crate::{CrateGraph, CrateId, CrateKind, DefId, ModuleId, Name, SourceProvider, VfsPath};

pub(crate) use types::TypeLowerer;

/// The file extension of Bluing source files.
pub const EXTENSION: &str = "blu";

/// The items the compiler knows by `#[lang = "..."]` name.
const LANG_ITEMS: &[&str] = &["sized", "copy", "clone", "Option", "Some", "None"];

/// Whether an item may carry a visibility qualifier.
#[derive(Clone, Copy, PartialEq, Eq)]
enum VisRule {
    Allowed,
    /// Not permitted: report a qualifier as an error.
    Forbidden,
    /// Not permitted, and the parser's own validation already reports it.
    ForbiddenReportedByParser,
}

/// A crate root that the source provider could not supply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingCrateRoot(pub VfsPath);

pub(crate) struct Collected {
    pub hir: Hir,
    pub files: Files,
    pub crate_roots: ArenaMap<CrateId, ModuleId>,
    pub diagnostics: Vec<Diagnostic>,
}

pub(crate) fn collect(
    graph: &CrateGraph,
    provider: &dyn SourceProvider,
) -> Result<Collected, MissingCrateRoot> {
    let mut collector = Collector {
        provider,
        hir: Hir::default(),
        files: Files::default(),
        crate_roots: ArenaMap::default(),
        diagnostics: Vec::new(),
    };
    for (krate, data) in graph.crates() {
        let text = provider
            .read(&data.root)
            .ok_or_else(|| MissingCrateRoot(data.root.clone()))?;
        let options = match data.kind {
            CrateKind::Script { allow_unsafe } => Options {
                allow_unsafe,
                library: false,
            },
            CrateKind::Library => Options {
                allow_unsafe: true,
                library: true,
            },
        };
        let file = collector.load(data.root.clone(), text, &options);
        let module = collector.hir.modules.alloc(Module {
            krate,
            parent: None,
            name: None,
            decl: None,
            file,
            vis: RawVisibility::Public,
            children: Vec::new(),
            items: Vec::new(),
        });
        collector.crate_roots.insert(krate, module);
        let ctx = ModCtx {
            module,
            file,
            dir: data.root.parent().unwrap_or_else(|| VfsPath::new("")),
            options: &options,
            library: data.kind == CrateKind::Library,
        };
        let tree = collector.files.get(file).tree.clone();
        collector.collect_items(&ctx, tree.items());
    }
    Ok(Collected {
        hir: collector.hir,
        files: collector.files,
        crate_roots: collector.crate_roots,
        diagnostics: collector.diagnostics,
    })
}

struct Collector<'a> {
    provider: &'a dyn SourceProvider,
    hir: Hir,
    files: Files,
    crate_roots: ArenaMap<CrateId, ModuleId>,
    diagnostics: Vec<Diagnostic>,
}

/// Where the items being collected belong.
struct ModCtx<'a> {
    module: ModuleId,
    file: FileId,
    /// The directory in which `mod name;` looks for `name.blu`.
    dir: VfsPath,
    options: &'a Options,
    library: bool,
}

impl Collector<'_> {
    fn error(&mut self, file: FileId, range: TextRange, code: Code, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::new(file, range, code, message));
    }

    fn load(&mut self, path: VfsPath, text: String, options: &Options) -> FileId {
        let file = self.files.next_id();
        let parsed = bluing_syntax::parse(file, &text, options);
        self.diagnostics.extend(parsed.diagnostics);
        let added = self.files.add(FileData {
            path,
            text,
            tree: parsed.tree,
        });
        debug_assert_eq!(added, file);
        file
    }

    fn types(&mut self, file: FileId) -> TypeLowerer<'_> {
        TypeLowerer {
            file,
            path_count: &mut self.hir.path_count,
            lifetime_count: &mut self.hir.lifetime_count,
            diagnostics: &mut self.diagnostics,
        }
    }

    fn collect_items(&mut self, ctx: &ModCtx<'_>, items: impl Iterator<Item = ast::Item>) {
        for item in items {
            let lowered = match &item {
                ast::Item::Module(module) => self.module(ctx, module).map(ModuleItem::Module),
                ast::Item::Use(use_item) => {
                    self.use_item(ctx, use_item);
                    None
                }
                ast::Item::Struct(it) => self.struct_(ctx, it).map(ModuleItem::Struct),
                ast::Item::Enum(it) => self.enum_(ctx, it).map(ModuleItem::Enum),
                ast::Item::Fn(it) => self.module_fn(ctx, it).map(ModuleItem::Fn),
                ast::Item::Trait(it) => self.trait_(ctx, it).map(ModuleItem::Trait),
                ast::Item::Impl(it) => self.impl_(ctx, it).map(ModuleItem::Impl),
                ast::Item::TypeAlias(it) => self.module_type_alias(ctx, it),
                ast::Item::Const(it) => self.module_const(ctx, it),
                ast::Item::Static(it) => self.static_(ctx, it).map(ModuleItem::Static),
                ast::Item::Union(it) => {
                    let message = "unions are not supported yet";
                    self.error(ctx.file, range_of(it), Code::NotYetSupported, message);
                    None
                }
                ast::Item::ExternCrate(it) => {
                    let message = "`extern crate` is not supported: a Bluing package's only \
                                   dependency is `std`, which is always available";
                    self.error(ctx.file, range_of(it), Code::Unsupported, message);
                    None
                }
                ast::Item::MacroDef(it) => {
                    let message = "`macro` items are not supported in Bluing";
                    self.error(ctx.file, range_of(it), Code::Unsupported, message);
                    None
                }
                ast::Item::MacroCall(call) => {
                    // Inline assembly macros were already rejected.
                    if !bluing_syntax::is_inline_asm_call(call) {
                        let message = "macro calls in item position are not supported yet";
                        self.error(ctx.file, range_of(call), Code::NotYetSupported, message);
                    }
                    None
                }
                // Already rejected by the subset check.
                ast::Item::ExternBlock(_) | ast::Item::MacroRules(_) | ast::Item::AsmExpr(_) => {
                    None
                }
            };
            if let Some(lowered) = lowered {
                self.hir.modules[ctx.module].items.push(lowered);
            }
        }
    }

    fn module(&mut self, ctx: &ModCtx<'_>, module: &ast::Module) -> Option<ModuleId> {
        let name_node = module.name()?;
        let name = Name::new(name_node.text());
        let decl = Loc {
            file: ctx.file,
            range: range_of(&name_node),
        };
        let vis = self.visibility(ctx.file, module.visibility(), VisRule::Allowed);
        let dir = ctx.dir.join(name.as_str());
        let (file, items): (FileId, Option<Box<dyn Iterator<Item = ast::Item>>>) =
            match module.item_list() {
                Some(list) => (ctx.file, Some(Box::new(list.items()))),
                None => match self.module_file(ctx, &name, decl) {
                    Some(file) => {
                        let tree = self.files.get(file).tree.clone();
                        (file, Some(Box::new(tree.items())))
                    }
                    // Keep an empty module so paths into it report the
                    // missing file once, not again at every use.
                    None => (ctx.file, None),
                },
            };
        let id = self.hir.modules.alloc(Module {
            krate: self.hir.modules[ctx.module].krate,
            parent: Some(ctx.module),
            name: Some(name),
            decl: Some(decl),
            file,
            vis,
            children: Vec::new(),
            items: Vec::new(),
        });
        self.hir.modules[ctx.module].children.push(id);
        self.lang(ctx, module, Some(DefId::Module(id)));
        if let Some(items) = items {
            let child = ModCtx {
                module: id,
                file,
                dir,
                options: ctx.options,
                library: ctx.library,
            };
            self.collect_items(&child, items);
        }
        Some(id)
    }

    /// Finds and loads the file for `mod name;`.
    fn module_file(&mut self, ctx: &ModCtx<'_>, name: &Name, decl: Loc) -> Option<FileId> {
        let flat = ctx.dir.join(&format!("{name}.{EXTENSION}"));
        let nested = ctx
            .dir
            .join(name.as_str())
            .join(&format!("mod.{EXTENSION}"));
        let found: Vec<(VfsPath, String)> = [flat.clone(), nested.clone()]
            .into_iter()
            .filter_map(|path| self.provider.read(&path).map(|text| (path, text)))
            .collect();
        // Messages show the candidates relative to the declaring file.
        let base = self.files.get(decl.file).path.parent();
        let relative = |path: &VfsPath| -> String {
            base.as_ref()
                .and_then(|base| path.as_str().strip_prefix(&format!("{base}/")))
                .unwrap_or(path.as_str())
                .to_owned()
        };
        let (flat, nested) = (relative(&flat), relative(&nested));
        match <[_; 1]>::try_from(found) {
            Ok([(path, text)]) => Some(self.load(path, text, ctx.options)),
            Err(found) if found.is_empty() => {
                let message = format!(
                    "file not found for module `{name}`: create `{flat}` or `{nested}` next to \
                     this file"
                );
                self.error(decl.file, decl.range, Code::ModuleFile, message);
                None
            }
            Err(_) => {
                let message =
                    format!("module `{name}` has two files, `{flat}` and `{nested}`; remove one");
                self.error(decl.file, decl.range, Code::ModuleFile, message);
                None
            }
        }
    }

    /// Records the `#[lang = "..."]` attributes on an item.
    fn lang(&mut self, ctx: &ModCtx<'_>, node: &impl HasAttrs, def: Option<DefId>) {
        for attr in attrs::lang(node) {
            let (name, range) = match attr {
                Ok(found) => found,
                Err(range) => {
                    let message = "`#[lang]` expects a string literal";
                    self.error(ctx.file, range, Code::Attribute, message);
                    continue;
                }
            };
            // Outside library crates the syntax check already rejected it.
            if !ctx.library {
                continue;
            }
            let Some(def) = def else {
                let message = "`#[lang]` is not allowed on this item";
                self.error(ctx.file, range, Code::Attribute, message);
                continue;
            };
            if !LANG_ITEMS.contains(&name.as_str()) {
                let message = format!("unknown lang item `{name}`");
                self.error(ctx.file, range, Code::Attribute, message);
                continue;
            }
            match self.hir.lang_items.entry(name) {
                Entry::Vacant(entry) => {
                    entry.insert(def);
                }
                Entry::Occupied(entry) => {
                    let message = format!("lang item `{}` is defined more than once", entry.key());
                    self.error(ctx.file, range, Code::Duplicate, message);
                }
            }
        }
    }
}

fn range_of(node: &impl AstNode) -> TextRange {
    node.syntax().text_range()
}
