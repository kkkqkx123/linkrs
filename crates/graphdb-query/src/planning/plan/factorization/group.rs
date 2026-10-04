use std::collections::HashMap;

use super::FactorizationError;
use graphdb_core::types::expr::ExpressionId;

/// A group of expressions sharing the same nesting level.
///
/// Mirrors `lbug::planner::FactorizationGroup` in
/// `ref/ladybug/src/include/planner/operator/schema.h`.
#[derive(Debug, Clone)]
pub struct FactorizationGroup {
    flat: bool,
    single_state: bool,
    cardinality_multiplier: f64,
    expressions: Vec<ExpressionId>,
    expression_id_to_pos: HashMap<ExpressionId, usize>,
    expression_name_to_pos: HashMap<String, usize>,
}

impl FactorizationGroup {
    pub fn new() -> Self {
        Self {
            flat: false,
            single_state: false,
            cardinality_multiplier: 1.0,
            expressions: Vec::new(),
            expression_id_to_pos: HashMap::new(),
            expression_name_to_pos: HashMap::new(),
        }
    }

    pub fn new_flat(single_state: bool) -> Self {
        Self {
            flat: true,
            single_state,
            cardinality_multiplier: 1.0,
            expressions: Vec::new(),
            expression_id_to_pos: HashMap::new(),
            expression_name_to_pos: HashMap::new(),
        }
    }

    pub fn is_flat(&self) -> bool {
        self.flat
    }

    pub fn is_single_state(&self) -> bool {
        self.single_state
    }

    pub fn set_flat(&mut self) -> Result<(), FactorizationError> {
        if self.flat {
            return Err(FactorizationError::GroupAlreadyFlat);
        }
        self.flat = true;
        Ok(())
    }

    pub fn set_single_state(&mut self) -> Result<(), FactorizationError> {
        if self.single_state {
            return Err(FactorizationError::GroupAlreadySingleState);
        }
        self.single_state = true;
        if !self.flat {
            self.flat = true;
        }
        Ok(())
    }

    pub fn cardinality_multiplier(&self) -> f64 {
        self.cardinality_multiplier
    }

    pub fn set_multiplier(&mut self, multiplier: f64) {
        self.cardinality_multiplier = multiplier;
    }

    pub fn expressions(&self) -> &[ExpressionId] {
        &self.expressions
    }

    pub fn len(&self) -> usize {
        self.expressions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.expressions.is_empty()
    }

    pub fn insert_expression(&mut self, expr_id: ExpressionId) -> Result<(), FactorizationError> {
        self.insert_expression_with_name(expr_id, None)
    }

    pub fn insert_expression_with_name(
        &mut self,
        expr_id: ExpressionId,
        name: Option<String>,
    ) -> Result<(), FactorizationError> {
        if self.expression_id_to_pos.contains_key(&expr_id) {
            return Err(FactorizationError::DuplicateExpressionId(expr_id));
        }
        if let Some(n) = name {
            if self.expression_name_to_pos.contains_key(&n) {
                return Err(FactorizationError::DuplicateExpressionName(n));
            }
            self.expression_name_to_pos
                .insert(n, self.expressions.len());
        }
        self.expression_id_to_pos
            .insert(expr_id.clone(), self.expressions.len());
        self.expressions.push(expr_id);
        Ok(())
    }

    pub fn get_expression_pos(&self, expr_id: &ExpressionId) -> Option<usize> {
        self.expression_id_to_pos.get(expr_id).copied()
    }

    pub fn contains(&self, expr_id: &ExpressionId) -> bool {
        self.expression_id_to_pos.contains_key(expr_id)
    }

    pub fn contains_name(&self, name: &str) -> bool {
        self.expression_name_to_pos.contains_key(name)
    }
}

impl Default for FactorizationGroup {
    fn default() -> Self {
        Self::new()
    }
}
