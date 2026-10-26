//! Lowering of expressions and statements, resolving local variables and
//! loop labels as it goes.

use bluing_base::{Code, Diagnostic, FileId, TextRange};
use bluing_syntax::ra_ap_syntax::{AstNode, SyntaxNode, ast};
use la_arena::{Arena, ArenaMap};

use super::control::TargetScope;
use super::{Body, BodyOwner, Expr, ExprId, LocalId};
use crate::hir::PathKind;
use crate::lower::TypeLowerer;
use crate::resolve::{LexScope, NameEnv};
use crate::{CrateGraph, Files, Hir, Name, Resolutions};

/// The syntax of a body to lower.
pub(super) enum BodySource {
    /// A function's block and its parameter patterns.
    Fn {
        block: ast::BlockExpr,
        params: Vec<Option<ast::Pat>>,
        self_param: Option<(bool, TextRange)>,
    },
    /// A constant or static initializer.
    Init(ast::Expr),
}

/// Every body in the compilation, with its syntax.
pub(super) fn body_sources(hir: &Hir, files: &Files) -> Vec<(BodyOwner, FileId, BodySource)> {
    let root = |file: FileId| -> SyntaxNode { files.get(file).tree.syntax().clone() };
    let mut sources = Vec::new();
    for (id, f) in hir.functions.iter() {
        let Some(body) = &f.body else { continue };
        let tree = root(body.file);
        let params = f
            .params
            .iter()
            .map(|param| param.pat.as_ref().map(|pat| pat.value.to_node(&tree)))
            .collect();
        let self_param = f.self_param.as_ref().map(|param| {
            let mutable = matches!(
                param.kind,
                crate::hir::SelfParamKind::Value { mutable: true }
                    | crate::hir::SelfParamKind::Typed { mutable: true, .. }
            );
            (mutable, param.range)
        });
        let block = body.value.to_node(&tree);
        sources.push((
            BodyOwner::Fn(id),
            body.file,
            BodySource::Fn {
                block,
                params,
                self_param,
            },
        ));
    }
    for (id, c) in hir.consts.iter() {
        if let Some(body) = &c.body {
            let expr = body.value.to_node(&root(body.file));
            sources.push((BodyOwner::Const(id), body.file, BodySource::Init(expr)));
        }
    }
    for (id, s) in hir.statics.iter() {
        if let Some(body) = &s.body {
            let expr = body.value.to_node(&root(body.file));
            sources.push((BodyOwner::Static(id), body.file, BodySource::Init(expr)));
        }
    }
    sources
}

/// The lexical scope a body's paths are written in.
pub(super) fn owner_scope<'h>(hir: &'h Hir, owner: BodyOwner, file: FileId) -> LexScope<'h> {
    use crate::resolve::{container_scope, scope};
    match owner {
        BodyOwner::Fn(id) => {
            let f = &hir.functions[id];
            let mut lex = container_scope(hir, f.container, file);
            lex.generics
                .push((crate::GenericOwner::Fn(id), &f.generics));
            lex
        }
        BodyOwner::Const(id) => container_scope(hir, hir.consts[id].container, file),
        BodyOwner::Static(id) => scope(hir.statics[id].module, file, Vec::new(), None),
    }
}

pub(super) fn lower(
    hir: &Hir,
    graph: &CrateGraph,
    resolutions: &Resolutions,
    owner: BodyOwner,
    file: FileId,
    source: &BodySource,
    diagnostics: &mut Vec<Diagnostic>,
) -> Body {
    let env = NameEnv::finished(hir, graph, resolutions);
    let mut lowerer = Lowerer {
        env: &env,
        scope: owner_scope(hir, owner, file),
        file,
        in_fn: matches!(owner, BodyOwner::Fn(_)),
        exprs: Arena::default(),
        expr_ranges: ArenaMap::default(),
        pats: Arena::default(),
        pat_ranges: ArenaMap::default(),
        locals: Arena::default(),
        break_targets: Vec::new(),
        local_scopes: vec![Vec::new()],
        targets: Vec::new(),
        path_count: 0,
        lifetime_count: 0,
        diagnostics,
    };
    let mut params = Vec::new();
    let mut self_local = None;
    let root = match source {
        BodySource::Fn {
            block,
            params: param_pats,
            self_param,
        } => {
            if let Some((mutable, range)) = *self_param {
                let local = lowerer.locals.alloc(super::Local {
                    name: Name::new("self"),
                    mutable,
                    range,
                });
                lowerer.bind(Name::new("self"), local);
                self_local = Some(local);
            }
            // All parameters are in scope for the whole body, and a name may
            // appear only once across them.
            let mut bound = Vec::new();
            for pat in param_pats {
                params.push(lowerer.pat(pat.as_ref(), &mut bound));
            }
            for (name, local) in bound {
                lowerer.bind(name, local);
            }
            lowerer.block_expr(block)
        }
        BodySource::Init(expr) => lowerer.expr(Some(expr.clone())),
    };
    Body {
        owner,
        file,
        exprs: lowerer.exprs,
        expr_ranges: lowerer.expr_ranges,
        pats: lowerer.pats,
        pat_ranges: lowerer.pat_ranges,
        locals: lowerer.locals,
        params,
        self_local,
        break_targets: lowerer.break_targets,
        root,
        paths: vec![None; lowerer.path_count as usize],
        lifetimes: vec![None; lowerer.lifetime_count as usize],
    }
}

