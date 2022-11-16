use sway_types::Ident;

use crate::{
    declaration_engine::{DeclMapping, ReplaceDecls},
    language::ty::*,
    type_system::*,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TyStructExpressionField {
    pub name: Ident,
    pub value: TyExpression,
}

impl CopyTypes for TyStructExpressionField {
    fn copy_types_inner(&mut self, type_mapping: &TypeMapping) {
        self.value.copy_types(type_mapping);
    }
}

impl ReplaceSelfType for TyStructExpressionField {
    fn replace_self_type(&mut self, self_type: TypeId) {
        self.value.replace_self_type(self_type);
    }
}

impl ReplaceDecls for TyStructExpressionField {
    fn replace_decls_inner(&mut self, decl_mapping: &DeclMapping) {
        self.value.replace_decls(decl_mapping);
    }
}
