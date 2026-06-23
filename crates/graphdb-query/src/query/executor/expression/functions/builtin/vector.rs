//! Vector Functions Module
//!
//! This module provides vector operations functions for SQL queries, including:
//! - Similarity functions (cosine_similarity, dot_product, euclidean_distance, manhattan_distance)
//! - Vector property functions (dimension, l2_norm, nnz, normalize)
//! - Vector access functions (element access, slicing)

use crate::core::Value;
use crate::query::executor::expression::functions::signature::{FunctionSignature, ValueType};
use crate::query::executor::expression::{ExpressionError, ExpressionErrorType};

/// Vector function enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VectorFunction {
    /// cosine_similarity(vec1, vec2) - Compute cosine similarity between two vectors
    CosineSimilarity,
    /// dot_product(vec1, vec2) - Compute dot product of two vectors
    DotProduct,
    /// euclidean_distance(vec1, vec2) - Compute Euclidean distance between two vectors
    EuclideanDistance,
    /// manhattan_distance(vec1, vec2) - Compute Manhattan distance between two vectors
    ManhattanDistance,
    /// dimension(vector) - Get the dimension of a vector
    Dimension,
    /// l2_norm(vector) - Compute L2 norm of a vector
    L2Norm,
    /// nnz(vector) - Get number of non-zero elements
    Nnz,
    /// normalize(vector) - Normalize a vector to unit length
    Normalize,
}

