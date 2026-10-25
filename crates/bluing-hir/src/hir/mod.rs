//! The HIR: every crate's module tree and items, lowered from syntax.
//! It is immutable once built; name resolution results live separately.

mod items;
mod types;

pub use items::*;
pub use types::*;

use std::collections::BTreeMap;

use la_arena::Arena;

use crate::{CrateId, DefId, FileId, ModuleId, Name};

#[derive(Debug, Default)]
pub struct Hir {
    pub modules: Arena<Module>,
    pub structs: Arena<Struct>,
    pub enums: Arena<Enum>,
    pub variants: Arena<Variant>,
    pub functions: Arena<Function>,
    pub traits: Arena<Trait>,
    pub impls: Arena<Impl>,
    pub type_aliases: Arena<TypeAlias>,
    pub consts: Arena<Const>,
    pub statics: Arena<Static>,
    pub imports: Arena<Import>,
    /// Items marked `#[lang = "..."]` in library crates, by lang item name.
    pub lang_items: BTreeMap<Name, DefId>,
    /// The number of paths and lifetimes written in signatures; ids below
    /// these are valid.
    pub path_count: u32,
    pub lifetime_count: u32,
}

#[derive(Debug)]
pub struct Module {
    pub krate: CrateId,
    pub parent: Option<ModuleId>,
    /// `None` for a crate root.
    pub name: Option<Name>,
    /// The `mod` declaration, `None` for a crate root.
    pub decl: Option<Loc>,
    /// The file holding the module's items. An inline module shares its
    /// parent's file.
    pub file: FileId,
    pub vis: RawVisibility,
    pub children: Vec<ModuleId>,
    pub items: Vec<ModuleItem>,
}

impl Hir {
    /// The crate root module of each module's crate.
    pub fn crate_root(&self, mut module: ModuleId) -> ModuleId {
        while let Some(parent) = self.modules[module].parent {
            module = parent;
        }
        module
    }

    /// Whether `ancestor` is `module` or one of its ancestors.
    pub fn is_ancestor(&self, ancestor: ModuleId, module: ModuleId) -> bool {
        let mut current = Some(module);
        while let Some(m) = current {
            if m == ancestor {
                return true;
            }
            current = self.modules[m].parent;
        }
        false
    }
}
