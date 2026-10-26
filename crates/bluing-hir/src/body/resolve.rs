//! Resolves the item paths and types written inside a body, once lowering
//! has resolved its local variables.

use bluing_base::Diagnostic;

use super::lower::owner_scope;
use super::{Body, Expr, Pat, PathExpr, Stmt};
use crate::resolve::{NameEnv, TypeResolver};
use crate::{CrateGraph, Hir, Resolutions};

pub(super) fn resolve_paths(
    hir: &Hir,
    graph: &CrateGraph,
    resolutions: &Resolutions,
    body: &mut Body,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let env = NameEnv::finished(hir, graph, resolutions);
    let scope = owner_scope(hir, body.owner, body.file);
    let mut resolver = TypeResolver {
        env: &env,
        diagnostics,
        paths: &mut body.paths,
        lifetimes: &mut body.lifetimes,
    };
    for (_, expr) in body.exprs.iter() {
        match expr {
            Expr::Path(PathExpr::Item(path)) => resolver.resolve_value_path(&scope, path),
            Expr::Struct { path, .. } => resolver.resolve_struct_path(&scope, path),
            Expr::MethodCall {
                generic_args: Some(args),
                ..
            } => resolver.resolve_generic_args(&scope, args),
            Expr::Block(block) => {
                for stmt in &block.stmts {
                    if let Stmt::Let { ty: Some(ty), .. } = stmt {
                        resolver.resolve_type(&scope, ty);
                    }
                }
            }
            _ => {}
        }
    }
    for (_, pat) in body.pats.iter() {
        if let Pat::Struct { path, .. } = pat {
            resolver.resolve_struct_path(&scope, path);
        }
    }
}
