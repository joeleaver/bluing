//! Structs and their fields: field access, struct literals, and the struct
//! a struct literal or pattern names.

use bluing_base::{Code, TextRange};
use bluing_hir::body::{ExprId, FieldInit};
use bluing_hir::hir::{Fields, Path};
use bluing_hir::resolve::{FieldOwner, Res, SelfTyOf};
use bluing_hir::{DefId, GenericOwner, Name};
use bluing_ty::fold::subst;
use bluing_ty::ty::{AdtId, InferTy, Substs, Ty, TyKind};

use crate::fn_ctxt::FnCtxt;

/// The struct or variant a struct literal or pattern names, with its type.
pub struct Named {
    pub ty: Ty,
    pub owner: FieldOwner,
    pub substs: Substs,
    pub is_variant: bool,
    /// For messages: `S` or `E::V`.
    pub name: String,
}

impl FnCtxt<'_, '_> {
    pub fn check_field(
        &mut self,
        expr: ExprId,
        base: ExprId,
        name: &Name,
        name_range: TextRange,
    ) -> Ty {
        let base_ty = self.check_expr(base);
        let mut current = base_ty;
        let mut derefs = 0;
        loop {
            match self.kind(current) {
                TyKind::Ref(_, pointee, _) => {
                    current = pointee;
                    derefs += 1;
                }
                TyKind::Adt(AdtId::Struct(id), substs) => {
                    let owner = FieldOwner::Struct(id);
                    match self.field_index(owner, name) {
                        Some(index) => {
                            self.check_field_visible(name_range, owner, index, name);
                            self.record_derefs(base, derefs);
                            self.results.field_indices.insert(expr, index);
                            return self.field_ty(owner, index, substs);
                        }
                        None => break,
                    }
                }
                TyKind::Tuple(list) => {
                    let tys = self.cx.interner.tys(list).to_vec();
                    match name
                        .as_str()
                        .parse::<usize>()
                        .ok()
                        .and_then(|index| tys.get(index).map(|&ty| (index, ty)))
                    {
                        Some((index, ty)) => {
                            self.record_derefs(base, derefs);
                            self.results.field_indices.insert(expr, index);
                            return ty;
                        }
                        None => break,
                    }
                }
                TyKind::Error => return current,
                TyKind::Infer(InferTy::Ty(_)) => {
                    self.error(
                        name_range,
                        Code::Inference,
                        "type annotations needed: the type of this value is not known here",
                    );
                    return self.cx.interner.error();
                }
                _ => break,
            }
        }
        let message = format!("no field `{name}` on type `{}`", self.display(base_ty));
        self.error(name_range, Code::Unresolved, message);
        self.cx.interner.error()
    }

    fn record_derefs(&mut self, expr: ExprId, derefs: u32) {
        if derefs > 0 {
            self.results.adjustments.entry(expr).or_default().derefs = derefs;
        }
    }

    pub fn fields_of(&self, owner: FieldOwner) -> &Fields {
        match owner {
            FieldOwner::Struct(id) => &self.cx.hir.structs[id].fields,
            FieldOwner::Variant(id) => &self.cx.hir.variants[id].fields,
        }
    }

    pub fn field_index(&self, owner: FieldOwner, name: &Name) -> Option<usize> {
        match self.fields_of(owner) {
            Fields::Record(fields) | Fields::Tuple(fields) => {
                fields.iter().position(|f| f.name == *name)
            }
            Fields::Unit => None,
        }
    }

    pub fn field_ty(&mut self, owner: FieldOwner, index: usize, substs: Substs) -> Ty {
        let ty = self.cx.items.fields[&owner][index];
        let ty = subst(&mut self.cx.interner, ty, substs);
        self.erase_regions(ty)
    }

    pub fn check_field_visible(
        &mut self,
        range: TextRange,
        owner: FieldOwner,
        index: usize,
        name: &Name,
    ) {
        let vis = self.cx.resolutions.field_visibilities[&owner][index];
        if !vis.is_accessible_from(self.cx.hir, self.module) {
            self.error(range, Code::Private, format!("field `{name}` is private"));
        }
    }

