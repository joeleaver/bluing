//! Resolved visibilities and accessibility.

use bluing_base::{Code, Diagnostic, FileId};

use crate::hir::{Hir, ModPath, PathKind, RawVisibility};
use crate::{ModuleId, Name};

/// Where an item can be named from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Visibility {
    Public,
    /// Visible in this module and its descendants.
    Restricted(ModuleId),
}

impl Visibility {
    pub fn is_accessible_from(self, hir: &Hir, from: ModuleId) -> bool {
        match self {
            Visibility::Public => true,
            Visibility::Restricted(module) => hir.is_ancestor(module, from),
        }
    }

    /// Whether `self` is visible everywhere `other` is.
    pub fn is_at_least(self, other: Visibility, hir: &Hir) -> bool {
        match (self, other) {
            (Visibility::Public, _) => true,
            (Visibility::Restricted(_), Visibility::Public) => false,
            (Visibility::Restricted(a), Visibility::Restricted(b)) => hir.is_ancestor(a, b),
        }
    }

    /// The narrower of two visibilities. Both must be accessible from a
    /// common module, which makes them comparable.
    pub fn min(self, other: Visibility, hir: &Hir) -> Visibility {
        if self.is_at_least(other, hir) {
            other
        } else {
            self
        }
    }
}

/// Resolves a visibility written in `module`. Reports errors to `diagnostics`.
pub(super) fn resolve(
    hir: &Hir,
    module: ModuleId,
    raw: &RawVisibility,
    file: FileId,
    diagnostics: &mut Vec<Diagnostic>,
) -> Visibility {
    match raw {
        RawVisibility::Public => Visibility::Public,
        RawVisibility::Private | RawVisibility::SelfModule => Visibility::Restricted(module),
        RawVisibility::Crate => Visibility::Restricted(hir.crate_root(module)),
        RawVisibility::Super => match hir.modules[module].parent {
            Some(parent) => Visibility::Restricted(parent),
            None => {
                let range = hir.modules[module]
                    .decl
                    .map(|loc| loc.range)
                    .unwrap_or_default();
                let message = "`pub(super)` is not allowed at the crate root";
                diagnostics.push(Diagnostic::new(file, range, Code::Visibility, message));
                Visibility::Restricted(module)
            }
        },
        RawVisibility::In(path) => resolve_in_path(hir, module, path, file, diagnostics),
    }
}

/// `pub(in path)`: the path must name `module` or one of its ancestors. It is
/// walked through the module tree, since only ancestors are valid targets.
fn resolve_in_path(
    hir: &Hir,
    module: ModuleId,
    path: &ModPath,
    file: FileId,
    diagnostics: &mut Vec<Diagnostic>,
) -> Visibility {
    let mut error = |message: String| {
        diagnostics.push(Diagnostic::new(file, path.range, Code::Visibility, message));
        // Reported; treat as public so uses don't also report privacy errors.
        Visibility::Public
    };
    let start = match path.kind {
        PathKind::Crate => Some(hir.crate_root(module)),
        PathKind::SelfModule => Some(module),
        PathKind::Super(n) => (0..n).try_fold(module, |m, _| hir.modules[m].parent),
        PathKind::Plain | PathKind::Global => None,
    };
    let Some(mut target) = start else {
        return error("invalid path in `pub(in ...)`".to_owned());
    };
    for (name, _) in &path.segments {
        match child_module(hir, target, name) {
            Some(child) => target = child,
            None => return error(format!("no module `{name}` in this path")),
        }
    }
    if hir.is_ancestor(target, module) {
        Visibility::Restricted(target)
    } else {
        error("visibilities can only be restricted to ancestor modules".to_owned())
    }
}

fn child_module(hir: &Hir, module: ModuleId, name: &Name) -> Option<ModuleId> {
    hir.modules[module]
        .children
        .iter()
        .copied()
        .find(|&child| hir.modules[child].name.as_ref() == Some(name))
}
