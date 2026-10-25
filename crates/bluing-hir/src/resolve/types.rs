//! Resolution of the paths, bounds and lifetimes inside types.

use bluing_base::{Code, FileId};

use super::lookup::Lookup;
use super::{LifetimeRes, Namespace, PathRes, Res, Resolver, SelfTyOf};
use crate::hir::{
    AssocBindingValue, GenericArg, GenericArgs, GenericParamKind, GenericParams, LifetimeRef, Path,
    PathKind, TypeBound, TypeRef, WherePredicate,
};
use crate::{DefId, GenericOwner, GenericParamId, ModuleId, Name};

/// The lexical context of a signature.
pub(super) struct LexScope<'h> {
    pub module: ModuleId,
    pub file: FileId,
    /// Generic parameter lists in scope, outermost first.
    pub generics: Vec<(GenericOwner, &'h GenericParams)>,
    /// What `Self` means here, if anything.
    pub self_ty: Option<SelfTyOf>,
    /// Resolving the self type of an impl, where `Self` is not allowed.
    pub in_impl_self_type: bool,
}

/// What a path must resolve to.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Expect {
    Type,
    Trait,
    /// A generic argument: a type, or a constant written as a single name.
    GenericArg,
}

impl Resolver<'_> {
    pub(super) fn resolve_type(&mut self, scope: &LexScope<'_>, ty: &TypeRef) {
        match ty {
            TypeRef::Path(path) => self.resolve_path(scope, path, Expect::Type),
            TypeRef::Reference {
                lifetime, inner, ..
            } => {
                if let Some(lifetime) = lifetime {
                    self.resolve_lifetime(scope, lifetime);
                }
                self.resolve_type(scope, inner);
            }
            TypeRef::RawPtr { inner, .. } | TypeRef::Slice(inner) => {
                self.resolve_type(scope, inner)
            }
            TypeRef::Array { elem, .. } => self.resolve_type(scope, elem),
            TypeRef::Tuple(types) => {
                for ty in types {
                    self.resolve_type(scope, ty);
                }
            }
            TypeRef::FnPtr { params, ret, .. } => {
                for ty in params {
                    self.resolve_type(scope, ty);
                }
                self.resolve_type(scope, ret);
            }
            TypeRef::ImplTrait(bounds) | TypeRef::DynTrait(bounds) => {
                for bound in bounds {
                    self.resolve_bound(scope, bound);
                }
            }
            TypeRef::Never | TypeRef::Infer | TypeRef::Error => {}
        }
    }

    pub(super) fn resolve_bound(&mut self, scope: &LexScope<'_>, bound: &TypeBound) {
        match bound {
            TypeBound::Trait { path, .. } => self.resolve_path(scope, path, Expect::Trait),
            TypeBound::Lifetime(lifetime) => self.resolve_lifetime(scope, lifetime),
            TypeBound::Error => {}
        }
    }

    pub(super) fn resolve_trait_ref(&mut self, scope: &LexScope<'_>, path: &Path) {
        self.resolve_path(scope, path, Expect::Trait);
    }

    pub(super) fn resolve_generics(&mut self, scope: &LexScope<'_>, generics: &GenericParams) {
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

    pub(super) fn resolve_lifetime(&mut self, scope: &LexScope<'_>, lifetime: &LifetimeRef) {
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
        self.out.lifetimes[lifetime.id.0 as usize] = Some(res);
    }

    fn resolve_generic_args(&mut self, scope: &LexScope<'_>, args: &GenericArgs) {
        for arg in &args.args {
            match arg {
                GenericArg::Type(TypeRef::Path(path)) => {
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

    fn resolve_path(&mut self, scope: &LexScope<'_>, path: &Path, expect: Expect) {
        let res = self.path_res(scope, path, expect);
        self.out.paths[path.id.0 as usize] = Some(res);
        for segment in &path.segments {
            if let Some(args) = &segment.args {
                self.resolve_generic_args(scope, args);
            }
        }
    }

    fn path_res(&mut self, scope: &LexScope<'_>, path: &Path, expect: Expect) -> PathRes {
        if let Some(qself) = &path.qself {
            self.resolve_type(scope, &qself.ty);
            if let Some(trait_ref) = &qself.trait_ref {
                self.resolve_trait_ref(scope, trait_ref);
            }
            return PathRes {
                base: Res::QualifiedSelf,
                unresolved_segments: path.segments.len(),
            };
        }
        let error = PathRes::full(Res::Error);
        let (mut current, first) = match path.kind {
            PathKind::Crate => (
                Res::Def(DefId::Module(self.hir.crate_root(scope.module))),
                0,
            ),
            PathKind::SelfModule => (Res::Def(DefId::Module(scope.module)), 0),
            PathKind::Super(n) => {
                match (0..n).try_fold(scope.module, |m, _| self.hir.modules[m].parent) {
                    Some(ancestor) => (Res::Def(DefId::Module(ancestor)), 0),
                    None => {
                        let message = "there are too many leading `super` keywords";
                        self.error(scope.file, path.range, Code::Unresolved, message);
                        return error;
                    }
                }
            }
            PathKind::Global => {
                let segment = &path.segments[0];
                match self.extern_crate(scope.module, &segment.name) {
                    Some(root) => (Res::Def(DefId::Module(root)), 1),
                    None => {
                        let message = format!("there is no crate named `{}`", segment.name);
                        self.error(scope.file, segment.range, Code::Unresolved, message);
                        return error;
                    }
                }
            }
            PathKind::Plain => {
                let segment = &path.segments[0];
                let single = path.segments.len() == 1;
                match self.lexical(scope, &segment.name, Namespace::Types) {
                    Lookup::Found(res) => (res, 1),
                    Lookup::NotFound if single && expect == Expect::GenericArg => {
                        // A single name can also be a constant argument.
                        match self.lexical(scope, &segment.name, Namespace::Values) {
                            Lookup::Found(
                                res @ (Res::GenericParam(_) | Res::Def(DefId::Const(_))),
                            ) => {
                                return PathRes::full(res);
                            }
                            _ => return self.not_found(scope, segment),
                        }
                    }
                    Lookup::NotFound => return self.not_found(scope, segment),
                    Lookup::Ambiguous => {
                        let message = format!("`{}` is ambiguous", segment.name);
                        self.error(scope.file, segment.range, Code::Ambiguous, message);
                        return error;
                    }
                    Lookup::Undetermined => unreachable!("imports are resolved before signatures"),
                }
            }
        };
        // Every later segment is looked up in the type namespace: signatures
        // only contain types and traits, and constant arguments with more
        // than one segment must be written in braces.
        let ns = Namespace::Types;
        for (index, segment) in path.segments.iter().enumerate().skip(first) {
            let remaining = path.segments.len() - index;
            current = match current {
                Res::Def(DefId::Module(m)) => {
                    match self.lookup_in_module(m, &segment.name, ns, true) {
                        Lookup::Found(binding) => {
                            if !binding.vis.is_accessible_from(self.hir, scope.module) {
                                let message = format!("`{}` is private", segment.name);
                                self.error(scope.file, segment.range, Code::Private, message);
                            }
                            binding.def.res()
                        }
                        Lookup::NotFound => {
                            let message = format!("cannot find `{}` in this module", segment.name);
                            self.error(scope.file, segment.range, Code::Unresolved, message);
                            return error;
                        }
                        Lookup::Ambiguous => {
                            let message = format!("`{}` is ambiguous", segment.name);
                            self.error(scope.file, segment.range, Code::Ambiguous, message);
                            return error;
                        }
                        Lookup::Undetermined => {
                            unreachable!("imports are resolved before signatures")
                        }
                    }
                }
                Res::Def(DefId::Enum(e)) => match self.variant_binding(e, &segment.name, ns) {
                    Some(binding) => binding.def.res(),
                    // Not a variant: an associated item, for type checking.
                    None => return self.partial(scope, path, current, remaining, expect),
                },
                Res::Def(DefId::Struct(_) | DefId::TypeAlias(_) | DefId::Trait(_))
                | Res::PrimTy(_)
                | Res::SelfTy(_)
                | Res::GenericParam(_)
                | Res::QualifiedSelf => {
                    return self.partial(scope, path, current, remaining, expect);
                }
                Res::Def(
                    def @ (DefId::Variant(_) | DefId::Fn(_) | DefId::Const(_) | DefId::Static(_)),
                ) => {
                    let message = format!("{} has no items to name with `::`", describe(def));
                    self.error(
                        scope.file,
                        path.segments[index - 1].range,
                        Code::Unresolved,
                        message,
                    );
                    return error;
                }
                Res::Error => return error,
            };
        }
        self.check_kind(scope, path, current, expect)
    }

    /// The first segment of a plain path: generic parameters, `Self`, then
    /// the module's lexical scope.
    fn lexical(&self, scope: &LexScope<'_>, name: &Name, ns: Namespace) -> Lookup<Res> {
        let param = find_generic_param(scope, name, |kind| match ns {
            Namespace::Types => matches!(kind, GenericParamKind::Type { .. }),
            Namespace::Values => matches!(kind, GenericParamKind::Const { .. }),
        });
        if let Some(id) = param {
            return Lookup::Found(Res::GenericParam(id));
        }
        if ns == Namespace::Types && name.as_str() == "Self" {
            return match scope.self_ty {
                Some(self_ty) => Lookup::Found(Res::SelfTy(self_ty)),
                None => Lookup::NotFound,
            };
        }
        self.lookup_lexical(scope.module, name, ns, true, false)
    }

    fn not_found(&mut self, scope: &LexScope<'_>, segment: &crate::hir::PathSegment) -> PathRes {
        let name = &segment.name;
        let is_const_param = find_generic_param(scope, name, |kind| {
            matches!(kind, GenericParamKind::Const { .. })
        })
        .is_some();
        let (code, message) = if is_const_param {
            (
                Code::WrongKind,
                format!("expected a type, found const parameter `{name}`"),
            )
        } else if name.as_str() == "Self" {
            let message = if scope.in_impl_self_type {
                "`Self` is not valid in the self type of an impl"
            } else {
                "`Self` is only available in impls, traits and type definitions"
            };
            (Code::Unresolved, message.to_owned())
        } else {
            (
                Code::Unresolved,
                format!("cannot find `{name}` in this scope"),
            )
        };
        self.error(scope.file, segment.range, code, message);
        PathRes::full(Res::Error)
    }

    /// A path whose last `remaining` segments name associated items.
    fn partial(
        &mut self,
        scope: &LexScope<'_>,
        path: &Path,
        base: Res,
        remaining: usize,
        expect: Expect,
    ) -> PathRes {
        if expect == Expect::Trait {
            self.error(scope.file, path.range, Code::WrongKind, "expected a trait");
            return PathRes::full(Res::Error);
        }
        PathRes {
            base,
            unresolved_segments: remaining,
        }
    }

    /// Checks that a fully resolved path names the kind of thing required.
    fn check_kind(
        &mut self,
        scope: &LexScope<'_>,
        path: &Path,
        res: Res,
        expect: Expect,
    ) -> PathRes {
        let problem = match (expect, res) {
            (_, Res::Error) => None,
            (Expect::Trait, Res::Def(DefId::Trait(_))) => None,
            (Expect::Trait, Res::Def(def)) => {
                Some(format!("expected a trait, found {}", describe(def)))
            }
            (Expect::Trait, _) => Some("expected a trait, found a type".to_owned()),
            (_, Res::Def(DefId::Trait(_))) => Some(format!(
                "expected a type, found a trait: write `dyn {}` for a trait object",
                last_name(path)
            )),
            (
                _,
                Res::Def(
                    def @ (DefId::Module(_) | DefId::Variant(_) | DefId::Fn(_) | DefId::Static(_)),
                ),
            ) => Some(format!("expected a type, found {}", describe(def))),
            (Expect::Type, Res::Def(def @ DefId::Const(_))) => {
                Some(format!("expected a type, found {}", describe(def)))
            }
            _ => None,
        };
        match problem {
            Some(message) => {
                self.error(scope.file, path.range, Code::WrongKind, message);
                PathRes::full(Res::Error)
            }
            None => PathRes::full(res),
        }
    }
}

/// The innermost generic parameter named `name` whose kind passes `filter`.
fn find_generic_param(
    scope: &LexScope<'_>,
    name: &Name,
    filter: impl Fn(&GenericParamKind) -> bool,
) -> Option<GenericParamId> {
    scope.generics.iter().rev().find_map(|&(owner, generics)| {
        generics
            .params
            .iter()
            .position(|param| &param.name == name && filter(&param.kind))
            .map(|index| GenericParamId {
                owner,
                index: index as u32,
            })
    })
}

fn describe(def: DefId) -> &'static str {
    match def {
        DefId::Module(_) => "a module",
        DefId::Struct(_) => "a struct",
        DefId::Enum(_) => "an enum",
        DefId::Variant(_) => "an enum variant",
        DefId::Fn(_) => "a function",
        DefId::Trait(_) => "a trait",
        DefId::TypeAlias(_) => "a type alias",
        DefId::Const(_) => "a constant",
        DefId::Static(_) => "a static",
    }
}

fn last_name(path: &Path) -> String {
    path.segments
        .last()
        .map_or_else(String::new, |segment| segment.name.to_string())
}