impl VectorFunction {
    /// Get function name
    pub fn name(&self) -> &'static str {
        match self {
            VectorFunction::CosineSimilarity => "cosine_similarity",
            VectorFunction::DotProduct => "dot_product",
            VectorFunction::EuclideanDistance => "euclidean_distance",
            VectorFunction::ManhattanDistance => "manhattan_distance",
            VectorFunction::Dimension => "dimension",
            VectorFunction::L2Norm => "l2_norm",
            VectorFunction::Nnz => "nnz",
            VectorFunction::Normalize => "normalize",
        }
    }

    /// Get function signature
    pub fn signature(&self) -> FunctionSignature {
        match self {
            VectorFunction::CosineSimilarity => FunctionSignature::new(
                "cosine_similarity",
                vec![ValueType::Any, ValueType::Any],
                Some(ValueType::Float),
                false,
            ),
            VectorFunction::DotProduct => FunctionSignature::new(
                "dot_product",
                vec![ValueType::Any, ValueType::Any],
                Some(ValueType::Float),
                false,
            ),
            VectorFunction::EuclideanDistance => FunctionSignature::new(
                "euclidean_distance",
                vec![ValueType::Any, ValueType::Any],
                Some(ValueType::Float),
                false,
            ),
            VectorFunction::ManhattanDistance => FunctionSignature::new(
                "manhattan_distance",
                vec![ValueType::Any, ValueType::Any],
                Some(ValueType::Float),
                false,
            ),
            VectorFunction::Dimension => FunctionSignature::new(
                "dimension",
                vec![ValueType::Any],
                Some(ValueType::Int),
                false,
            ),
            VectorFunction::L2Norm => FunctionSignature::new(
                "l2_norm",
                vec![ValueType::Any],
                Some(ValueType::Float),
                false,
            ),
            VectorFunction::Nnz => {
                FunctionSignature::new("nnz", vec![ValueType::Any], Some(ValueType::Int), false)
            }
            VectorFunction::Normalize => FunctionSignature::new(
                "normalize",
                vec![ValueType::Any],
                Some(ValueType::Any),
                false,
            ),
        }
    }

    /// Get the number of parameters
    pub fn arity(&self) -> usize {
        match self {
            VectorFunction::CosineSimilarity => 2,
            VectorFunction::DotProduct => 2,
            VectorFunction::EuclideanDistance => 2,
            VectorFunction::ManhattanDistance => 2,
            VectorFunction::Dimension => 1,
            VectorFunction::L2Norm => 1,
            VectorFunction::Nnz => 1,
            VectorFunction::Normalize => 1,
        }
    }

    /// Check if variable parameters are accepted
    pub fn is_variadic(&self) -> bool {
        false
    }

    /// Get function description
    pub fn description(&self) -> &'static str {
        match self {
            VectorFunction::CosineSimilarity => "Compute cosine similarity between two vectors",
            VectorFunction::DotProduct => "Compute dot product of two vectors",
            VectorFunction::EuclideanDistance => "Compute Euclidean distance between two vectors",
            VectorFunction::ManhattanDistance => "Compute Manhattan distance between two vectors",
            VectorFunction::Dimension => "Get the dimension of a vector",
            VectorFunction::L2Norm => "Compute L2 norm of a vector",
            VectorFunction::Nnz => "Get number of non-zero elements in a vector",
            VectorFunction::Normalize => "Normalize a vector to unit length",
        }
    }

    /// Execute the function
    pub fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match self {
            VectorFunction::CosineSimilarity => self.execute_cosine_similarity(args),
            VectorFunction::DotProduct => self.execute_dot_product(args),
            VectorFunction::EuclideanDistance => self.execute_euclidean_distance(args),
            VectorFunction::ManhattanDistance => self.execute_manhattan_distance(args),
            VectorFunction::Dimension => self.execute_dimension(args),
            VectorFunction::L2Norm => self.execute_l2_norm(args),
            VectorFunction::Nnz => self.execute_nnz(args),
            VectorFunction::Normalize => self.execute_normalize(args),
        }
    }

    /// Execute cosine_similarity function
    fn execute_cosine_similarity(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        if args.len() != 2 {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                format!(
                    "cosine_similarity() expects 2 arguments, got {}",
                    args.len()
                ),
            ));
        }

        let vec1 = extract_vector(&args[0])?;
        let vec2 = extract_vector(&args[1])?;

        let similarity = compute_cosine_similarity(&vec1, &vec2)?;
        Ok(Value::Double(similarity as f64))
    }

    /// Execute dot_product function
    fn execute_dot_product(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        if args.len() != 2 {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                format!("dot_product() expects 2 arguments, got {}", args.len()),
            ));
        }

        let vec1 = extract_vector(&args[0])?;
        let vec2 = extract_vector(&args[1])?;

        let dot = compute_dot_product(&vec1, &vec2)?;
        Ok(Value::Double(dot as f64))
    }

    /// Execute euclidean_distance function
    fn execute_euclidean_distance(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        if args.len() != 2 {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                format!(
                    "euclidean_distance() expects 2 arguments, got {}",
                    args.len()
                ),
            ));
        }

        let vec1 = extract_vector(&args[0])?;
        let vec2 = extract_vector(&args[1])?;

        let distance = compute_euclidean_distance(&vec1, &vec2)?;
        Ok(Value::Double(distance as f64))
    }

    /// Execute manhattan_distance function
    fn execute_manhattan_distance(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        if args.len() != 2 {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                format!(
                    "manhattan_distance() expects 2 arguments, got {}",
                    args.len()
                ),
            ));
        }

        let vec1 = extract_vector(&args[0])?;
        let vec2 = extract_vector(&args[1])?;

        let distance = compute_manhattan_distance(&vec1, &vec2)?;
        Ok(Value::Double(distance as f64))
    }

    /// Execute dimension function
    fn execute_dimension(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        if args.len() != 1 {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                format!("dimension() expects 1 argument, got {}", args.len()),
            ));
        }

        let vec = extract_vector(&args[0])?;
        Ok(Value::BigInt(vec.len() as i64))
    }

    /// Execute l2_norm function
    fn execute_l2_norm(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        if args.len() != 1 {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                format!("l2_norm() expects 1 argument, got {}", args.len()),
            ));
        }

        let vec = extract_vector(&args[0])?;
        let norm = compute_l2_norm(&vec);
        Ok(Value::Double(norm as f64))
    }

    /// Execute nnz function
    fn execute_nnz(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        if args.len() != 1 {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                format!("nnz() expects 1 argument, got {}", args.len()),
            ));
        }

        let vec = extract_vector(&args[0])?;
        let nnz = vec.iter().filter(|&&x| x != 0.0).count();
        Ok(Value::BigInt(nnz as i64))
    }

    /// Execute normalize function
    fn execute_normalize(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        if args.len() != 1 {
            return Err(ExpressionError::new(
                ExpressionErrorType::InvalidArgumentCount,
                format!("normalize() expects 1 argument, got {}", args.len()),
            ));
        }

        let vec = extract_vector(&args[0])?;
        let norm = compute_l2_norm(&vec);

        if norm == 0.0 {
            return Ok(Value::vector(vec)); // Return original vector if norm is zero
        }

        let normalized: Vec<f32> = vec.iter().map(|&x| x / norm).collect();
        Ok(Value::vector(normalized))
    }
}

