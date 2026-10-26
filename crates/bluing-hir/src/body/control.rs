//! Lowering of blocks, statements and control flow: loops, labels,
//! `break` and `continue`, with their validity checks.

use bluing_base::{Code, TextRange};
use bluing_syntax::ra_ap_syntax::{AstNode, ast};

use super::lower::{Lowered, Lowerer, label_name};
use super::{Block, BreakTarget, Expr, ExprId, Stmt};
use crate::Name;

/// What a break target is, for checking `break` and `continue`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TargetKind {
    Loop,
    While,
    Block,
}

pub(super) struct TargetScope {
    pub(super) label: Option<Name>,
    pub(super) target: BreakTarget,
    pub(super) kind: TargetKind,
}

impl Lowerer<'_, '_> {
    pub(super) fn block_opt(&mut self, block: Option<ast::BlockExpr>) -> ExprId {
        match block {
            Some(block) => self.block_expr(&block),
            None => self.alloc(Expr::Missing, TextRange::default()),
        }
    }

    pub fn block_expr(&mut self, block: &ast::BlockExpr) -> ExprId {
        let range = block.syntax().text_range();
        let lowered = self.block_kind(block);
        self.finish(lowered, range)
    }

    pub(super) fn block_kind(&mut self, block: &ast::BlockExpr) -> Lowered {
        let range = block.syntax().text_range();
        let is_unsafe = match block.modifier() {
            None | Some(ast::BlockModifier::Label(_)) => false,
            Some(ast::BlockModifier::Unsafe(_)) => true,
            Some(ast::BlockModifier::Async(_)) => return self.not_yet(range, "`async` blocks"),
            Some(ast::BlockModifier::Const(_)) => return self.not_yet(range, "`const` blocks"),
            Some(ast::BlockModifier::Try { .. })
            | Some(ast::BlockModifier::Gen(_))
            | Some(ast::BlockModifier::AsyncGen(_)) => {
                return self.unsupported(range, "`try` and `gen` blocks");
            }
        };
        match label_name(block.label()) {
            Some(label) => {
                let (target, mut lowered) =
                    self.with_target(Some(label), TargetKind::Block, |this| {
                        this.block_contents(block, is_unsafe)
                    });
                lowered.target = Some(target);
                self.finish_target(target, Expr::Block(lowered), range)
            }
            None => Lowered::New(Expr::Block(self.block_contents(block, is_unsafe))),
        }
    }

    pub(super) fn block_contents(&mut self, block: &ast::BlockExpr, is_unsafe: bool) -> Block {
        self.local_scopes.push(Vec::new());
        let mut stmts = Vec::new();
        let list = block.stmt_list();
        for stmt in list.iter().flat_map(|list| list.statements()) {
            match stmt {
                ast::Stmt::LetStmt(stmt) => {
                    if let Some(lowered) = self.let_stmt(&stmt) {
                        stmts.push(lowered);
                    }
                }
                ast::Stmt::ExprStmt(stmt) => {
                    let has_semi = stmt.semicolon_token().is_some();
                    let expr = self.expr(stmt.expr());
                    stmts.push(Stmt::Expr { expr, has_semi });
                }
                // Items in bodies were reported when the item was lowered.
                ast::Stmt::Item(_) => {}
            }
        }
        let tail = list
            .and_then(|list| list.tail_expr())
            .map(|tail| self.expr(Some(tail)));
        self.local_scopes.pop();
        Block {
            stmts,
            tail,
            target: None,
            is_unsafe,
        }
    }

