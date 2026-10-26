//! Lowering of expressions.

use bluing_base::{Code, TextRange};
use bluing_syntax::ra_ap_syntax::{
    AstNode,
    ast::{self, HasArgList, HasGenericArgs, HasLoopBody},
};

use super::control::TargetKind;
use super::literal::{self, LiteralError};
use super::lower::{Lowered, Lowerer, label_name};
use super::{ArithOp, BinaryOp, CmpOp, Expr, FieldInit, LogicOp, PathExpr, UnaryOp};
use crate::Name;
use crate::hir::PathKind;

impl Lowerer<'_, '_> {
    pub(super) fn expr_kind(&mut self, expr: &ast::Expr) -> Lowered {
        let range = expr.syntax().text_range();
        let expr = match expr {
            ast::Expr::Literal(lit) => match literal::lower(lit) {
                Ok(lit) => Expr::Literal(lit),
                Err(LiteralError::Reported) => Expr::Missing,
                Err(LiteralError::Error(code, message)) => {
                    self.error(range, code, message);
                    Expr::Missing
                }
            },
            ast::Expr::PathExpr(e) => match e.path() {
                Some(path) => self.path_expr(&path),
                None => Expr::Missing,
            },
            ast::Expr::ParenExpr(e) => match e.expr() {
                Some(inner) => return self.expr_kind(&inner),
                None => Expr::Missing,
            },
            ast::Expr::BlockExpr(block) => return self.block_kind(block),
            ast::Expr::IfExpr(e) => return self.if_expr(e),
            ast::Expr::LoopExpr(e) => {
                let label = label_name(e.label());
                let (target, body) = self.with_target(label, TargetKind::Loop, |this| {
                    this.block_opt(e.loop_body())
                });
                return self.finish_target(target, Expr::Loop { body, target }, range);
            }
            ast::Expr::WhileExpr(e) => {
                if matches!(e.condition(), Some(ast::Expr::LetExpr(_))) {
                    return self.not_yet(range, "`while let` loops");
                }
                let condition = self.expr(e.condition());
                let label = label_name(e.label());
                let (target, body) = self.with_target(label, TargetKind::While, |this| {
                    this.block_opt(e.loop_body())
                });
                let lowered = Expr::While {
                    condition,
                    body,
                    target,
                };
                return self.finish_target(target, lowered, range);
            }
            ast::Expr::BreakExpr(e) => self.break_expr(e, range),
            ast::Expr::ContinueExpr(e) => self.continue_expr(e, range),
            ast::Expr::ReturnExpr(e) => {
                if !self.in_fn {
                    self.error(range, Code::Control, "`return` outside of a function body");
                    return Lowered::New(Expr::Missing);
                }
                Expr::Return(e.expr().map(|value| self.expr(Some(value))))
            }
            ast::Expr::CallExpr(e) => {
                let callee = self.expr(e.expr());
                let args = self.exprs(e.arg_list().into_iter().flat_map(|list| list.args()));
                Expr::Call { callee, args }
            }
            ast::Expr::MethodCallExpr(e) => {
                let Some(name) = e.name_ref() else {
                    return Lowered::New(Expr::Missing);
                };
                let receiver = self.expr(e.receiver());
                let generic_args = e
                    .generic_arg_list()
                    .map(|list| self.types().generic_args(&list));
                let args = self.exprs(e.arg_list().into_iter().flat_map(|list| list.args()));
                Expr::MethodCall {
                    receiver,
                    method: Name::new(name.text()),
                    method_range: name.syntax().text_range(),
                    generic_args,
                    args,
                }
            }
            ast::Expr::FieldExpr(e) => {
                let Some(name) = e.name_ref() else {
                    return Lowered::New(Expr::Missing);
                };
                let base = self.expr(e.expr());
                Expr::Field {
                    base,
                    name: Name::new(name.text()),
                    name_range: name.syntax().text_range(),
                }
            }
            ast::Expr::RecordExpr(e) => return self.record_expr(e, range),
            ast::Expr::TupleExpr(e) => Expr::Tuple(self.exprs(e.fields())),
            ast::Expr::ArrayExpr(e) => match e.kind() {
                ast::ArrayExprKind::ElementList(elements) => Expr::Array(self.exprs(elements)),
                ast::ArrayExprKind::Repeat {
                    initializer,
                    repeat,
                } => {
                    let value = self.expr(initializer);
                    let count = self.expr(repeat);
                    Expr::Repeat { value, count }
                }
            },
            ast::Expr::IndexExpr(e) => {
                let base = self.expr(e.base());
                let index = self.expr(e.index());
                Expr::Index { base, index }
            }
            ast::Expr::RefExpr(e) => {
                if e.raw_token().is_some() {
                    return self.not_yet(range, "raw borrows (`&raw const`, `&raw mut`)");
                }
                let mutable = e.mut_token().is_some();
                Expr::Ref {
                    mutable,
                    expr: self.expr(e.expr()),
                }
            }
            ast::Expr::PrefixExpr(e) => {
                let Some(op) = e.op_kind() else {
                    return Lowered::New(Expr::Missing);
                };
                let op = match op {
                    ast::UnaryOp::Deref => UnaryOp::Deref,
                    ast::UnaryOp::Not => UnaryOp::Not,
                    ast::UnaryOp::Neg => UnaryOp::Neg,
                };
                Expr::Unary {
                    op,
                    expr: self.expr(e.expr()),
                }
            }
            ast::Expr::BinExpr(e) => self.bin_expr(e),
            ast::Expr::ForExpr(_) => return self.not_yet(range, "`for` loops"),
            ast::Expr::MatchExpr(_) => return self.not_yet(range, "`match` expressions"),
            ast::Expr::LetExpr(_) => return self.not_yet(range, "`let` expressions"),
            ast::Expr::ClosureExpr(_) => return self.not_yet(range, "closures"),
            ast::Expr::AwaitExpr(_) => return self.not_yet(range, "`.await` expressions"),
            ast::Expr::RangeExpr(_) => return self.not_yet(range, "range expressions"),
            ast::Expr::CastExpr(_) => return self.not_yet(range, "`as` casts"),
            ast::Expr::TryExpr(_) => return self.not_yet(range, "`?` expressions"),
            ast::Expr::MacroExpr(_) => return self.not_yet(range, "macro calls in expressions"),
            ast::Expr::UnderscoreExpr(_) => {
                return self.not_yet(range, "destructuring assignments");
            }
            ast::Expr::YieldExpr(_) | ast::Expr::YeetExpr(_) | ast::Expr::BecomeExpr(_) => {
                return self.unsupported(range, "`yield`, `do yeet` and `become` expressions");
            }
            ast::Expr::AsmExpr(_)
            | ast::Expr::FormatArgsExpr(_)
            | ast::Expr::OffsetOfExpr(_)
            | ast::Expr::IncludeBytesExpr(_)
            | ast::Expr::CfgPredExpr(_) => {
                return self.unsupported(range, "`builtin #` expressions");
            }
        };
        Lowered::New(expr)
    }

