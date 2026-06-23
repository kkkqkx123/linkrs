//! Tag Filter Processor
//!
//! Provide an advanced filtering function for vertex labels that supports the evaluation of complex expressions.

use crate::core::value::list::List;
use crate::core::vertex_edge_path::Vertex;
use crate::core::Expression;
use crate::core::Value;
use crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator;
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::DefaultExpressionContext;

/// Tag Filter Processor
///
/// Using the unit struct pattern, with zero overhead.
#[derive(Debug)]
pub struct TagFilterProcessor;

impl TagFilterProcessor {
    /// Processing tag filtering expressions
    pub fn process_tag_filter(filter_expression: &Expression, vertex: &Vertex) -> bool {
        // Create a context that includes vertex labels.
        let mut context = Self::create_tag_context(vertex);

        // Evaluating an expression
        match ExpressionEvaluator::evaluate(filter_expression, &mut context) {
            Ok(value) => Self::value_to_bool(&value),
            Err(e) => {
                log::warn!("Tag filter expression evaluation failed: {}", e);
                false // Default exclusion
            }
        }
    }

    /// Create an evaluation context that includes tag information.
    fn create_tag_context(vertex: &Vertex) -> DefaultExpressionContext {
        let mut context = DefaultExpressionContext::new();

        // Add the vertices as variables.
        context.set_variable(
            "vertex".to_string(),
            Value::Vertex(Box::new(vertex.clone())),
        );

        // Add a list of tags
        let tag_names: Vec<Value> = vertex
            .tags
            .iter()
            .map(|tag| Value::String(tag.name.clone()))
            .collect();
        context.set_variable("tags".to_string(), Value::list(List::from(tag_names)));

        // Number of tags added
        let tag_count = vertex.tags.len() as i64;
        context.set_variable("tag_count".to_string(), Value::BigInt(tag_count));

        // Add each tag as a separate variable.
        for (i, tag) in vertex.tags.iter().enumerate() {
            context.set_variable(format!("tag_{}", tag.name), Value::String(tag.name.clone()));
            context.set_variable(format!("tag_{}", i), Value::String(tag.name.clone()));
        }

        // Add tag attributes
        for tag in &vertex.tags {
            let tag_prefix = format!("tag_{}_", tag.name);
            for (prop_name, prop_value) in &tag.properties {
                context.set_variable(format!("{}{}", tag_prefix, prop_name), prop_value.clone());
            }
        }

        context
    }

    /// Convert the value to a boolean value.
    fn value_to_bool(value: &Value) -> bool {
        match value {
            Value::Bool(b) => *b,
            Value::Null(_) => false,
            Value::Empty => false,
            Value::Int(0) => false,
            Value::Float(0.0) => false,
            Value::String(s) if s.is_empty() => false,
            Value::List(l) if l.is_empty() => false,
            Value::Map(m) if m.is_empty() => false,
            Value::Set(s) if s.is_empty() => false,
            _ => true, // Non-empty, non-zero values are considered to be true.
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::operators::BinaryOperator;
    use crate::core::types::VertexId;
    use crate::core::vertex_edge_path::{Tag, Vertex};

    #[test]
    fn test_process_tag_filter_with_contains() {
        // Create test vertices.
        let vertex = Vertex::new(
            VertexId::from_int64(1),
            vec![
                Tag::new("user".to_string(), std::collections::HashMap::new()),
                Tag::new("admin".to_string(), std::collections::HashMap::new()),
            ],
        );

        // The test includes expressions with tags – “user” IN tags
        let expression = Expression::binary(
            Expression::literal("user".to_string()),
            BinaryOperator::In,
            Expression::variable("tags".to_string()),
        );

        assert!(TagFilterProcessor::process_tag_filter(&expression, &vertex));
    }

    #[test]
    fn test_process_tag_filter_with_count() {
        let vertex = Vertex::new(
            VertexId::from_int64(1),
            vec![
                Tag::new("user".to_string(), std::collections::HashMap::new()),
                Tag::new("admin".to_string(), std::collections::HashMap::new()),
            ],
        );

        let expression = Expression::binary(
            Expression::variable("tag_count".to_string()),
            BinaryOperator::GreaterThan,
            Expression::literal(1i64),
        );

        assert!(TagFilterProcessor::process_tag_filter(&expression, &vertex));
    }
}
