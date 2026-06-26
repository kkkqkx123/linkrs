//! Full-Text Search Expression Functions
//!
//! This module provides expression functions for full-text search, including:
//! - score(): Get the relevance score of a document
//! - highlight(): Get highlighted text fragments
//! - matched_fields(): Get the list of matched fields
//! - snippet(): Get a text snippet

use crate::core::Value;
use crate::query::executor::expression::functions::signature::{FunctionSignature, ValueType};
use crate::query::executor::expression::{ExpressionError, ExpressionErrorType};
use crate::search::FulltextSearchEntry;
use std::collections::HashMap;

/// Full-text search function enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FulltextFunction {
    /// score() - Get document relevance score
    Score,
    /// highlight(field, [pre_tag], [post_tag], [fragment_size]) - Get highlighted text
    Highlight,
    /// matched_fields() - Get list of matched fields
    MatchedFields,
    /// snippet(field, [max_len]) - Get text snippet
    Snippet,
    /// rank() - Get the rank of the document in search results
    Rank,
    /// search_match(field, query) - Check if field matches query pattern
    SearchMatch,
    /// search_score(field) - Get field-specific relevance score
    FieldScore,
}

impl FulltextFunction {
    /// Get function name
    pub fn name(&self) -> &'static str {
        match self {
            FulltextFunction::Score => "score",
            FulltextFunction::Highlight => "highlight",
            FulltextFunction::MatchedFields => "matched_fields",
            FulltextFunction::Snippet => "snippet",
            FulltextFunction::Rank => "rank",
            FulltextFunction::SearchMatch => "search_match",
            FulltextFunction::FieldScore => "field_score",
        }
    }

    /// Get function signature
    pub fn signature(&self) -> FunctionSignature {
        match self {
            FulltextFunction::Score => {
                FunctionSignature::new("score", vec![], Some(ValueType::Float), false)
            }
            FulltextFunction::Highlight => FunctionSignature::new(
                "highlight",
                vec![
                    ValueType::String,
                    ValueType::String,
                    ValueType::String,
                    ValueType::Int,
                ],
                Some(ValueType::String),
                true,
            ),
            FulltextFunction::MatchedFields => {
                FunctionSignature::new("matched_fields", vec![], Some(ValueType::List), false)
            }
            FulltextFunction::Snippet => FunctionSignature::new(
                "snippet",
                vec![ValueType::String, ValueType::Int],
                Some(ValueType::String),
                true,
            ),
            FulltextFunction::Rank => {
                FunctionSignature::new("rank", vec![], Some(ValueType::Int), false)
            }
            FulltextFunction::SearchMatch => FunctionSignature::new(
                "search_match",
                vec![ValueType::String, ValueType::String],
                Some(ValueType::Bool),
                false,
            ),
            FulltextFunction::FieldScore => FunctionSignature::new(
                "field_score",
                vec![ValueType::String],
                Some(ValueType::Float),
                false,
            ),
        }
    }

    /// Get the number of parameters
    pub fn arity(&self) -> usize {
        match self {
            FulltextFunction::Score => 0,
            FulltextFunction::Highlight => 4,
            FulltextFunction::MatchedFields => 0,
            FulltextFunction::Snippet => 2,
            FulltextFunction::Rank => 0,
            FulltextFunction::SearchMatch => 2,
            FulltextFunction::FieldScore => 1,
        }
    }

    /// Check if variable parameters are accepted
    pub fn is_variadic(&self) -> bool {
        match self {
            FulltextFunction::Score => false,
            FulltextFunction::Highlight => true,
            FulltextFunction::MatchedFields => false,
            FulltextFunction::Snippet => true,
            FulltextFunction::Rank => false,
            FulltextFunction::SearchMatch => false,
            FulltextFunction::FieldScore => false,
        }
    }

    /// Get function description
    pub fn description(&self) -> &'static str {
        match self {
            FulltextFunction::Score => "Get the relevance score of a document in full-text search",
            FulltextFunction::Highlight => "Get highlighted text fragments for a field",
            FulltextFunction::MatchedFields => "Get the list of matched fields",
            FulltextFunction::Snippet => "Get a text snippet from a field",
            FulltextFunction::Rank => "Get the rank position of a document in search results",
            FulltextFunction::SearchMatch => "Check if a field matches a search query pattern",
            FulltextFunction::FieldScore => "Get the field-specific relevance score",
        }
    }

    /// Execute the function
    pub fn execute(
        &self,
        args: &[Value],
        context: &FulltextExecutionContext,
    ) -> Result<Value, ExpressionError> {
        match self {
            FulltextFunction::Score => self.execute_score(args, context),
            FulltextFunction::Highlight => self.execute_highlight(args, context),
            FulltextFunction::MatchedFields => self.execute_matched_fields(args, context),
            FulltextFunction::Snippet => self.execute_snippet(args, context),
            FulltextFunction::Rank => self.execute_rank(args, context),
            FulltextFunction::SearchMatch => self.execute_search_match(args, context),
            FulltextFunction::FieldScore => self.execute_field_score(args, context),
        }
    }

    /// Execute score() function
    fn execute_score(
        &self,
        args: &[Value],
        context: &FulltextExecutionContext,
    ) -> Result<Value, ExpressionError> {
        if !args.is_empty() {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                format!("score() expects 0 arguments, got {}", args.len()),
            ));
        }

        Ok(Value::Double(context.score))
    }

    /// Execute highlight() function
    fn execute_highlight(
        &self,
        args: &[Value],
        context: &FulltextExecutionContext,
    ) -> Result<Value, ExpressionError> {
        if args.is_empty() {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                "highlight() expects at least 1 argument (field name)",
            ));
        }

        let field_name = match &args[0] {
            Value::String(s) => s.clone(),
            _ => {
                return Err(ExpressionError::new(
                    ExpressionErrorType::TypeError,
                    "highlight() first argument must be a string (field name)",
                ));
            }
        };

        let pre_tag = args
            .get(1)
            .and_then(|v| {
                if let Value::String(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "<em>".to_string());

        let post_tag = args
            .get(2)
            .and_then(|v| {
                if let Value::String(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| "</em>".to_string());

        let fragment_size = args
            .get(3)
            .and_then(|v| {
                if let Value::Int(n) = v {
                    Some(*n as usize)
                } else {
                    None
                }
            })
            .unwrap_or(100);

        // Get highlights from context
        if let Some(highlights) = &context.highlights {
            if let Some(field_highlights) = highlights.get(&field_name) {
                if !field_highlights.is_empty() {
                    let highlighted_text = field_highlights.join(" ... ");
                    return Ok(Value::String(highlighted_text));
                }
            }
        }

        // Return original text if no highlights available
        if let Some(source) = &context.source {
            if let Some(Value::String(text)) = source.get(&field_name) {
                // Truncate if needed
                if text.len() > fragment_size {
                    return Ok(Value::String(format!(
                        "{}{}{}",
                        pre_tag,
                        &text[..fragment_size.min(text.len())],
                        post_tag
                    )));
                }
                return Ok(Value::String(text.clone()));
            }
        }

        Ok(Value::Null(crate::core::null::NullType::Null))
    }

    /// Execute matched_fields() function
    fn execute_matched_fields(
        &self,
        args: &[Value],
        context: &FulltextExecutionContext,
    ) -> Result<Value, ExpressionError> {
        if !args.is_empty() {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                "matched_fields() expects 0 arguments",
            ));
        }

        let fields: Vec<Value> = context
            .matched_fields
            .iter()
            .map(|f| Value::String(f.clone()))
            .collect();

        Ok(Value::list(crate::core::value::list::List {
            values: fields,
        }))
    }

    /// Execute snippet() function
    fn execute_snippet(
        &self,
        args: &[Value],
        context: &FulltextExecutionContext,
    ) -> Result<Value, ExpressionError> {
        if args.is_empty() {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                "snippet() expects at least 1 argument (field name)",
            ));
        }

        let field_name = match &args[0] {
            Value::String(s) => s.clone(),
            _ => {
                return Err(ExpressionError::new(
                    ExpressionErrorType::TypeError,
                    "snippet() first argument must be a string (field name)",
                ));
            }
        };

        let max_len = args
            .get(1)
            .and_then(|v| {
                if let Value::Int(n) = v {
                    Some(*n as usize)
                } else {
                    None
                }
            })
            .unwrap_or(200);

        // Get text from source
        if let Some(source) = &context.source {
            if let Some(Value::String(text)) = source.get(&field_name) {
                if text.len() <= max_len {
                    return Ok(Value::String(text.clone()));
                }

                // Try to find a good break point
                let break_point = text[..max_len].rfind(' ').unwrap_or(max_len);

                return Ok(Value::String(format!("{}...", &text[..break_point])));
            }
        }

        Ok(Value::Null(crate::core::null::NullType::Null))
    }

    /// Execute rank() function
    fn execute_rank(
        &self,
        args: &[Value],
        context: &FulltextExecutionContext,
    ) -> Result<Value, ExpressionError> {
        if !args.is_empty() {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                format!("rank() expects 0 arguments, got {}", args.len()),
            ));
        }

        let rank = if context.score > 0.0 {
            ((1.0 / context.score) * 100.0) as i64
        } else {
            0
        };

        Ok(Value::BigInt(rank))
    }

    /// Execute search_match() function
    fn execute_search_match(
        &self,
        args: &[Value],
        context: &FulltextExecutionContext,
    ) -> Result<Value, ExpressionError> {
        if args.len() < 2 {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                "search_match() expects 2 arguments (field, query)",
            ));
        }

        let field_name = match &args[0] {
            Value::String(s) => s.clone(),
            _ => {
                return Err(ExpressionError::new(
                    ExpressionErrorType::TypeError,
                    "search_match() first argument must be a string (field name)",
                ));
            }
        };

        let query = match &args[1] {
            Value::String(s) => s.to_lowercase(),
            _ => {
                return Err(ExpressionError::new(
                    ExpressionErrorType::TypeError,
                    "search_match() second argument must be a string (query)",
                ));
            }
        };

        if let Some(source) = &context.source {
            if let Some(Value::String(text)) = source.get(&field_name) {
                return Ok(Value::Bool(text.to_lowercase().contains(&query)));
            }
        }

        Ok(Value::Bool(false))
    }

    /// Execute field_score() function
    fn execute_field_score(
        &self,
        args: &[Value],
        context: &FulltextExecutionContext,
    ) -> Result<Value, ExpressionError> {
        if args.is_empty() {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                "field_score() expects 1 argument (field name)",
            ));
        }

        let field_name = match &args[0] {
            Value::String(s) => s.clone(),
            _ => {
                return Err(ExpressionError::new(
                    ExpressionErrorType::TypeError,
                    "field_score() first argument must be a string (field name)",
                ));
            }
        };

        if context.matched_fields.contains(&field_name) {
            let field_bonus = if context.highlights.as_ref().is_some_and(|h| h.contains_key(&field_name)) {
                1.5
            } else {
                1.0
            };
            return Ok(Value::Double(context.score * field_bonus));
        }

        Ok(Value::Double(0.0))
    }
}