    pub(super) fn path_expr(&mut self, path: &ast::Path) -> Expr {
        let range = path.syntax().text_range();
        // `self` is the receiver of a method.
        if path.qualifier().is_none()
            && path
                .segment()
                .and_then(|segment| segment.kind())
                .is_some_and(|kind| matches!(kind, ast::PathSegmentKind::SelfKw))
        {
            return match self.local(&Name::new("self")) {
                Some(local) => Expr::Path(PathExpr::Local(local)),
                None => {
                    let message = "`self` is only available in methods with a `self` parameter";
                    self.error(range, Code::Unresolved, message);
                    Expr::Missing
                }
            };
        }
        let Some(lowered) = self.types().path(path) else {
            return Expr::Missing;
        };
        if let [segment] = lowered.segments.as_slice()
            && lowered.kind == PathKind::Plain
            && lowered.qself.is_none()
            && segment.args.is_none()
            && let Some(local) = self.local(&segment.name)
        {
            return Expr::Path(PathExpr::Local(local));
        }
        Expr::Path(PathExpr::Item(lowered))
    }

    pub(super) fn if_expr(&mut self, e: &ast::IfExpr) -> Lowered {
        let range = e.syntax().text_range();
        if matches!(e.condition(), Some(ast::Expr::LetExpr(_))) {
            return self.not_yet(range, "`if let` expressions");
        }
        let condition = self.expr(e.condition());
        let then_branch = self.block_opt(e.then_branch());
        let else_branch = e.else_branch().map(|branch| match branch {
            ast::ElseBranch::Block(block) => self.block_expr(&block),
            ast::ElseBranch::IfExpr(nested) => {
                let range = nested.syntax().text_range();
                let lowered = self.if_expr(&nested);
                self.finish(lowered, range)
            }
        });
        Lowered::New(Expr::If {
            condition,
            then_branch,
            else_branch,
        })
    }

