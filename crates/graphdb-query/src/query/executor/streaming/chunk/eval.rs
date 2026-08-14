//! Batch expression evaluation on DataChunk

use crate::core::types::expr::Expression;
use crate::core::Value;
use crate::query::executor::expression::evaluator::operations::{
    BinaryOperationEvaluator, UnaryOperationEvaluator,
};
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::expression::ExpressionError;
use crate::query::executor::streaming::chunk::core::DataChunk;
use crate::query::executor::streaming::context::BorrowedRowContext;
use crate::query::executor::streaming::slot::SlotId;
use crate::query::executor::streaming::subquery::EvalEnv;
use std::collections::HashMap;

use super::typed::{
    typed_binary_batch, typed_cast_batch, typed_column_batch, typed_literal_batch,
    typed_unary_batch, TypedBatch,
};

impl DataChunk {
    // ── Batch expression evaluation ──

    pub fn evaluate_expressions(
        &mut self,
        expressions: &[Expression],
        env: Option<&EvalEnv>,
    ) -> Result<Vec<Vec<Value>>, ExpressionError> {
        if self.rows.is_empty() {
            return Ok(vec![Vec::new(); expressions.len()]);
        }
        if expressions
            .iter()
            .all(|e| matches!(e, Expression::Variable(_)))
        {
            let mut columns = Vec::with_capacity(expressions.len());
            for expr in expressions {
                if let Expression::Variable(name) = expr {
                    let slot = self
                        .layout
                        .slot_id(name)
                        .ok_or_else(|| ExpressionError::undefined_variable(name))?;
                    let col = self
                        .get_column(slot)
                        .ok_or_else(|| ExpressionError::undefined_variable(name))?;
                    columns.push(col);
                }
            }
            for _ in 0..expressions.len() {
                self.count_columnar(true);
            }
            return Ok(columns);
        }
        if expressions
            .iter()
            .all(|e| matches!(e, Expression::Literal(_)))
        {
            let mut columns = Vec::with_capacity(expressions.len());
            for expr in expressions {
                if let Expression::Literal(v) = expr {
                    columns.push(vec![v.clone(); self.rows.len()]);
                }
            }
            for _ in 0..expressions.len() {
                self.count_columnar(true);
            }
            return Ok(columns);
        }
        let mut results = Vec::with_capacity(expressions.len());
        for expr in expressions {
            results.push(self.evaluate_expression(expr, env)?);
        }
        Ok(results)
    }

    pub fn evaluate_expression(
        &mut self,
        expression: &Expression,
        env: Option<&EvalEnv>,
    ) -> Result<Vec<Value>, ExpressionError> {
        if self.rows.is_empty() {
            return Ok(Vec::new());
        }
        if let Ok((result, typed_hit)) = self.try_evaluate_columnar(expression, env) {
            self.count_columnar(true);
            if typed_hit {
                self.count_typed_hit();
            }
            return Ok(result);
        }
        self.count_columnar(false);
        debug_assert!(
            !self.columnar_promise_holds(expression),
            "flat column promise broken: expression {:?} should have hit the \
             columnar path but fell back to per-row evaluation",
            expression
        );
        self.evaluate_expression_per_row(expression, env)
    }

    pub fn evaluate_expression_visible(
        &mut self,
        expression: &Expression,
        env: Option<&EvalEnv>,
    ) -> Result<Vec<Value>, ExpressionError> {
        let Some(sel) = self.selection().map(|s| s.to_vec()) else {
            return self.evaluate_expression(expression, env);
        };
        let slot: Option<SlotId> = match expression {
            Expression::Variable(name) => self.layout.slot_id(name),
            Expression::Property { object, property }
                if matches!(object.as_ref(), Expression::Variable(_)) =>
            {
                if let Expression::Variable(var) = object.as_ref() {
                    self.layout.slot_id(&format!("{}.{}", var, property))
                } else {
                    None
                }
            }
            _ => None,
        };
        if let Some(slot) = slot {
            let mut out = Vec::with_capacity(sel.len());
            for &i in &sel {
                match self.get_typed_by_slot(i, slot) {
                    Some(v) => out.push(v),
                    None => return Err(ExpressionError::undefined_variable("column slot")),
                }
            }
            self.count_columnar(true);
            self.count_selection_pushed();
            return Ok(out);
        }
        let layout = self.get_layout();
        let mut out = Vec::with_capacity(sel.len());
        for &i in &sel {
            let row = &self.rows[i];
            let mut ctx = match env {
                Some(env) => BorrowedRowContext::with_env(row, layout.clone(), env),
                None => BorrowedRowContext::new(row, layout.clone()),
            };
            out.push(ExpressionEvaluator::evaluate(expression, &mut ctx)?);
        }
        self.count_columnar(false);
        self.count_selection_pushed();
        Ok(out)
    }