/// Full-text search execution context
#[derive(Debug, Clone)]
pub struct FulltextExecutionContext {
    /// Current document score
    pub score: f64,
    /// Highlighted text by field
    pub highlights: Option<HashMap<String, Vec<String>>>,
    /// Matched field names
    pub matched_fields: Vec<String>,
    /// Source document data
    pub source: Option<HashMap<String, Value>>,
}

impl Default for FulltextExecutionContext {
    fn default() -> Self {
        Self {
            score: 0.0,
            highlights: None,
            matched_fields: Vec::new(),
            source: None,
        }
    }
}

impl FulltextExecutionContext {
    /// Create a new execution context
    pub fn new() -> Self {
        Self::default()
    }

    /// Create context from search result entry
    pub fn from_search_entry(entry: &FulltextSearchEntry) -> Self {
        Self {
            score: entry.score,
            highlights: entry.highlights.clone(),
            matched_fields: entry.matched_fields.clone(),
            source: entry.source.clone(),
        }
    }

    /// Set the score
    pub fn with_score(mut self, score: f64) -> Self {
        self.score = score;
        self
    }

    /// Set the highlights
    pub fn with_highlights(mut self, highlights: HashMap<String, Vec<String>>) -> Self {
        self.highlights = Some(highlights);
        self
    }

