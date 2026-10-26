//! Checks what names resolve to, not just that they resolve.

mod support;

use bluing_hir::hir::{Function, GenericArg, Path, TypeRef, TypeRefKind};
use bluing_hir::resolve::{LifetimeRes, PathRes, PrimTy, Res, ScopeDef, SelfTyOf, Visibility};
use bluing_hir::{Analysis, DefId, GenericOwner, GenericParamId, Name};
use support::{analyze_files, render};

fn analyze_ok(text: &str) -> Analysis {
    let analysis = analyze_files(&[("main.blu", text)]);
    assert!(analysis.diagnostics.is_empty(), "{}", render(&analysis));
    analysis
}

fn function<'a>(analysis: &'a Analysis, name: &str) -> &'a Function {
    analysis
        .hir
        .functions
        .iter()
        .map(|(_, f)| f)
        .find(|f| f.name.as_str() == name)
        .unwrap_or_else(|| panic!("no function `{name}`"))
}

fn param_path<'a>(analysis: &'a Analysis, fn_name: &str, index: usize) -> &'a Path {
    match &function(analysis, fn_name).params[index].ty.kind {
        TypeRefKind::Path(path) => path,
        TypeRefKind::Reference { inner, .. } => match &inner.kind {
            TypeRefKind::Path(path) => path,
            other => panic!("expected a path, found {other:?}"),
        },
        other => panic!("expected a path, found {other:?}"),
    }
}

fn param_res(analysis: &Analysis, fn_name: &str, index: usize) -> PathRes {
    let path = param_path(analysis, fn_name, index);
    analysis
        .resolutions
        .path(path.id)
        .expect("every signature path is resolved")
}

fn struct_def(analysis: &Analysis, name: &str, module: Option<&str>) -> DefId {
    let (id, _) = analysis
        .hir
        .structs
        .iter()
        .find(|(_, s)| {
            s.name.as_str() == name
                && module.is_none_or(|m| {
                    analysis.hir.modules[s.module]
                        .name
                        .as_ref()
                        .map(Name::as_str)
                        == Some(m)
                })
        })
        .unwrap_or_else(|| panic!("no struct `{name}`"));
    DefId::Struct(id)
}

/// The only impl in the script crate (the last crate).
fn script_impl(analysis: &Analysis) -> bluing_hir::ImplId {
    let script =
        analysis.hir.modules[*analysis.resolutions.crate_roots.values().last().unwrap()].krate;
    let mut impls = analysis
        .hir
        .impls
        .iter()
        .filter(|(_, imp)| analysis.hir.modules[imp.module].krate == script);
    let (id, _) = impls.next().expect("an impl in the script");
    assert!(impls.next().is_none(), "more than one impl in the script");
    id
}

#[test]
fn explicit_imports_shadow_globs() {
    let analysis = analyze_ok(
        "mod a { pub struct X; }
         mod b { pub struct X; }
         use a::*;
         use b::X;
         pub fn f(_: X) {}",
    );
    let expected = struct_def(&analysis, "X", Some("b"));
    assert_eq!(
        param_res(&analysis, "f", 0),
        PathRes::full(Res::Def(expected))
    );
}

#[test]
fn prelude_names_resolve_into_std() {
    let analysis = analyze_ok("pub fn f(_: Option<u8>) {}");
    let option = analysis.hir.lang_items[&Name::new("Option")];
    assert_eq!(
        param_res(&analysis, "f", 0),
        PathRes::full(Res::Def(option))
    );
    let path = param_path(&analysis, "f", 0);
    let Some(GenericArg::Type(TypeRef {
        kind: TypeRefKind::Path(arg),
        ..
    })) = path.segments[0].args.as_ref().map(|a| &a.args[0])
    else {
        panic!("expected a type argument");
    };
    assert_eq!(
        analysis.resolutions.path(arg.id),
        Some(PathRes::full(Res::PrimTy(PrimTy::U8)))
    );
}