    fn try_evaluate_columnar(
        &mut self,
        expression: &Expression,
        env: Option<&EvalEnv>,
    ) -> Result<(Vec<Value>, bool), ExpressionError> {
        let mut col_cache: HashMap<String, Vec<Value>> = HashMap::new();
        self.collect_variables(expression, &mut col_cache);
        let mut typed_hit = false;
        let result = self.eval_with_cache(expression, &col_cache, env, &mut typed_hit)?;
        Ok((result, typed_hit))
    }

    fn collect_variables(
        &mut self,
        expr: &Expression,
        col_cache: &mut HashMap<String, Vec<Value>>,
    ) {
        match expr {
            Expression::Variable(name) => {
                if !col_cache.contains_key(name) {
                    if let Some(slot) = self.layout.slot_id(name) {
                        if let Some(col) = self.get_column(slot) {
                            col_cache.insert(name.clone(), col);
                        }
                    }
                }
            }
            Expression::Binary { left, right, .. } => {
                self.collect_variables(left, col_cache);
                self.collect_variables(right, col_cache);
            }
            Expression::Unary { operand, .. } => {
                self.collect_variables(operand, col_cache);
            }
            Expression::TypeCast { expression, .. } => {
                self.collect_variables(expression, col_cache);
            }
            _ => {}
        }
    }

    fn eval_with_cache(
        &mut self,
        expression: &Expression,
        col_cache: &HashMap<String, Vec<Value>>,
        env: Option<&EvalEnv>,
        typed_used: &mut bool,
    ) -> Result<Vec<Value>, ExpressionError> {
        if let Some(batch) = self.try_eval_typed_batch(expression, env)? {
            *typed_used = true;
            return Ok(batch.into_values());
        }
        match expression {
            Expression::Literal(v) => Ok(vec![v.clone(); self.rows.len()]),

            Expression::Variable(name) => {
                if let Some(col) = col_cache.get(name) {
                    return Ok(col.clone());
                }
                let slot = self
                    .layout
                    .slot_id(name)
                    .ok_or_else(|| ExpressionError::undefined_variable(name))?;
                self.get_column(slot)
                    .ok_or_else(|| ExpressionError::undefined_variable(name))
            }

            Expression::Parameter(name) => {
                let val = env
                    .and_then(|env| env.params.as_ref())
                    .and_then(|p| p.get(name).cloned())
                    .ok_or_else(|| ExpressionError::undefined_parameter(name))?;
                Ok(vec![val; self.rows.len()])
            }

            Expression::Unary { op, operand } => {
                let values = self.eval_with_cache(operand, col_cache, env, typed_used)?;
                values
                    .into_iter()
                    .map(|v| UnaryOperationEvaluator::evaluate(op, &v))
                    .collect()
            }

            Expression::Binary { left, op, right } => {
                let left_values = self.eval_with_cache(left, col_cache, env, typed_used)?;
                let right_values = self.eval_with_cache(right, col_cache, env, typed_used)?;
                left_values
                    .into_iter()
                    .zip(right_values)
                    .map(|(l, r)| BinaryOperationEvaluator::evaluate(&l, op, &r))
                    .collect()
            }

            Expression::TypeCast {
                expression,
                target_type,
            } => {
                let values = self.eval_with_cache(expression, col_cache, env, typed_used)?;
                values
                    .into_iter()
                    .map(|v| ExpressionEvaluator::eval_type_cast(&v, target_type))
                    .collect()
            }

            Expression::Property { object, property } => {
                if let Expression::Variable(var_name) = object.as_ref() {
                    let compound = format!("{}.{}", var_name, property);
                    if let Some(slot) = self.layout.slot_id(&compound) {
                        if let Some(col) = self.get_column(slot) {
                            return Ok(col);
                        }
                    }
                    return Err(ExpressionError::type_error(
                        "Property access requires per-row evaluation",
                    ));
                }
                Err(ExpressionError::type_error(
                    "Property access requires per-row evaluation",
                ))
            }

            _ => Err(ExpressionError::type_error(
                "Expression requires per-row evaluation",
            )),
        }
    }

