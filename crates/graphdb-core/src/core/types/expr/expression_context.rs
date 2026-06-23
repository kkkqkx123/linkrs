use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use super::contextual::ContextualExpression;
use super::{Expression, ExpressionId, ExpressionMeta};
use crate::core::types::operators::BinaryOperator;
use crate::core::types::operators::UnaryOperator;
use crate::core::types::DataType;
use crate::core::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OptimizationFlags {
    pub typed: bool,
    pub constant_folded: bool,
    pub cse_eliminated: bool,
}

#[derive(Debug)]
pub struct ExpressionAnalysisContext {
    expressions: Arc<RwLock<HashMap<ExpressionId, Arc<ExpressionMeta>>>>,
    type_cache: Arc<RwLock<HashMap<ExpressionId, DataType>>>,
    constant_cache: Arc<RwLock<HashMap<ExpressionId, Value>>>,
    optimization_flags: Arc<RwLock<HashMap<ExpressionId, OptimizationFlags>>>,
}

impl ExpressionAnalysisContext {
    pub fn new() -> Self {
        Self {
            expressions: Arc::new(RwLock::new(HashMap::new())),
            type_cache: Arc::new(RwLock::new(HashMap::new())),
            constant_cache: Arc::new(RwLock::new(HashMap::new())),
            optimization_flags: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn register_expression(&self, expr: ExpressionMeta) -> ExpressionId {
        let id = expr
            .id()
            .cloned()
            .unwrap_or_else(|| ExpressionId::new(self.expressions.read().len() as u64));

        self.expressions.write().insert(id.clone(), Arc::new(expr));
        id
    }

    pub fn get_expression(&self, id: &ExpressionId) -> Option<Arc<ExpressionMeta>> {
        self.expressions.read().get(id).cloned()
    }

    pub fn set_type(&self, id: &ExpressionId, data_type: DataType) {
        self.type_cache.write().insert(id.clone(), data_type);
        let mut flags = self
            .optimization_flags
            .read()
            .get(id)
            .copied()
            .unwrap_or_default();
        flags.typed = true;
        self.optimization_flags.write().insert(id.clone(), flags);
    }

    pub fn get_type(&self, id: &ExpressionId) -> Option<DataType> {
        self.type_cache.read().get(id).cloned()
    }

    pub fn set_constant(&self, id: &ExpressionId, value: Value) {
        self.constant_cache.write().insert(id.clone(), value);
        self.optimization_flags.write().insert(
            id.clone(),
            OptimizationFlags {
                typed: true,
                constant_folded: true,
                cse_eliminated: false,
            },
        );
    }

    pub fn get_constant(&self, id: &ExpressionId) -> Option<Value> {
        self.constant_cache.read().get(id).cloned()
    }

    pub fn set_optimization_flag(&self, id: &ExpressionId, flags: OptimizationFlags) {
        self.optimization_flags.write().insert(id.clone(), flags);
    }

    pub fn get_optimization_flags(&self, id: &ExpressionId) -> Option<OptimizationFlags> {
        self.optimization_flags.read().get(id).copied()
    }

    pub fn is_constant(&self, id: &ExpressionId) -> bool {
        self.constant_cache.read().contains_key(id)
    }

    pub fn is_typed(&self, id: &ExpressionId) -> bool {
        self.optimization_flags
            .read()
            .get(id)
            .map(|f| f.typed)
            .unwrap_or(false)
    }

    pub fn is_constant_folded(&self, id: &ExpressionId) -> bool {
        self.optimization_flags
            .read()
            .get(id)
            .map(|f| f.constant_folded)
            .unwrap_or(false)
    }

    pub fn is_cse_eliminated(&self, id: &ExpressionId) -> bool {
        self.optimization_flags
            .read()
            .get(id)
            .map(|f| f.cse_eliminated)
            .unwrap_or(false)
    }

    pub fn expression_count(&self) -> usize {
        self.expressions.read().len()
    }

    pub fn clear_caches(&self) {
        self.type_cache.write().clear();
        self.constant_cache.write().clear();
        self.optimization_flags.write().clear();
    }

    pub fn clear_all(&self) {
        self.expressions.write().clear();
        self.clear_caches();
    }

    pub fn clone_expression(
        &self,
        ctx_expr: &ContextualExpression,
    ) -> Option<ContextualExpression> {
        let expr_meta = ctx_expr.expression()?;
        let inner_expr = expr_meta.inner().clone();
        let meta = ExpressionMeta::new(inner_expr);
        let id = self.register_expression(meta);
        Some(ContextualExpression::new(id, ctx_expr.context().clone()))
    }

    pub fn combine_expressions(
        &self,
        op: BinaryOperator,
        left: &ContextualExpression,
        right: &ContextualExpression,
    ) -> Option<ContextualExpression> {
        let left_meta = left.expression()?;
        let right_meta = right.expression()?;

        let combined_expr = Expression::Binary {
            left: Box::new(left_meta.inner().clone()),
            op,
            right: Box::new(right_meta.inner().clone()),
        };

        let meta = ExpressionMeta::new(combined_expr);
        let id = self.register_expression(meta);
        Some(ContextualExpression::new(id, left.context().clone()))
    }

    pub fn create_unary_expression(
        &self,
        op: UnaryOperator,
        operand: &ContextualExpression,
    ) -> Option<ContextualExpression> {
        let operand_meta = operand.expression()?;

        let unary_expr = Expression::Unary {
            op,
            operand: Box::new(operand_meta.inner().clone()),
        };

        let meta = ExpressionMeta::new(unary_expr);
        let id = self.register_expression(meta);
        Some(ContextualExpression::new(id, operand.context().clone()))
    }

    pub fn create_property_expression(
        &self,
        object: &ContextualExpression,
        property: &str,
    ) -> Option<ContextualExpression> {
        let object_meta = object.expression()?;

        let property_expr = Expression::Property {
            object: Box::new(object_meta.inner().clone()),
            property: property.to_string(),
        };

        let meta = ExpressionMeta::new(property_expr);
        let id = self.register_expression(meta);
        Some(ContextualExpression::new(id, object.context().clone()))
    }

    pub fn create_function_expression(
        &self,
        name: &str,
        args: &[ContextualExpression],
        ctx_expr: &ContextualExpression,
    ) -> Option<ContextualExpression> {
        let arg_exprs: Vec<Expression> = args
            .iter()
            .filter_map(|arg| arg.expression().map(|meta| meta.inner().clone()))
            .collect();

        if arg_exprs.len() != args.len() {
            return None;
        }

        let function_expr = Expression::Function {
            name: name.to_string(),
            args: arg_exprs,
        };

        let meta = ExpressionMeta::new(function_expr);
        let id = self.register_expression(meta);
        Some(ContextualExpression::new(id, ctx_expr.context().clone()))
    }

    pub fn and(
        &self,
        left: &ContextualExpression,
        right: &ContextualExpression,
    ) -> Option<ContextualExpression> {
        self.combine_expressions(BinaryOperator::And, left, right)
    }

    pub fn or(
        &self,
        left: &ContextualExpression,
        right: &ContextualExpression,
    ) -> Option<ContextualExpression> {
        self.combine_expressions(BinaryOperator::Or, left, right)
    }

    pub fn not(&self, operand: &ContextualExpression) -> Option<ContextualExpression> {
        self.create_unary_expression(UnaryOperator::Not, operand)
    }
}

impl Clone for ExpressionAnalysisContext {
    fn clone(&self) -> Self {
        Self {
            expressions: Arc::new(RwLock::new(self.expressions.read().clone())),
            type_cache: Arc::new(RwLock::new(self.type_cache.read().clone())),
            constant_cache: Arc::new(RwLock::new(self.constant_cache.read().clone())),
            optimization_flags: Arc::new(RwLock::new(self.optimization_flags.read().clone())),
        }
    }
}

impl Default for ExpressionAnalysisContext {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::BinaryOperator;

    #[test]
    fn test_expression_context_creation() {
        let ctx = ExpressionAnalysisContext::new();
        assert_eq!(ctx.expression_count(), 0);
    }

    #[test]
    fn test_register_expression() {
        let ctx = ExpressionAnalysisContext::new();
        let expr = Expression::literal(42);
        let meta = ExpressionMeta::new(expr);

        let id = ctx.register_expression(meta);
        assert_eq!(ctx.expression_count(), 1);
        assert_eq!(id.0, 0);
    }

    #[test]
    fn test_register_expression_with_id() {
        let ctx = ExpressionAnalysisContext::new();
        let expr = Expression::literal("test");
        let meta = ExpressionMeta::new(expr).with_id(ExpressionId::new(100));

        let id = ctx.register_expression(meta);
        assert_eq!(id.0, 100);
    }

    #[test]
    fn test_get_expression() {
        let ctx = ExpressionAnalysisContext::new();
        let expr = Expression::variable("x");
        let meta = ExpressionMeta::new(expr);

        let id = ctx.register_expression(meta);
        let retrieved = ctx.get_expression(&id);
        assert!(retrieved.is_some());
        assert!(retrieved
            .expect("The expression should exist")
            .is_variable());
    }

    #[test]
    fn test_set_and_get_type() {
        let ctx = ExpressionAnalysisContext::new();
        let expr = Expression::literal(42);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        ctx.set_type(&id, DataType::Int);
        let data_type = ctx.get_type(&id);
        assert_eq!(data_type, Some(DataType::Int));
        assert!(ctx.is_typed(&id));
    }

    #[test]
    fn test_set_and_get_constant() {
        let ctx = ExpressionAnalysisContext::new();
        let expr = Expression::binary(
            Expression::literal(1),
            BinaryOperator::Add,
            Expression::literal(2),
        );
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        ctx.set_constant(&id, Value::Int(3));
        let constant = ctx.get_constant(&id);
        assert_eq!(constant, Some(Value::Int(3)));
        assert!(ctx.is_constant(&id));
        assert!(ctx.is_constant_folded(&id));
    }

    #[test]
    fn test_optimization_flags() {
        let ctx = ExpressionAnalysisContext::new();
        let expr = Expression::variable("x");
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        let flags = OptimizationFlags {
            typed: true,
            constant_folded: false,
            cse_eliminated: true,
        };
        ctx.set_optimization_flag(&id, flags);

        let retrieved = ctx.get_optimization_flags(&id);
        assert_eq!(retrieved, Some(flags));
        assert!(ctx.is_typed(&id));
        assert!(!ctx.is_constant_folded(&id));
        assert!(ctx.is_cse_eliminated(&id));
    }

    #[test]
    fn test_clear_caches() {
        let ctx = ExpressionAnalysisContext::new();
        let expr = Expression::literal(42);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        ctx.set_type(&id, DataType::Int);
        ctx.set_constant(&id, Value::Int(42));

        ctx.clear_caches();

        assert!(ctx.get_type(&id).is_none());
        assert!(ctx.get_constant(&id).is_none());
        assert_eq!(ctx.expression_count(), 1);
    }

    #[test]
    fn test_clear_all() {
        let ctx = ExpressionAnalysisContext::new();
        let expr = Expression::literal(42);
        let meta = ExpressionMeta::new(expr);
        let id = ctx.register_expression(meta);

        ctx.set_type(&id, DataType::Int);

        ctx.clear_all();

        assert_eq!(ctx.expression_count(), 0);
        assert!(ctx.get_expression(&id).is_none());
    }

    #[test]
    fn test_default() {
        let ctx = ExpressionAnalysisContext::default();
        assert_eq!(ctx.expression_count(), 0);
    }
}