#[test]
fn imported_primitives_resolve_to_the_primitive() {
    let analysis = analyze_ok("use u8 as Byte; pub fn f(_: Byte) {}");
    assert_eq!(
        param_res(&analysis, "f", 0),
        PathRes::full(Res::PrimTy(PrimTy::U8))
    );
}

#[test]
fn self_and_generic_parameters() {
    let analysis = analyze_ok(
        "pub struct S;
         pub trait Tr { type Out; fn t(&self, _: Self::Out); }
         impl S { pub fn m<T>(_: Self, _: T) {} }",
    );
    let impl_id = script_impl(&analysis);
    assert_eq!(
        param_res(&analysis, "m", 0),
        PathRes::full(Res::SelfTy(SelfTyOf::Impl(impl_id)))
    );
    let (fn_id, _) = analysis
        .hir
        .functions
        .iter()
        .find(|(_, f)| f.name.as_str() == "m")
        .expect("m exists");
    let param = GenericParamId {
        owner: GenericOwner::Fn(fn_id),
        index: 0,
    };
    assert_eq!(
        param_res(&analysis, "m", 1),
        PathRes::full(Res::GenericParam(param))
    );

    // `Self::Out` in a trait leaves `Out` for type checking.
    let (trait_id, _) = analysis
        .hir
        .traits
        .iter()
        .find(|(_, t)| t.name.as_str() == "Tr")
        .expect("Tr exists");
    let res = param_res(&analysis, "t", 0);
    assert_eq!(res.base, Res::SelfTy(SelfTyOf::Trait(trait_id)));
    assert_eq!(res.unresolved_segments, 1);
}

#[test]
fn lifetimes_resolve_to_their_declarations() {
    let analysis = analyze_ok(
        "pub struct H<'a> { pub r: &'a u8 }
         impl<'a> H<'a> { pub fn g<'b>(_: &'b u8, _: &'a u8, _: &'static u8, _: &'_ u8) {} }",
    );
    let f = function(&analysis, "g");
    let lifetime = |index: usize| match &f.params[index].ty.kind {
        TypeRefKind::Reference {
            lifetime: Some(lt), ..
        } => analysis.resolutions.lifetime(lt.id),
        other => panic!("expected a reference with a lifetime, found {other:?}"),
    };
    let (fn_id, _) = analysis
        .hir
        .functions
        .iter()
        .find(|(_, f)| f.name.as_str() == "g")
        .expect("g exists");
    let impl_id = script_impl(&analysis);
    let fn_param = GenericParamId {
        owner: GenericOwner::Fn(fn_id),
        index: 0,
    };
    let impl_param = GenericParamId {
        owner: GenericOwner::Impl(impl_id),
        index: 0,
    };
    assert_eq!(lifetime(0), Some(LifetimeRes::Param(fn_param)));
    assert_eq!(lifetime(1), Some(LifetimeRes::Param(impl_param)));
    assert_eq!(lifetime(2), Some(LifetimeRes::Static));
    assert_eq!(lifetime(3), Some(LifetimeRes::Anonymous));
}

#[test]
fn glob_reexports_take_the_narrower_visibility() {
    let analysis = analyze_ok("mod a { pub(crate) struct X; } pub use a::*;");
    let root = analysis
        .resolutions
        .crate_roots
        .values()
        .copied()
        .last()
        .expect("a script root");
    let bindings = analysis.resolutions.scopes[root]
        .get(bluing_hir::resolve::Namespace::Types, &Name::new("X"))
        .expect("X is glob-imported");
    let binding = bindings.globs[0];
    assert_eq!(binding.def, ScopeDef::Def(struct_def(&analysis, "X", None)));
    assert_eq!(binding.vis, Visibility::Restricted(root));
}

#[test]
fn the_standard_library_is_clean_and_registers_lang_items() {
    let analysis = analyze_ok("");
    let names: Vec<_> = analysis.hir.lang_items.keys().map(Name::as_str).collect();
    assert_eq!(names, ["None", "Option", "Some", "clone", "copy", "sized"]);
}
