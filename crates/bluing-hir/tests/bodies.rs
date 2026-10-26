//! Checks what names inside bodies resolve to.

mod support;

use bluing_hir::body::{Body, BreakTarget, Expr, ExprId, Pat, PathExpr, Stmt};
use bluing_hir::resolve::{PathRes, Res};
use bluing_hir::{Analysis, BodyOwner, DefId};
use support::{analyze_files, render};

fn analyze_ok(text: &str) -> Analysis {
    let analysis = analyze_files(&[("main.blu", text)]);
    assert!(analysis.diagnostics.is_empty(), "{}", render(&analysis));
    analysis
}

fn body<'a>(analysis: &'a Analysis, fn_name: &str) -> &'a Body {
    let (id, _) = analysis
        .hir
        .functions
        .iter()
        .find(|(_, f)| f.name.as_str() == fn_name)
        .unwrap_or_else(|| panic!("no function `{fn_name}`"));
    &analysis.bodies[&BodyOwner::Fn(id)]
}

/// The statements of the body's outer block.
fn stmts(body: &Body) -> &[Stmt] {
    match &body.exprs[body.root] {
        Expr::Block(block) => &block.stmts,
        other => panic!("expected a block, found {other:?}"),
    }
}

fn let_init(body: &Body, index: usize) -> ExprId {
    match &stmts(body)[index] {
        Stmt::Let {
            init: Some(init), ..
        } => *init,
        other => panic!("expected `let` with an initializer, found {other:?}"),
    }
}

fn let_local(body: &Body, index: usize) -> bluing_hir::body::LocalId {
    match &stmts(body)[index] {
        Stmt::Let { pat, .. } => match body.pats[*pat] {
            Pat::Bind { local } => local,
            ref other => panic!("expected a binding, found {other:?}"),
        },
        other => panic!("expected `let`, found {other:?}"),
    }
}

#[test]
fn a_later_let_shadows_an_earlier_one_but_not_in_its_own_initializer() {
    let analysis = analyze_ok("pub fn f() { let x = 1; let x = x; let y = x; }");
    let body = body(&analysis, "f");
    let first = let_local(body, 0);
    let second = let_local(body, 1);
    assert!(matches!(body.exprs[let_init(body, 1)], Expr::Path(PathExpr::Local(l)) if l == first));
    assert!(matches!(body.exprs[let_init(body, 2)], Expr::Path(PathExpr::Local(l)) if l == second));
}

#[test]
fn labeled_break_targets_the_named_loop() {
    let analysis = analyze_ok("pub fn f() { 'outer: loop { loop { break 'outer; } } }");
    let body = body(&analysis, "f");
    let outer = body
        .exprs
        .iter()
        .find_map(|(id, e)| {
            matches!(e, Expr::Loop { target, .. } if *target == BreakTarget(0)).then_some(id)
        })
        .expect("the outer loop is target 0");
    assert_eq!(body.break_targets[0], outer);
    let break_target = body
        .exprs
        .iter()
        .find_map(|(_, e)| match e {
            Expr::Break { target, .. } => Some(*target),
            _ => None,
        })
        .expect("a break");
    assert_eq!(break_target, BreakTarget(0));
}

#[test]
fn item_and_struct_paths_resolve() {
    let analysis = analyze_ok(
        "pub struct P { pub x: i32 }
         fn helper() -> i32 { 1 }
         pub fn f() { let p = P { x: helper() }; }",
    );
    let body = body(&analysis, "f");
    let (helper, _) = analysis
        .hir
        .functions
        .iter()
        .find(|(_, f)| f.name.as_str() == "helper")
        .unwrap();
    let (p, _) = analysis
        .hir
        .structs
        .iter()
        .find(|(_, s)| s.name.as_str() == "P")
        .unwrap();
    let Expr::Struct { path, fields, .. } = &body.exprs[let_init(body, 0)] else {
        panic!("expected a struct literal");
    };
    assert_eq!(
        body.path_res(path),
        Some(PathRes::full(Res::Def(DefId::Struct(p))))
    );
    let Expr::Call { callee, .. } = &body.exprs[fields[0].value] else {
        panic!("expected a call");
    };
    let Expr::Path(PathExpr::Item(callee)) = &body.exprs[*callee] else {
        panic!("expected a path");
    };
    assert_eq!(
        body.path_res(callee),
        Some(PathRes::full(Res::Def(DefId::Fn(helper))))
    );
}