    /// The struct or variant named by the path of a struct literal or
    /// pattern. Returns `None` after reporting an error.
    pub fn named_struct(&mut self, range: TextRange, path: &Path) -> Option<Named> {
        let res = self.body.path_res(path)?;
        if path.segments.iter().any(|segment| segment.args.is_some()) {
            self.error(
                range,
                Code::NotYetSupported,
                "generic arguments on struct paths are not supported yet",
            );
            return None;
        }
        let hir = self.cx.hir;
        let (owner, adt, is_variant, name) = match res.base {
            Res::Def(DefId::Struct(id)) => (
                FieldOwner::Struct(id),
                AdtId::Struct(id),
                false,
                hir.structs[id].name.to_string(),
            ),
            Res::Def(DefId::Variant(id)) => {
                let parent = hir.variants[id].parent;
                let name = format!("{}::{}", hir.enums[parent].name, hir.variants[id].name);
                (FieldOwner::Variant(id), AdtId::Enum(parent), true, name)
            }
            Res::Def(DefId::TypeAlias(_)) | Res::SelfTy(SelfTyOf::Impl(_)) => {
                let ty = match res.base {
                    Res::Def(DefId::TypeAlias(id)) => {
                        if self
                            .lifetime_only_substs(GenericOwner::TypeAlias(id))
                            .is_none()
                        {
                            self.error(range, Code::NotYetSupported, "generic type aliases in struct literals and patterns are not supported yet");
                            return None;
                        }
                        self.cx.alias_ty(id)
                    }
                    Res::SelfTy(SelfTyOf::Impl(id)) => self.cx.impl_self_ty(id),
                    _ => unreachable!("matched above"),
                };
                let ty = self.erase_regions(ty);
                return match self.kind(ty) {
                    TyKind::Adt(AdtId::Struct(id), substs) => Some(Named {
                        ty,
                        owner: FieldOwner::Struct(id),
                        substs,
                        is_variant: false,
                        name: hir.structs[id].name.to_string(),
                    }),
                    TyKind::Error => None,
                    _ => {
                        let message = format!("expected a struct, found `{}`", self.display(ty));
                        self.error(range, Code::WrongKind, message);
                        None
                    }
                };
            }
            // Anything else was reported during name resolution.
            _ => return None,
        };
        let generic_owner = match adt {
            AdtId::Struct(id) => GenericOwner::Struct(id),
            AdtId::Enum(id) => GenericOwner::Enum(id),
        };
        let Some(substs) = self.lifetime_only_substs(generic_owner) else {
            self.error(
                range,
                Code::NotYetSupported,
                "values and patterns of generic types are not supported yet",
            );
            return None;
        };
        let ty = self.intern(TyKind::Adt(adt, substs));
        Some(Named {
            ty,
            owner,
            substs,
            is_variant,
            name,
        })
    }

    pub fn check_struct_literal(
        &mut self,
        expr: ExprId,
        path: &Path,
        fields: &[FieldInit],
        base: Option<ExprId>,
    ) -> Ty {
        let range = self.expr_range(expr);
        let Some(named) = self.named_struct(path.range, path) else {
            for field in fields {
                self.check_expr(field.value);
            }
            if let Some(base) = base {
                self.check_expr(base);
            }
            return self.cx.interner.error();
        };
        let field_count = match self.fields_of(named.owner) {
            Fields::Record(fields) | Fields::Tuple(fields) => fields.len(),
            Fields::Unit => 0,
        };
        let mut seen = vec![false; field_count];
        for field in fields {
            match self.field_index(named.owner, &field.name) {
                Some(index) => {
                    if seen[index] {
                        let message = format!("field `{}` is specified more than once", field.name);
                        self.error(field.range, Code::Duplicate, message);
                    }
                    seen[index] = true;
                    self.check_field_visible(field.range, named.owner, index, &field.name);
                    let ty = self.field_ty(named.owner, index, named.substs);
                    self.check_expr_coercible(field.value, ty);
                }
                None => {
                    let message = format!("`{}` has no field named `{}`", named.name, field.name);
                    self.error(field.range, Code::Unresolved, message);
                    self.check_expr(field.value);
                }
            }
        }
        match base {
            Some(base) if named.is_variant => {
                let message =
                    "functional update syntax (`..base`) requires a struct, not an enum variant";
                self.error(self.expr_range(base), Code::WrongKind, message);
                self.check_expr(base);
            }
            Some(base) => {
                self.check_expr_coercible(base, named.ty);
                // Fields taken from the base must be visible too.
                for index in (0..field_count).filter(|&i| !seen[i]) {
                    let name = self.field_name(named.owner, index);
                    self.check_field_visible(range, named.owner, index, &name);
                }
            }
            None => {
                let missing: Vec<String> = (0..field_count)
                    .filter(|&i| !seen[i])
                    .map(|i| format!("`{}`", self.field_name(named.owner, i)))
                    .collect();
                if !missing.is_empty() {
                    let message = format!(
                        "missing fields {} in initializer of `{}`",
                        missing.join(", "),
                        named.name
                    );
                    self.error(range, Code::MissingFields, message);
                }
            }
        }
        named.ty
    }

    pub fn field_name(&self, owner: FieldOwner, index: usize) -> Name {
        match self.fields_of(owner) {
            Fields::Record(fields) | Fields::Tuple(fields) => fields[index].name.clone(),
            Fields::Unit => unreachable!("unit structs have no fields"),
        }
    }
}
