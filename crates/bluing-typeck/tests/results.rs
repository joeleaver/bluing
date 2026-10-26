//! Checks what type checking records about a body.

use std::collections::BTreeMap;

use bluing_hir::body::{BodyOwner, Expr, ExprId, Stmt};
use bluing_hir::{
    Analysis, CrateData, CrateGraph, CrateKind, MemoryProvider, Name, VfsPath, analyze,
};
use bluing_ty::TyCx;
use bluing_ty::ty::{Mutability, Ty};
use bluing_typeck::{Adjustment, TypeckResults, typeck};

struct Checked {
    analysis: Analysis,
    graph: CrateGraph,
}

fn analyze_script(text: &str) -> Checked {
    let mut provider = MemoryProvider::new();
    for (path, text) in bluing_std::files() {
        provider.insert(VfsPath::new(format!("{}/{path}", bluing_std::DIR)), *text);
    }
    provider.insert(VfsPath::new("/script/main.blu"), text);
    let mut graph = CrateGraph::new();
    let std = graph.add(CrateData {
        name: Name::new("std"),
        root: VfsPath::new(bluing_std::ROOT),
        kind: CrateKind::Library,
        deps: Vec::new(),
    });
    graph.add(CrateData {
        name: Name::new("script"),
        root: VfsPath::new("/script/main.blu"),
        kind: CrateKind::Script {
            allow_unsafe: false,
        },
        deps: vec![std],
    });
    let analysis = analyze(&graph, &provider).expect("roots exist");
    assert!(
        analysis.diagnostics.is_empty(),
        "{:?}",
        analysis.diagnostics
    );
    Checked { analysis, graph }
}

/// Type checks `text` and calls `f` with the results of function `name`.
fn with_results(
    text: &str,
    name: &str,
    f: impl FnOnce(&mut TyCx<'_>, &bluing_hir::body::Body, &TypeckResults),
) {
    let checked = analyze_script(text);
    let mut cx = bluing_ty::collect(&checked.analysis, &checked.graph);
    let results: BTreeMap<BodyOwner, TypeckResults> = typeck(&mut cx, &checked.analysis.bodies);
    assert!(cx.diagnostics.is_empty(), "{:?}", cx.diagnostics);
    let (id, _) = checked
        .analysis
        .hir
        .functions
        .iter()
        .find(|(_, function)| function.name.as_str() == name)
        .expect("the function exists");
    let owner = BodyOwner::Fn(id);
    f(&mut cx, &checked.analysis.bodies[&owner], &results[&owner]);
}

fn let_init(body: &bluing_hir::body::Body, index: usize) -> ExprId {
    let Expr::Block(block) = &body.exprs[body.root] else {
        panic!("a function body is a block");
    };
    match &block.stmts[index] {
        Stmt::Let {
            init: Some(init), ..
        } => *init,
        other => panic!("expected `let` with an initializer, found {other:?}"),
    }
}

fn show(cx: &TyCx<'_>, ty: Ty) -> String {
    cx.display(ty, None)
}

#[test]
fn literals_take_their_type_from_use_or_fall_back() {
    let text = "pub fn f() { let a = 1; let b: u8 = 2 + a; let c = 3; let d = 1.5; }";
    with_results(text, "f", |cx, body, results| {
        let a = results.expr_tys[let_init(body, 0)];
        let c = results.expr_tys[let_init(body, 2)];
        let d = results.expr_tys[let_init(body, 3)];
        assert_eq!(show(cx, a), "u8");
        assert_eq!(show(cx, c), "i32");
        assert_eq!(show(cx, d), "f64");
    });
}

#[test]
fn method_calls_record_the_method_and_receiver_adjustment() {
    let text = "
        pub struct P { pub x: i32 }
        impl P { pub fn get(&self) -> i32 { self.x } pub fn bump(&mut self) { self.x += 1; } }
        pub fn f(r: &P) -> i32 { let mut p = P { x: 1 }; p.bump(); let v = r.get(); v }
    ";
    with_results(text, "f", |cx, body, results| {
        let (get, _) = cx
            .hir
            .functions
            .iter()
            .find(|(_, f)| f.name.as_str() == "get")
            .unwrap();
        let call = let_init(body, 2);
        assert_eq!(results.method_calls[call], get);
        let Expr::MethodCall { receiver, .. } = &body.exprs[call] else {
            panic!("a method call");
        };
        // `r: &P` already has the receiver type `&P`: no adjustment.
        let adjustment = results
            .adjustments
            .get(*receiver)
            .copied()
            .unwrap_or_default();
        assert_eq!(adjustment, Adjustment::default());
        // `p.bump()` borrows `p` mutably.
        let Expr::Block(block) = &body.exprs[body.root] else {
            unreachable!()
        };
        let Stmt::Expr { expr: bump, .. } = &block.stmts[1] else {
            panic!("a call statement")
        };
        let Expr::MethodCall { receiver, .. } = &body.exprs[*bump] else {
            panic!("a method call")
        };
        assert_eq!(
            results.adjustments[*receiver].autoref,
            Some(Mutability::Mut)
        );
    });
}

#[test]
fn field_access_through_references_records_derefs() {
    let text = "pub struct P { pub x: i32 } pub fn f(r: &&P) -> i32 { let v = r.x; v }";
    with_results(text, "f", |cx, body, results| {
        let access = let_init(body, 0);
        assert_eq!(show(cx, results.expr_tys[access]), "i32");
        assert_eq!(results.field_indices[access], 0);
        let Expr::Field { base, .. } = &body.exprs[access] else {
            panic!("a field access")
        };
        assert_eq!(results.adjustments[*base].derefs, 2);
    });
}

#[test]
fn diverging_branches_coerce_to_the_other_branch() {
    let text = "pub fn f(c: bool) -> u8 { let x = if c { return 1 } else { 2u8 }; x }";
    with_results(text, "f", |cx, body, results| {
        let if_expr = let_init(body, 0);
        assert_eq!(show(cx, results.expr_tys[if_expr]), "u8");
        let Expr::If { then_branch, .. } = &body.exprs[if_expr] else {
            panic!("an if")
        };
        assert!(results.adjustments[*then_branch].never_to_any);
    });
}