    pub(super) fn record_expr(&mut self, e: &ast::RecordExpr, range: TextRange) -> Lowered {
        let Some(path) = e.path().and_then(|path| self.types().path(&path)) else {
            return Lowered::New(Expr::Missing);
        };
        let list = e.record_expr_field_list();
        let mut fields = Vec::new();
        for field in list.iter().flat_map(|list| list.fields()) {
            let Some(name) = field.field_name() else {
                continue;
            };
            let field_range = field.syntax().text_range();
            let value = match field.expr() {
                Some(value) => self.expr(Some(value)),
                // `S { x }` is `S { x: x }`.
                None => {
                    let name = Name::new(name.text());
                    let expr = match self.local(&name) {
                        Some(local) => Expr::Path(PathExpr::Local(local)),
                        None => {
                            Expr::Path(PathExpr::Item(self.single_name_path(name, field_range)))
                        }
                    };
                    self.alloc(expr, field_range)
                }
            };
            fields.push(FieldInit {
                name: Name::new(name.text()),
                range: field_range,
                value,
            });
        }
        let base = match list.as_ref() {
            Some(list) if list.dotdot_token().is_some() => match list.spread() {
                Some(base) => Some(self.expr(Some(base))),
                None => {
                    return self.unsupported(range, "struct literals with default field values");
                }
            },
            _ => None,
        };
        Lowered::New(Expr::Struct { path, fields, base })
    }

    pub(super) fn bin_expr(&mut self, e: &ast::BinExpr) -> Expr {
        let Some(op) = e.op_kind() else {
            return Expr::Missing;
        };
        let lhs = self.expr(e.lhs());
        let rhs = self.expr(e.rhs());
        let op = match op {
            ast::BinaryOp::Assignment { op: None } => return Expr::Assign { lhs, rhs },
            ast::BinaryOp::Assignment { op: Some(op) } => {
                return Expr::CompoundAssign {
                    op: arith_op(op),
                    lhs,
                    rhs,
                };
            }
            ast::BinaryOp::ArithOp(op) => BinaryOp::Arith(arith_op(op)),
            ast::BinaryOp::LogicOp(ast::LogicOp::And) => BinaryOp::Logic(LogicOp::And),
            ast::BinaryOp::LogicOp(ast::LogicOp::Or) => BinaryOp::Logic(LogicOp::Or),
            ast::BinaryOp::CmpOp(op) => BinaryOp::Cmp(match op {
                ast::CmpOp::Eq { negated: false } => CmpOp::Eq,
                ast::CmpOp::Eq { negated: true } => CmpOp::Ne,
                ast::CmpOp::Ord {
                    ordering: ast::Ordering::Less,
                    strict: true,
                } => CmpOp::Lt,
                ast::CmpOp::Ord {
                    ordering: ast::Ordering::Less,
                    strict: false,
                } => CmpOp::Le,
                ast::CmpOp::Ord {
                    ordering: ast::Ordering::Greater,
                    strict: true,
                } => CmpOp::Gt,
                ast::CmpOp::Ord {
                    ordering: ast::Ordering::Greater,
                    strict: false,
                } => CmpOp::Ge,
            }),
        };
        Expr::Binary { op, lhs, rhs }
    }
}

fn arith_op(op: ast::ArithOp) -> ArithOp {
    match op {
        ast::ArithOp::Add => ArithOp::Add,
        ast::ArithOp::Sub => ArithOp::Sub,
        ast::ArithOp::Mul => ArithOp::Mul,
        ast::ArithOp::Div => ArithOp::Div,
        ast::ArithOp::Rem => ArithOp::Rem,
        ast::ArithOp::BitAnd => ArithOp::BitAnd,
        ast::ArithOp::BitOr => ArithOp::BitOr,
        ast::ArithOp::BitXor => ArithOp::BitXor,
        ast::ArithOp::Shl => ArithOp::Shl,
        ast::ArithOp::Shr => ArithOp::Shr,
    }
}