    fn evaluate_expression_per_row(
        &self,
        expression: &Expression,
        env: Option<&EvalEnv>,
    ) -> Result<Vec<Value>, ExpressionError> {
        let layout = self.get_layout();
        let mut results = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let mut ctx = match env {
                Some(env) => BorrowedRowContext::with_env(row, layout.clone(), env),
                None => BorrowedRowContext::new(row, layout.clone()),
            };
            results.push(ExpressionEvaluator::evaluate(expression, &mut ctx)?);
        }
        Ok(results)
    }

    // ── Typed batch evaluation ──

    fn try_eval_typed_batch(
        &mut self,
        expression: &Expression,
        env: Option<&EvalEnv>,
    ) -> Result<Option<TypedBatch>, ExpressionError> {
        match expression {
            Expression::Literal(v) => Ok(typed_literal_batch(v, self.rows.len())),
            Expression::Parameter(name) => {
                let val = env
                    .and_then(|env| env.params.as_ref())
                    .and_then(|p| p.get(name).cloned())
                    .ok_or_else(|| ExpressionError::undefined_parameter(name))?;
                Ok(typed_literal_batch(&val, self.rows.len()))
            }
            Expression::Variable(name) => {
                let slot = match self.layout.slot_id(name) {
                    Some(slot) => slot,
                    None => return Ok(None),
                };
                Ok(self.typed_column(slot).and_then(typed_column_batch))
            }
            Expression::Property { object, property } => {
                // Flat property access (e.g. `p.age`) resolves to the
                // compound slot `p.age` in the layout; when that column is
                // typed (I64/F64/I32/Bool) the whole predicate can run on the
                // raw batch instead of per-row Value evaluation.
                let Expression::Variable(var_name) = object.as_ref() else {
                    return Ok(None);
                };
                let compound = format!("{}.{}", var_name, property);
                let slot = match self.layout.slot_id(&compound) {
                    Some(slot) => slot,
                    None => return Ok(None),
                };
                Ok(self.typed_column(slot).and_then(typed_column_batch))
            }
            Expression::Unary { op, operand } => {
                let Some(batch) = self.try_eval_typed_batch(operand, env)? else {
                    return Ok(None);
                };
                Ok(typed_unary_batch(op, batch))
            }
            Expression::Binary { left, op, right } => {
                let Some(left_batch) = self.try_eval_typed_batch(left, env)? else {
                    return Ok(None);
                };
                let Some(right_batch) = self.try_eval_typed_batch(right, env)? else {
                    return Ok(None);
                };
                Ok(typed_binary_batch(op, &left_batch, &right_batch))
            }
            Expression::TypeCast {
                expression,
                target_type,
            } => {
                let Some(batch) = self.try_eval_typed_batch(expression, env)? else {
                    return Ok(None);
                };
                Ok(typed_cast_batch(batch, target_type))
            }
            _ => Ok(None),
        }
    }

    #[cfg(debug_assertions)]
    pub(super) fn columnar_promise_holds(&self, expr: &Expression) -> bool {
        match expr {
            Expression::Literal(_) | Expression::Parameter(_) | Expression::Variable(_) => true,
            Expression::Unary { operand, .. } => self.columnar_promise_holds(operand),
            Expression::Binary { left, right, .. } => {
                self.columnar_promise_holds(left) && self.columnar_promise_holds(right)
            }
            Expression::TypeCast { expression, .. } => self.columnar_promise_holds(expression),
            Expression::Property { object, property } => {
                if let Expression::Variable(var) = object.as_ref() {
                    let compound = format!("{}.{}", var, property);
                    return self.layout.slot_id(&compound).is_some();
                }
                false
            }
            _ => false,
        }
    }

    #[cfg(not(debug_assertions))]
    fn columnar_promise_holds(&self, _expr: &Expression) -> bool {
        false
    }
}
