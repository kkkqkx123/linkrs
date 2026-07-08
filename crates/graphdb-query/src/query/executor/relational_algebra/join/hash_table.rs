//! Hash table implementation, used for the join operation
//!
//! Provide an efficient hash table for use in join operations.

use crate::core::types::expr::Expression;
use crate::core::{DBError, DBResult, Value};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::executor::expression::evaluator::ExpressionEvaluator;
use crate::query::executor::expression::DefaultExpressionContext;
use crate::query::DataSet;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};

/// The “Join” key supports efficient hashing and serialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JoinKey {
    values: Vec<Value>,
    /// Precomputed hash values to avoid duplicate calculations.
    cached_hash: u64,
}

impl JoinKey {
    pub fn new(values: Vec<Value>) -> Self {
        let cached_hash = Self::calculate_hash(&values);
        Self {
            values,
            cached_hash,
        }
    }

    fn calculate_hash(values: &[Value]) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        let mut hasher = DefaultHasher::new();
        for value in values {
            value.hash(&mut hasher);
        }
        hasher.finish()
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&Value> {
        self.values.get(index)
    }
}

impl Hash for JoinKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_u64(self.cached_hash);
    }
}

/// Hash table entry, containing row data and metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HashTableEntry {
    /// Row data
    pub row: Vec<Value>,
    /// Original row index (used for debugging and processing duplicate data)
    pub original_index: usize,
}

impl HashTableEntry {
    pub fn new(row: Vec<Value>, original_index: usize) -> Self {
        Self {
            row,
            original_index,
        }
    }
}

/// Hash table
pub struct HashTable {
    /// hash table
    table: HashMap<JoinKey, Vec<HashTableEntry>>,
}

impl HashTable {
    /// Create a new hash table.
    pub fn new(initial_capacity: usize) -> Self {
        Self {
            table: HashMap::with_capacity(initial_capacity),
        }
    }

    /// Insert an entry
    pub fn insert(&mut self, key: JoinKey, entry: HashTableEntry) -> DBResult<()> {
        self.table.entry(key).or_default().push(entry);
        Ok(())
    }

    /// Detecting hash tables
    pub fn probe(&self, key: &JoinKey) -> Vec<HashTableEntry> {
        self.table
            .get(key)
            .map_or_else(Vec::new, |entries| entries.clone())
    }

    /// Get the number of entries
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Check whether it is empty.
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    /// Clear the hash table.
    pub fn clear(&mut self) {
        self.table.clear();
    }
}

/// Hash table builder
pub struct HashTableBuilder;

impl HashTableBuilder {
    /// Constructing a hash table from a dataset
    pub fn build_from_dataset(
        dataset: &DataSet,
        key_indices: &[usize],
        initial_capacity: usize,
    ) -> DBResult<HashTable> {
        let mut hash_table = HashTable::new(initial_capacity);

        for (idx, row) in dataset.rows.iter().enumerate() {
            let mut key_values = Vec::new();
            for &key_index in key_indices {
                if key_index < row.len() {
                    key_values.push(row[key_index].clone());
                } else {
                    return Err(DBError::validation(format!(
                        "Key index {} out of bounds for row with {} columns",
                        key_index,
                        row.len()
                    )));
                }
            }

            let key = JoinKey::new(key_values);
            let entry = HashTableEntry::new(row.clone(), idx);

            hash_table.insert(key, entry)?;
        }

        Ok(hash_table)
    }
}

/// Construct a hash table function (that accepts an expression)
pub fn build_hash_table(
    dataset: &DataSet,
    key_exprs: &[Expression],
) -> Result<HashMap<JoinKey, Vec<usize>>, String> {
    let mut hash_table = HashMap::new();

    for (idx, row) in dataset.rows.iter().enumerate() {
        let mut expr_context = DefaultExpressionContext::new();
        for (i, col_name) in dataset.col_names.iter().enumerate() {
            if i < row.len() {
                expr_context.set_variable(col_name.clone(), row[i].clone());
            }
        }

        let mut key_values = Vec::new();
        for key_expression in key_exprs {
            match ExpressionEvaluator::evaluate(key_expression, &mut expr_context) {
                Ok(value) => key_values.push(value),
                Err(e) => return Err(format!("Key expression failed: {}", e)),
            }
        }

        let key = JoinKey::new(key_values);
        hash_table.entry(key).or_insert_with(Vec::new).push(idx);
    }

    Ok(hash_table)
}

