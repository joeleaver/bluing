//! Resolution of the paths, bounds and lifetimes inside types.

use bluing_base::{Code, Diagnostic, FileId, TextRange};

use super::env::NameEnv;
use super::paths::{Expect, find_generic_param};
use super::{LifetimeRes, PathRes, SelfTyOf};
use crate::hir::{
    AssocBindingValue, GenericArg, GenericArgs, GenericParamKind, GenericParams, LifetimeRef, Path,
    TypeBound, TypeRef, TypeRefKind, WherePredicate,
};
use crate::{GenericOwner, ModuleId};

/// The lexical context of a signature.
pub(crate) struct LexScope<'h> {
    pub module: ModuleId,
    pub file: FileId,
    /// Generic parameter lists in scope, outermost first.
    pub generics: Vec<(GenericOwner, &'h GenericParams)>,
    /// What `Self` means here, if anything.
    pub self_ty: Option<SelfTyOf>,
    /// Resolving the self type of an impl, where `Self` is not allowed.
    pub in_impl_self_type: bool,
}

/// Resolves the paths and lifetimes in types, recording each resolution in
/// the given tables (indexed by path and lifetime id).
pub(crate) struct TypeResolver<'e, 'o> {
    pub env: &'e NameEnv<'e>,
    pub diagnostics: &'o mut Vec<Diagnostic>,
    pub paths: &'o mut [Option<PathRes>],
    pub lifetimes: &'o mut [Option<LifetimeRes>],
}

impl TypeResolver<'_, '_> {
    pub(super) fn error(
        &mut self,
        file: FileId,
        range: TextRange,
        code: Code,
        message: impl Into<String>,
    ) {
        self.diagnostics
            .push(Diagnostic::new(file, range, code, message));
    }

    pub(crate) fn resolve_type(&mut self, scope: &LexScope<'_>, ty: &TypeRef) {
        match &ty.kind {
            TypeRefKind::Path(path) => self.resolve_path(scope, path, Expect::Type),
            TypeRefKind::Reference {
                lifetime, inner, ..
            } => {
                if let Some(lifetime) = lifetime {
                    self.resolve_lifetime(scope, lifetime);
                }
                self.resolve_type(scope, inner);
            }
            TypeRefKind::RawPtr { inner, .. } | TypeRefKind::Slice(inner) => {
                self.resolve_type(scope, inner)
            }
            TypeRefKind::Array { elem, .. } => self.resolve_type(scope, elem),
            TypeRefKind::Tuple(types) => {
                for ty in types {
                    self.resolve_type(scope, ty);
                }
            }
            TypeRefKind::FnPtr { params, ret, .. } => {
                for ty in params {
                    self.resolve_type(scope, ty);
                }
                self.resolve_type(scope, ret);
            }
            TypeRefKind::ImplTrait(bounds) | TypeRefKind::DynTrait(bounds) => {
                for bound in bounds {
                    self.resolve_bound(scope, bound);
                }
            }
            TypeRefKind::Never | TypeRefKind::Infer | TypeRefKind::Error => {}
        }
    }

    pub(crate) fn resolve_bound(&mut self, scope: &LexScope<'_>, bound: &TypeBound) {
        match bound {
            TypeBound::Trait { path, .. } => self.resolve_path(scope, path, Expect::Trait),
            TypeBound::Lifetime(lifetime) => self.resolve_lifetime(scope, lifetime),
            TypeBound::Error => {}
        }
    }

    pub(crate) fn resolve_trait_ref(&mut self, scope: &LexScope<'_>, path: &Path) {
        self.resolve_path(scope, path, Expect::Trait);
    }

    pub(crate) fn resolve_generics(&mut self, scope: &LexScope<'_>, generics: &GenericParams) {
        for param in &generics.params {
            match &param.kind {
                GenericParamKind::Lifetime { bounds } => {
                    for lifetime in bounds {
                        self.resolve_lifetime(scope, lifetime);
                    }
                }
                GenericParamKind::Type { bounds, default } => {
                    for bound in bounds {
                        self.resolve_bound(scope, bound);
                    }
                    if let Some(default) = default {
                        self.resolve_type(scope, default);
                    }
                }
                GenericParamKind::Const { ty, .. } => self.resolve_type(scope, ty),
            }
        }
        for predicate in &generics.where_predicates {
            match predicate {
                WherePredicate::Type { ty, bounds } => {
                    self.resolve_type(scope, ty);
                    for bound in bounds {
                        self.resolve_bound(scope, bound);
                    }
                }
                WherePredicate::Lifetime { lifetime, bounds } => {
                    self.resolve_lifetime(scope, lifetime);
                    for bound in bounds {
                        self.resolve_lifetime(scope, bound);
                    }
                }
            }
        }
    }

    pub(crate) fn resolve_lifetime(&mut self, scope: &LexScope<'_>, lifetime: &LifetimeRef) {
        let res = match lifetime.name.as_str() {
            "static" => LifetimeRes::Static,
            "_" => LifetimeRes::Anonymous,
            _ => match find_generic_param(scope, &lifetime.name, |kind| {
                matches!(kind, GenericParamKind::Lifetime { .. })
            }) {
                Some(id) => LifetimeRes::Param(id),
                None => {
                    let message = format!("use of undeclared lifetime `'{}`", lifetime.name);
                    self.error(
                        scope.file,
                        lifetime.range,
                        Code::UndeclaredLifetime,
                        message,
                    );
                    LifetimeRes::Error
                }
            },
        };
        self.lifetimes[lifetime.id.0 as usize] = Some(res);
    }

    pub(crate) fn resolve_generic_args(&mut self, scope: &LexScope<'_>, args: &GenericArgs) {
        for arg in &args.args {
            match arg {
                GenericArg::Type(TypeRef {
                    kind: TypeRefKind::Path(path),
                    ..
                }) => {
                    self.resolve_path(scope, path, Expect::GenericArg);
                }
                GenericArg::Type(ty) => self.resolve_type(scope, ty),
                GenericArg::Lifetime(lifetime) => self.resolve_lifetime(scope, lifetime),
                // Constant expressions are resolved with bodies.
                GenericArg::Const(_) => {}
            }
        }
        for binding in &args.bindings {
            if let Some(args) = &binding.args {
                self.resolve_generic_args(scope, args);
            }
            match &binding.value {
                AssocBindingValue::Type(ty) => self.resolve_type(scope, ty),
                AssocBindingValue::Bounds(bounds) => {
                    for bound in bounds {
                        self.resolve_bound(scope, bound);
                    }
                }
            }
        }
    }

    /// Resolves a path in a value position: `f`, `Enum::Variant`, `CONST`.
    pub(crate) fn resolve_value_path(&mut self, scope: &LexScope<'_>, path: &Path) {
        self.resolve_path(scope, path, Expect::Value);
    }

    /// Resolves the path of a struct literal or struct pattern.
    pub(crate) fn resolve_struct_path(&mut self, scope: &LexScope<'_>, path: &Path) {
        self.resolve_path(scope, path, Expect::Struct);
    }

    fn resolve_path(&mut self, scope: &LexScope<'_>, path: &Path, expect: Expect) {
        let res = self.path_res(scope, path, expect);
        self.paths[path.id.0 as usize] = Some(res);
        for segment in &path.segments {
            if let Some(args) = &segment.args {
                self.resolve_generic_args(scope, args);
            }
        }
    }
}