/// Extract vector from Value
fn extract_vector(value: &Value) -> Result<Vec<f32>, ExpressionError> {
    match value {
        Value::Vector(vec) => Ok(vec.to_dense()),
        Value::List(list) => {
            // Try to convert List<Float> or List<Int> to vector
            let mut vec = Vec::with_capacity(list.values.len());
            for item in &list.values {
                match item {
                    Value::Float(f) => vec.push(*f),
                    Value::Double(f) => vec.push(*f as f32),
                    Value::SmallInt(i) => vec.push(*i as f32),
                    Value::Int(i) => vec.push(*i as f32),
                    Value::BigInt(i) => vec.push(*i as f32),
                    _ => {
                        return Err(ExpressionError::new(
                            ExpressionErrorType::TypeError,
                            format!("Vector elements must be numeric, got {:?}", item.get_type()),
                        ))
                    }
                }
            }
            Ok(vec)
        }
        _ => Err(ExpressionError::new(
            ExpressionErrorType::TypeError,
            format!(
                "Expected vector or list of numbers, got {:?}",
                value.get_type()
            ),
        )),
    }
}

/// Compute cosine similarity between two vectors
fn compute_cosine_similarity(vec1: &[f32], vec2: &[f32]) -> Result<f32, ExpressionError> {
    if vec1.len() != vec2.len() {
        return Err(ExpressionError::new(
            ExpressionErrorType::TypeError,
            format!(
                "Vector dimensions must match: {} vs {}",
                vec1.len(),
                vec2.len()
            ),
        ));
    }

    let dot_product: f32 = vec1.iter().zip(vec2.iter()).map(|(&a, &b)| a * b).sum();
    let norm1: f32 = vec1.iter().map(|&x| x * x).sum::<f32>().sqrt();
    let norm2: f32 = vec2.iter().map(|&x| x * x).sum::<f32>().sqrt();

    if norm1 == 0.0 || norm2 == 0.0 {
        return Ok(0.0);
    }

    Ok(dot_product / (norm1 * norm2))
}

/// Compute dot product of two vectors
fn compute_dot_product(vec1: &[f32], vec2: &[f32]) -> Result<f32, ExpressionError> {
    if vec1.len() != vec2.len() {
        return Err(ExpressionError::new(
            ExpressionErrorType::TypeError,
            format!(
                "Vector dimensions must match: {} vs {}",
                vec1.len(),
                vec2.len()
            ),
        ));
    }

    Ok(vec1.iter().zip(vec2.iter()).map(|(&a, &b)| a * b).sum())
}

/// Compute Euclidean distance between two vectors
fn compute_euclidean_distance(vec1: &[f32], vec2: &[f32]) -> Result<f32, ExpressionError> {
    if vec1.len() != vec2.len() {
        return Err(ExpressionError::new(
            ExpressionErrorType::TypeError,
            format!(
                "Vector dimensions must match: {} vs {}",
                vec1.len(),
                vec2.len()
            ),
        ));
    }

    let sum: f32 = vec1
        .iter()
        .zip(vec2.iter())
        .map(|(&a, &b)| (a - b).powi(2))
        .sum();
    Ok(sum.sqrt())
}

/// Compute Manhattan distance between two vectors
fn compute_manhattan_distance(vec1: &[f32], vec2: &[f32]) -> Result<f32, ExpressionError> {
    if vec1.len() != vec2.len() {
        return Err(ExpressionError::new(
            ExpressionErrorType::TypeError,
            format!(
                "Vector dimensions must match: {} vs {}",
                vec1.len(),
                vec2.len()
            ),
        ));
    }

    Ok(vec1
        .iter()
        .zip(vec2.iter())
        .map(|(&a, &b)| (a - b).abs())
        .sum())
}

/// Compute L2 norm of a vector
fn compute_l2_norm(vec: &[f32]) -> f32 {
    vec.iter().map(|&x| x * x).sum::<f32>().sqrt()
}