    /// Set the matched fields
    pub fn with_matched_fields(mut self, fields: Vec<String>) -> Self {
        self.matched_fields = fields;
        self
    }

    /// Set the source
    pub fn with_source(mut self, source: HashMap<String, Value>) -> Self {
        self.source = Some(source);
        self
    }
}

/// Register full-text search functions
pub fn register_fulltext_functions(
    registry: &mut crate::query::executor::expression::functions::FunctionRegistry,
) {
    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Fulltext(
            FulltextFunction::Score,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Fulltext(
            FulltextFunction::Highlight,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Fulltext(
            FulltextFunction::MatchedFields,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Fulltext(
            FulltextFunction::Snippet,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Fulltext(
            FulltextFunction::Rank,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Fulltext(
            FulltextFunction::SearchMatch,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Fulltext(
            FulltextFunction::FieldScore,
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_context() -> FulltextExecutionContext {
        let mut source = HashMap::new();
        source.insert(
            "title".to_string(),
            Value::String("Database Optimization".to_string()),
        );
        source.insert(
            "content".to_string(),
            Value::String(
                "This is a test article about database optimization techniques.".to_string(),
            ),
        );

        let mut highlights = HashMap::new();
        highlights.insert(
            "content".to_string(),
            vec!["<em>database</em> optimization".to_string()],
        );

        FulltextExecutionContext {
            score: 0.85,
            highlights: Some(highlights),
            matched_fields: vec!["title".to_string(), "content".to_string()],
            source: Some(source),
        }
    }

    #[test]
    fn test_score_function() {
        let func = FulltextFunction::Score;
        let context = create_test_context();

        let result = func.execute(&[], &context).unwrap();
        assert!(matches!(result, Value::Double(_)));
        if let Value::Double(score) = result {
            assert!((score - 0.85).abs() < 0.001);
        }
    }

    #[test]
    fn test_highlight_function() {
        let func = FulltextFunction::Highlight;
        let context = create_test_context();

        let result = func
            .execute(&[Value::String("content".to_string())], &context)
            .unwrap();

        assert!(matches!(result, Value::String(_)));
        if let Value::String(text) = result {
            assert!(text.contains("<em>"));
            assert!(text.contains("</em>"));
        }
    }

    #[test]
    fn test_matched_fields_function() {
        let func = FulltextFunction::MatchedFields;
        let context = create_test_context();

        let result = func.execute(&[], &context).unwrap();
        assert!(matches!(result, Value::List(_)));

        if let Value::List(fields) = result {
            assert_eq!(fields.len(), 2);
            assert!(fields.contains(&Value::String("title".to_string())));
            assert!(fields.contains(&Value::String("content".to_string())));
        }
    }

    #[test]
    fn test_snippet_function() {
        let func = FulltextFunction::Snippet;
        let context = create_test_context();

        let result = func
            .execute(
                &[Value::String("content".to_string()), Value::Int(50)],
                &context,
            )
            .unwrap();

        assert!(matches!(result, Value::String(_)));
        if let Value::String(text) = result {
            assert!(text.len() <= 53); // 50 + "..."
        }
    }
}
