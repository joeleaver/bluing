//! Resolution of one path: its start, each segment through modules and
//! enums, and a check that it names the kind of thing its position requires.

use bluing_base::Code;

use super::env::Lookup;
use super::types::{LexScope, TypeResolver};
use super::{Namespace, PathRes, Res};
use crate::hir::{Fields, GenericParamKind, Path, PathKind, PathSegment};
use crate::{DefId, GenericParamId, Name};

/// What a path must resolve to.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Expect {
    Type,
    Trait,
    /// A generic argument: a type, or a constant written as a single name.
    GenericArg,
    /// A value: a function, constant, static, constructor or const parameter.
    Value,
    /// The path of a struct literal or struct pattern.
    Struct,
}

impl Expect {
    /// The namespace the last segment is looked up in.
    fn last_ns(self) -> Namespace {
        match self {
            Expect::Value => Namespace::Values,
            Expect::Type | Expect::Trait | Expect::GenericArg | Expect::Struct => Namespace::Types,
        }
    }
}

impl TypeResolver<'_, '_> {
    pub(super) fn path_res(
        &mut self,
        scope: &LexScope<'_>,
        path: &Path,
        expect: Expect,
    ) -> PathRes {
        if let Some(qself) = &path.qself {
            self.resolve_type(scope, &qself.ty);
            if let Some(trait_ref) = &qself.trait_ref {
                self.resolve_trait_ref(scope, trait_ref);
            }
            return self.partial(scope, path, Res::QualifiedSelf, path.segments.len(), expect);
        }
        let error = PathRes::full(Res::Error);
        let hir = self.env.hir;
        let (mut current, first) = match path.kind {
            PathKind::Crate => (Res::Def(DefId::Module(hir.crate_root(scope.module))), 0),
            PathKind::SelfModule => (Res::Def(DefId::Module(scope.module)), 0),
            PathKind::Super(n) => match (0..n).try_fold(scope.module, |m, _| hir.modules[m].parent)
            {
                Some(ancestor) => (Res::Def(DefId::Module(ancestor)), 0),
                None => {
                    let message = "there are too many leading `super` keywords";
                    self.error(scope.file, path.range, Code::Unresolved, message);
                    return error;
                }
            },
            PathKind::Global => {
                let segment = &path.segments[0];
                match self.env.extern_crate(scope.module, &segment.name) {
                    Some(root) => (Res::Def(DefId::Module(root)), 1),
                    None => {
                        let message = format!("there is no crate named `{}`", segment.name);
                        self.error(scope.file, segment.range, Code::Unresolved, message);
                        return error;
                    }
                }
            }
            PathKind::Plain => match self.first_segment(scope, path, expect) {
                Some(res) => (res, 1),
                None => return error,
            },
        };
        for (index, segment) in path.segments.iter().enumerate().skip(first) {
            let is_last = index + 1 == path.segments.len();
            let ns = if is_last {
                expect.last_ns()
            } else {
                Namespace::Types
            };
            let remaining = path.segments.len() - index;
            current = match current {
                Res::Def(DefId::Module(m)) => {
                    match self.env.lookup_in_module(m, &segment.name, ns, true) {
                        Lookup::Found(binding) => {
                            if !binding.vis.is_accessible_from(hir, scope.module) {
                                let message = format!("`{}` is private", segment.name);
                                self.error(scope.file, segment.range, Code::Private, message);
                            }
                            binding.def.res()
                        }
                        Lookup::NotFound => {
                            // A value position naming a type gets a clearer message.
                            if ns == Namespace::Values
                                && let Lookup::Found(binding) = self.env.lookup_in_module(
                                    m,
                                    &segment.name,
                                    Namespace::Types,
                                    true,
                                )
                            {
                                return self.check_kind(scope, path, binding.def.res(), expect);
                            }
                            let message = format!("cannot find `{}` in this module", segment.name);
                            self.error(scope.file, segment.range, Code::Unresolved, message);
                            return error;
                        }
                        Lookup::Ambiguous => return self.ambiguous(scope, segment),
                        Lookup::Undetermined => unreachable!("imports are resolved before this"),
                    }
                }
                Res::Def(DefId::Enum(e)) => match self.env.variant_binding(e, &segment.name, ns) {
                    Some(binding) => binding.def.res(),
                    None => {
                        if ns == Namespace::Values
                            && let Some(binding) =
                                self.env.variant_binding(e, &segment.name, Namespace::Types)
                        {
                            return self.check_kind(scope, path, binding.def.res(), expect);
                        }
                        // Not a variant: an associated item, for type checking.
                        return self.partial(scope, path, current, remaining, expect);
                    }
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
                    let range = path.segments[index - 1].range;
                    self.error(scope.file, range, Code::Unresolved, message);
                    return error;
                }
                Res::Error => return error,
            };
        }
        self.check_kind(scope, path, current, expect)
    }