    pub(super) fn let_stmt(&mut self, stmt: &ast::LetStmt) -> Option<Stmt> {
        let range = stmt.syntax().text_range();
        if stmt.super_token().is_some() {
            self.unsupported(range, "`super let` statements");
            return None;
        }
        if stmt.let_else().is_some() {
            self.not_yet(range, "`let ... else` statements");
            return None;
        }
        let ty = stmt.ty().map(|ty| self.types().ty(&ty));
        // The initializer cannot see the bindings the pattern introduces.
        let init = stmt.initializer().map(|init| self.expr(Some(init)));
        let mut bound = Vec::new();
        let pat = self.pat(stmt.pat().as_ref(), &mut bound);
        for (name, local) in bound {
            self.bind(name, local);
        }
        Some(Stmt::Let { pat, ty, init })
    }

    /// Runs `f` with a new break target in scope. The target's expression is
    /// reserved now and filled in by [`Self::finish_target`].
    pub(super) fn with_target<T>(
        &mut self,
        label: Option<Name>,
        kind: TargetKind,
        f: impl FnOnce(&mut Self) -> T,
    ) -> (BreakTarget, T) {
        let target = BreakTarget(self.break_targets.len() as u32);
        let reserved = self.alloc(Expr::Missing, TextRange::default());
        self.break_targets.push(reserved);
        self.targets.push(TargetScope {
            label,
            target,
            kind,
        });
        let result = f(self);
        self.targets.pop();
        (target, result)
    }

    pub(super) fn finish_target(
        &mut self,
        target: BreakTarget,
        expr: Expr,
        range: TextRange,
    ) -> Lowered {
        let id = self.break_targets[target.0 as usize];
        self.exprs[id] = expr;
        self.expr_ranges.insert(id, range);
        Lowered::Allocated(id)
    }

    /// The target of a `break` or `continue`, reporting errors.
    pub(super) fn find_target(
        &mut self,
        label: Option<ast::Lifetime>,
        range: TextRange,
        is_continue: bool,
    ) -> Option<(BreakTarget, TargetKind)> {
        let keyword = if is_continue { "continue" } else { "break" };
        if let Some(label) = label {
            let text = label.text();
            let name = Name::new(text.strip_prefix('\'').unwrap_or(text));
            let Some(index) = self
                .targets
                .iter()
                .rposition(|t| t.label.as_ref() == Some(&name))
            else {
                let message = format!("use of undeclared label `{text}`");
                self.error(label.syntax().text_range(), Code::Unresolved, message);
                return None;
            };
            let found = &self.targets[index];
            if is_continue && found.kind == TargetKind::Block {
                let message = format!("`continue` pointing to a labeled block `{text}`");
                self.error(range, Code::Control, message);
                return None;
            }
            return Some((found.target, found.kind));
        }
        let loop_index = self
            .targets
            .iter()
            .rposition(|t| t.kind != TargetKind::Block);
        // A labeled block between here and the loop (or with no loop at all)
        // makes an unlabeled `break` or `continue` an error.
        if loop_index.map_or(!self.targets.is_empty(), |index| {
            index + 1 < self.targets.len()
        }) {
            let message = format!("unlabeled `{keyword}` inside of a labeled block");
            self.error(range, Code::Control, message);
            return None;
        }
        let Some(index) = loop_index else {
            let message = format!("`{keyword}` outside of a loop");
            self.error(range, Code::Control, message);
            return None;
        };
        let found = &self.targets[index];
        Some((found.target, found.kind))
    }

    pub(super) fn break_expr(&mut self, e: &ast::BreakExpr, range: TextRange) -> Expr {
        let found = self.find_target(e.lifetime(), range, false);
        let value = e.expr().map(|value| self.expr(Some(value)));
        let Some((target, kind)) = found else {
            return Expr::Missing;
        };
        if kind == TargetKind::While && value.is_some() {
            self.error(
                range,
                Code::Control,
                "`break` with a value from a `while` loop",
            );
            return Expr::Missing;
        }
        Expr::Break { target, value }
    }

    pub(super) fn continue_expr(&mut self, e: &ast::ContinueExpr, range: TextRange) -> Expr {
        match self.find_target(e.lifetime(), range, true) {
            Some((target, _)) => Expr::Continue { target },
            None => Expr::Missing,
        }
    }
}