/// The result of lowering one expression. Loops and labeled blocks allocate
/// themselves early, so `break` inside them can name their id.
pub(super) enum Lowered {
    New(Expr),
    Allocated(ExprId),
}

pub(super) struct Lowerer<'a, 'd> {
    pub env: &'a NameEnv<'a>,
    pub scope: LexScope<'a>,
    pub file: FileId,
    pub(super) in_fn: bool,
    pub(super) exprs: Arena<Expr>,
    pub(super) expr_ranges: ArenaMap<ExprId, TextRange>,
    pub pats: Arena<super::Pat>,
    pub pat_ranges: ArenaMap<super::PatId, TextRange>,
    pub locals: Arena<super::Local>,
    pub(super) break_targets: Vec<ExprId>,
    /// Local variables in scope, innermost block last. Within a block, a later
    /// `let` shadows an earlier one, so lookups search from the end.
    pub(super) local_scopes: Vec<Vec<(Name, LocalId)>>,
    /// The loops and labeled blocks enclosing the current expression.
    pub(super) targets: Vec<TargetScope>,
    pub(super) path_count: u32,
    pub(super) lifetime_count: u32,
    pub diagnostics: &'d mut Vec<Diagnostic>,
}

impl Lowerer<'_, '_> {
    pub fn error(&mut self, range: TextRange, code: Code, message: impl Into<String>) {
        self.diagnostics
            .push(Diagnostic::new(self.file, range, code, message));
    }

    pub fn types(&mut self) -> TypeLowerer<'_> {
        TypeLowerer {
            file: self.file,
            path_count: &mut self.path_count,
            lifetime_count: &mut self.lifetime_count,
            diagnostics: self.diagnostics,
        }
    }

    pub fn bind(&mut self, name: Name, local: LocalId) {
        self.local_scopes
            .last_mut()
            .expect("there is always a scope")
            .push((name, local));
    }

    pub(super) fn local(&self, name: &Name) -> Option<LocalId> {
        self.local_scopes
            .iter()
            .rev()
            .flat_map(|scope| scope.iter().rev())
            .find(|(n, _)| n == name)
            .map(|&(_, local)| local)
    }

    pub(super) fn alloc(&mut self, expr: Expr, range: TextRange) -> ExprId {
        let id = self.exprs.alloc(expr);
        self.expr_ranges.insert(id, range);
        id
    }

    pub(super) fn not_yet(&mut self, range: TextRange, what: &str) -> Lowered {
        self.error(
            range,
            Code::NotYetSupported,
            format!("{what} are not supported yet"),
        );
        Lowered::New(Expr::Missing)
    }

    pub(super) fn unsupported(&mut self, range: TextRange, what: &str) -> Lowered {
        self.error(
            range,
            Code::Unsupported,
            format!("{what} are not supported in Bluing"),
        );
        Lowered::New(Expr::Missing)
    }

    pub(super) fn finish(&mut self, lowered: Lowered, range: TextRange) -> ExprId {
        match lowered {
            Lowered::New(expr) => self.alloc(expr, range),
            Lowered::Allocated(id) => id,
        }
    }

    pub fn expr(&mut self, expr: Option<ast::Expr>) -> ExprId {
        match expr {
            Some(expr) => {
                let range = expr.syntax().text_range();
                let lowered = self.expr_kind(&expr);
                self.finish(lowered, range)
            }
            // A syntax error the parser already reported.
            None => self.alloc(Expr::Missing, TextRange::default()),
        }
    }

    pub(super) fn exprs(&mut self, exprs: impl Iterator<Item = ast::Expr>) -> Vec<ExprId> {
        exprs.map(|expr| self.expr(Some(expr))).collect()
    }

    /// A path made of one name, as the value of a shorthand field.
    pub(super) fn single_name_path(&mut self, name: Name, range: TextRange) -> crate::hir::Path {
        let id = crate::PathId(self.path_count);
        self.path_count += 1;
        crate::hir::Path {
            id,
            kind: PathKind::Plain,
            qself: None,
            segments: vec![crate::hir::PathSegment {
                name,
                range,
                args: None,
            }],
            range,
        }
    }
}

pub(super) fn label_name(label: Option<ast::Label>) -> Option<Name> {
    let lifetime = label?.lifetime()?;
    let text = lifetime.text();
    Some(Name::new(text.strip_prefix('\'').unwrap_or(text)))
}