/// Register vector functions
pub fn register_vector_functions(
    registry: &mut crate::query::executor::expression::functions::FunctionRegistry,
) {
    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Vector(
            VectorFunction::CosineSimilarity,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Vector(
            VectorFunction::DotProduct,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Vector(
            VectorFunction::EuclideanDistance,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Vector(
            VectorFunction::ManhattanDistance,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Vector(
            VectorFunction::Dimension,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Vector(
            VectorFunction::L2Norm,
        ),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Vector(VectorFunction::Nnz),
    );

    registry.register_builtin(
        crate::query::executor::expression::functions::BuiltinFunction::Vector(
            VectorFunction::Normalize,
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity() {
        let vec1 = Value::vector(vec![1.0, 0.0, 0.0]);
        let vec2 = Value::vector(vec![0.0, 1.0, 0.0]);

        let result = VectorFunction::CosineSimilarity
            .execute(&[vec1, vec2])
            .unwrap();

        assert!(matches!(result, Value::Double(_)));
        if let Value::Double(score) = result {
            assert!((score - 0.0).abs() < 1e-6);
        }
    }

    #[test]
    fn test_dot_product() {
        let vec1 = Value::vector(vec![1.0, 2.0, 3.0]);
        let vec2 = Value::vector(vec![4.0, 5.0, 6.0]);

        let result = VectorFunction::DotProduct.execute(&[vec1, vec2]).unwrap();

        assert!(matches!(result, Value::Double(_)));
        if let Value::Double(dot) = result {
            assert!((dot - 32.0).abs() < 1e-6);
        }
    }

    #[test]
    fn test_euclidean_distance() {
        let vec1 = Value::vector(vec![0.0, 0.0]);
        let vec2 = Value::vector(vec![3.0, 4.0]);

        let result = VectorFunction::EuclideanDistance
            .execute(&[vec1, vec2])
            .unwrap();

        assert!(matches!(result, Value::Double(_)));
        if let Value::Double(dist) = result {
            assert!((dist - 5.0).abs() < 1e-6);
        }
    }

    #[test]
    fn test_manhattan_distance() {
        let vec1 = Value::vector(vec![0.0, 0.0]);
        let vec2 = Value::vector(vec![3.0, 4.0]);

        let result = VectorFunction::ManhattanDistance
            .execute(&[vec1, vec2])
            .unwrap();

        assert!(matches!(result, Value::Double(_)));
        if let Value::Double(dist) = result {
            assert!((dist - 7.0).abs() < 1e-6);
        }
    }

    #[test]
    fn test_dimension() {
        let vec = Value::vector(vec![0.1; 1536]);

        let result = VectorFunction::Dimension.execute(&[vec]).unwrap();

        assert!(matches!(result, Value::BigInt(_)));
        if let Value::BigInt(dim) = result {
            assert_eq!(dim, 1536);
        }
    }

    #[test]
    fn test_l2_norm() {
        let vec = Value::vector(vec![3.0, 4.0]);

        let result = VectorFunction::L2Norm.execute(&[vec]).unwrap();

        assert!(matches!(result, Value::Double(_)));
        if let Value::Double(norm) = result {
            assert!((norm - 5.0).abs() < 1e-6);
        }
    }

    #[test]
    fn test_nnz() {
        let vec = Value::vector(vec![1.0, 0.0, 2.0, 0.0, 3.0]);

        let result = VectorFunction::Nnz.execute(&[vec]).unwrap();

        assert!(matches!(result, Value::BigInt(_)));
        if let Value::BigInt(n) = result {
            assert_eq!(n, 3);
        }
    }

    #[test]
    fn test_normalize() {
        let vec = Value::vector(vec![3.0, 4.0]);

        let result = VectorFunction::Normalize.execute(&[vec]).unwrap();

        assert!(matches!(result, Value::Vector(_)));
        if let Value::Vector(normalized) = result {
            let data = normalized.to_dense();
            assert!((data[0] - 0.6).abs() < 1e-6); // 3/5 = 0.6
            assert!((data[1] - 0.8).abs() < 1e-6); // 4/5 = 0.8
        }
    }

    #[test]
    fn test_dimension_mismatch() {
        let vec1 = Value::vector(vec![1.0, 2.0]);
        let vec2 = Value::vector(vec![1.0, 2.0, 3.0]);

        let result = VectorFunction::CosineSimilarity.execute(&[vec1, vec2]);
        assert!(result.is_err());
    }
}