/// Extract the key-value pairs.
pub fn extract_key_values(
    row: &[Value],
    _col_names: &[String],
    key_exprs: &[Expression],
    col_map: &std::collections::HashMap<&str, usize>,
) -> Vec<Value> {
    let mut key_values = Vec::new();
    for key_expression in key_exprs {
        let mut expr_context = DefaultExpressionContext::new();
        for (col_name, &col_idx) in col_map.iter() {
            if col_idx < row.len() {
                expr_context.set_variable(col_name.to_string(), row[col_idx].clone());
            }
        }
        if let Ok(value) = ExpressionEvaluator::evaluate(key_expression, &mut expr_context) {
            key_values.push(value);
        }
    }
    key_values
}

impl std::fmt::Debug for HashTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HashTable")
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_key() {
        let key1 = JoinKey::new(vec![Value::Int(1), Value::String("test".to_string())]);
        let key2 = JoinKey::new(vec![Value::Int(1), Value::String("test".to_string())]);

        assert_eq!(key1, key2);
        assert_eq!(key1.len(), 2);
        assert!(!key1.is_empty());
    }

    #[test]
    fn test_hash_table_entry() {
        let entry = HashTableEntry::new(vec![Value::Int(1), Value::String("test".to_string())], 0);

        assert_eq!(entry.original_index, 0);
        assert_eq!(entry.row.len(), 2);
    }

    #[test]
    fn test_hash_table_basic() {
        let mut hash_table = HashTable::new(100);

        let key = JoinKey::new(vec![Value::Int(1)]);
        let entry = HashTableEntry::new(vec![Value::String("test".to_string())], 0);

        hash_table
            .insert(key.clone(), entry)
            .expect("insert should succeed");

        let results = hash_table.probe(&key);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].row[0], Value::String("test".to_string()));

        assert_eq!(hash_table.len(), 1);
        assert!(!hash_table.is_empty());

        hash_table.clear();
        assert_eq!(hash_table.len(), 0);
        assert!(hash_table.is_empty());
    }

    #[test]
    fn test_build_hash_table() {
        let mut dataset = DataSet::new();
        dataset.col_names = vec!["id".to_string(), "name".to_string()];
        dataset
            .rows
            .push(vec![Value::Int(1), Value::String("Alice".to_string())]);
        dataset
            .rows
            .push(vec![Value::Int(2), Value::String("Bob".to_string())]);

        let hash_table =
            HashTableBuilder::build_from_dataset(&dataset, &[0], 10).expect("build should succeed");

        assert_eq!(hash_table.len(), 2);

        let key = JoinKey::new(vec![Value::Int(1)]);
        let results = hash_table.probe(&key);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_build_hash_table_with_expr() {
        let mut dataset = DataSet::new();
        dataset.col_names = vec!["id".to_string(), "name".to_string()];
        dataset
            .rows
            .push(vec![Value::Int(1), Value::String("Alice".to_string())]);
        dataset
            .rows
            .push(vec![Value::Int(2), Value::String("Bob".to_string())]);

        let id_expr = Expression::Variable("id".to_string());
        let hash_table = build_hash_table(&dataset, &[id_expr]).expect("build should succeed");

        assert_eq!(hash_table.len(), 2);

        let key = JoinKey::new(vec![Value::Int(1)]);
        let indices = hash_table.get(&key);
        assert!(indices.is_some());
        assert_eq!(indices.expect("The index should exist"), &vec![0]);
    }
}