    /// Resolves the first segment of a plain path. Returns `None` after
    /// reporting an error.
    fn first_segment(&mut self, scope: &LexScope<'_>, path: &Path, expect: Expect) -> Option<Res> {
        let segment = &path.segments[0];
        let single = path.segments.len() == 1;
        // Only a lone name is looked up as a value; a longer path starts with
        // a module or type.
        let ns = if single {
            expect.last_ns()
        } else {
            Namespace::Types
        };
        match self.lexical(scope, &segment.name, ns) {
            Lookup::Found(res) => Some(res),
            Lookup::NotFound => {
                let fallback = match (single, expect, ns) {
                    // A single name can also be a constant argument.
                    (true, Expect::GenericArg, _) => {
                        match self.lexical(scope, &segment.name, Namespace::Values) {
                            Lookup::Found(
                                res @ (Res::GenericParam(_) | Res::Def(DefId::Const(_))),
                            ) => Some(res),
                            _ => None,
                        }
                    }
                    // A value position naming a type gets a clearer message.
                    (_, _, Namespace::Values) => {
                        match self.lexical(scope, &segment.name, Namespace::Types) {
                            Lookup::Found(res) => {
                                self.check_kind(scope, path, res, expect);
                                return None;
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };
                if fallback.is_none() {
                    self.not_found(scope, segment, ns);
                }
                fallback
            }
            Lookup::Ambiguous => {
                self.ambiguous(scope, segment);
                None
            }
            Lookup::Undetermined => unreachable!("imports are resolved before this"),
        }
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
        // `Self` names the type, and also its constructor in value position.
        if name.as_str() == "Self" {
            return match scope.self_ty {
                Some(self_ty) => Lookup::Found(Res::SelfTy(self_ty)),
                None => Lookup::NotFound,
            };
        }
        self.env.lookup_lexical(scope.module, name, ns, true, false)
    }

    fn not_found(&mut self, scope: &LexScope<'_>, segment: &PathSegment, ns: Namespace) {
        let name = &segment.name;
        let is_const_param = find_generic_param(scope, name, |kind| {
            matches!(kind, GenericParamKind::Const { .. })
        })
        .is_some();
        let (code, message) = if is_const_param && ns == Namespace::Types {
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
        } else if ns == Namespace::Values {
            (
                Code::Unresolved,
                format!("cannot find value `{name}` in this scope"),
            )
        } else {
            (
                Code::Unresolved,
                format!("cannot find `{name}` in this scope"),
            )
        };
        self.error(scope.file, segment.range, code, message);
    }

    fn ambiguous(&mut self, scope: &LexScope<'_>, segment: &PathSegment) -> PathRes {
        let message = format!("`{}` is ambiguous", segment.name);
        self.error(scope.file, segment.range, Code::Ambiguous, message);
        PathRes::full(Res::Error)
    }

    /// A path whose last `remaining` segments name associated items, which
    /// type checking resolves.
    fn partial(
        &mut self,
        scope: &LexScope<'_>,
        path: &Path,
        base: Res,
        remaining: usize,
        expect: Expect,
    ) -> PathRes {
        match expect {
            Expect::Trait => {
                self.error(scope.file, path.range, Code::WrongKind, "expected a trait");
                PathRes::full(Res::Error)
            }
            Expect::Struct => {
                let message =
                    "struct literals and patterns through associated types are not supported yet";
                self.error(scope.file, path.range, Code::NotYetSupported, message);
                PathRes::full(Res::Error)
            }
            Expect::Type | Expect::GenericArg | Expect::Value => PathRes {
                base,
                unresolved_segments: remaining,
            },
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
        let found = match res {
            Res::Error => return PathRes::full(res),
            Res::Def(def) => describe(def),
            Res::PrimTy(_) => "a primitive type",
            Res::SelfTy(_) => "`Self`",
            Res::GenericParam(_) => "a generic parameter",
            Res::QualifiedSelf => "an associated item",
        };
        let ok = match expect {
            Expect::Trait => matches!(res, Res::Def(DefId::Trait(_))),
            Expect::Type => !matches!(
                res,
                Res::Def(
                    DefId::Module(_)
                        | DefId::Variant(_)
                        | DefId::Fn(_)
                        | DefId::Static(_)
                        | DefId::Const(_)
                        | DefId::Trait(_)
                )
            ),
            Expect::GenericArg => !matches!(
                res,
                Res::Def(
                    DefId::Module(_)
                        | DefId::Variant(_)
                        | DefId::Fn(_)
                        | DefId::Static(_)
                        | DefId::Trait(_)
                )
            ),
            Expect::Value => match res {
                Res::Def(DefId::Fn(_) | DefId::Const(_) | DefId::Static(_)) => true,
                // Found in the value namespace: a tuple or unit constructor.
                Res::Def(DefId::Struct(id)) => {
                    !matches!(self.env.hir.structs[id].fields, Fields::Record(_))
                }
                Res::Def(DefId::Variant(id)) => {
                    !matches!(self.env.hir.variants[id].fields, Fields::Record(_))
                }
                Res::SelfTy(_) | Res::GenericParam(_) => true,
                _ => false,
            },
            Expect::Struct => matches!(
                res,
                Res::Def(DefId::Struct(_) | DefId::Variant(_) | DefId::TypeAlias(_))
                    | Res::SelfTy(_)
            ),
        };
        if ok {
            return PathRes::full(res);
        }
        let message = match expect {
            Expect::Trait => format!("expected a trait, found {found}"),
            Expect::Type | Expect::GenericArg if matches!(res, Res::Def(DefId::Trait(_))) => {
                format!(
                    "expected a type, found a trait: write `dyn {}` for a trait object",
                    last_name(path)
                )
            }
            Expect::Type | Expect::GenericArg => format!("expected a type, found {found}"),
            Expect::Value => match res {
                Res::Def(DefId::Struct(_) | DefId::Variant(_)) => {
                    format!("expected a value, found {found} with named fields: use braces")
                }
                _ => format!("expected a value, found {found}"),
            },
            Expect::Struct => format!("expected a struct, variant or type alias, found {found}"),
        };
        self.error(scope.file, path.range, Code::WrongKind, message);
        PathRes::full(Res::Error)
    }
}

/// The innermost generic parameter named `name` whose kind passes `filter`.
pub(crate) fn find_generic_param(
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
