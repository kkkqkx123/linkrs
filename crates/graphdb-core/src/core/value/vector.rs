//! Vector Type Module
//!
//! This module provides vector types for efficient storage and manipulation of vector data.
//! Vector types are essential for vector similarity search and embedding operations.

use serde::{Deserialize, Serialize};
use std::hash::Hash;

/// Vector value type for efficient storage of numerical vectors
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VectorValue {
    /// Dense vector - most common, stores all values
    Dense(Vec<f32>),
    /// Sparse vector - stores only non-zero values with indices
    Sparse { indices: Vec<u32>, values: Vec<f32> },
}

impl VectorValue {
    /// Create a new dense vector
    pub fn dense(data: Vec<f32>) -> Self {
        VectorValue::Dense(data)
    }

    /// Create a new sparse vector
    pub fn sparse(indices: Vec<u32>, values: Vec<f32>) -> Self {
        VectorValue::Sparse { indices, values }
    }

    /// Get the dimension of the vector
    pub fn dimension(&self) -> usize {
        match self {
            VectorValue::Dense(data) => data.len(),
            VectorValue::Sparse { indices, .. } => {
                indices.last().map(|i| *i as usize + 1).unwrap_or(0)
            }
        }
    }

    /// Get the number of non-zero elements
    pub fn nnz(&self) -> usize {
        match self {
            VectorValue::Dense(data) => data.iter().filter(|&&x| x != 0.0).count(),
            VectorValue::Sparse { values, .. } => values.len(),
        }
    }

    /// Convert to dense vector (Vec<f32>)
    pub fn into_dense(self) -> Option<Vec<f32>> {
        match self {
            VectorValue::Dense(data) => Some(data),
            VectorValue::Sparse { .. } => None,
        }
    }

    /// Get reference to dense vector data
    pub fn as_dense(&self) -> Option<&[f32]> {
        match self {
            VectorValue::Dense(data) => Some(data),
            VectorValue::Sparse { .. } => None,
        }
    }

    /// Validate dimension matches expected value
    pub fn validate_dimension(&self, expected: usize) -> Result<(), VectorError> {
        let actual = self.dimension();
        if actual != expected {
            Err(VectorError::DimensionMismatch { expected, actual })
        } else {
            Ok(())
        }
    }

    /// Check if this is a sparse vector
    pub fn is_sparse(&self) -> bool {
        matches!(self, VectorValue::Sparse { .. })
    }

    /// Check if this is a dense vector
    pub fn is_dense(&self) -> bool {
        matches!(self, VectorValue::Dense(_))
    }

    /// Convert sparse to dense representation
    pub fn to_dense(&self) -> Vec<f32> {
        match self {
            VectorValue::Dense(data) => data.clone(),
            VectorValue::Sparse { indices, values } => {
                let dim = self.dimension();
                let mut dense = vec![0.0f32; dim];
                for (&idx, &val) in indices.iter().zip(values.iter()) {
                    if idx < dim as u32 {
                        dense[idx as usize] = val;
                    }
                }
                dense
            }
        }
    }

    /// Estimate memory usage in bytes
    pub fn estimated_size(&self) -> usize {
        match self {
            VectorValue::Dense(data) => {
                std::mem::size_of::<Self>() + data.capacity() * std::mem::size_of::<f32>()
            }
            VectorValue::Sparse { indices, values } => {
                std::mem::size_of::<Self>()
                    + indices.capacity() * std::mem::size_of::<u32>()
                    + values.capacity() * std::mem::size_of::<f32>()
            }
        }
    }

    /// Compute dot product with another vector
    pub fn dot(&self, other: &VectorValue) -> Result<f32, VectorError> {
        let dim = self.dimension();
        if dim != other.dimension() {
            return Err(VectorError::DimensionMismatch {
                expected: dim,
                actual: other.dimension(),
            });
        }

        match (self, other) {
            (VectorValue::Dense(a), VectorValue::Dense(b)) => {
                Ok(a.iter().zip(b.iter()).map(|(&x, &y)| x * y).sum())
            }
            (
                VectorValue::Sparse {
                    indices: idx_a,
                    values: val_a,
                },
                VectorValue::Sparse {
                    indices: idx_b,
                    values: val_b,
                },
            ) => {
                // Sparse dot product - only multiply non-zero elements
                let mut result = 0.0f32;
                let mut i = 0;
                let mut j = 0;
                while i < idx_a.len() && j < idx_b.len() {
                    if idx_a[i] == idx_b[j] {
                        result += val_a[i] * val_b[j];
                        i += 1;
                        j += 1;
                    } else if idx_a[i] < idx_b[j] {
                        i += 1;
                    } else {
                        j += 1;
                    }
                }
                Ok(result)
            }
            _ => {
                // Mixed sparse/dense - convert sparse to dense
                Ok(self
                    .to_dense()
                    .iter()
                    .zip(other.to_dense().iter())
                    .map(|(&x, &y)| x * y)
                    .sum())
            }
        }
    }

    /// Compute L2 norm (Euclidean norm)
    pub fn l2_norm(&self) -> f32 {
        match self {
            VectorValue::Dense(data) => data.iter().map(|&x| x * x).sum::<f32>().sqrt(),
            VectorValue::Sparse { values, .. } => values.iter().map(|&x| x * x).sum::<f32>().sqrt(),
        }
    }

    /// Compute cosine similarity with another vector
    pub fn cosine_similarity(&self, other: &VectorValue) -> Result<f32, VectorError> {
        let dot = self.dot(other)?;
        let norm_self = self.l2_norm();
        let norm_other = other.l2_norm();

        if norm_self == 0.0 || norm_other == 0.0 {
            return Ok(0.0);
        }

        Ok(dot / (norm_self * norm_other))
    }
}

impl PartialEq for VectorValue {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (VectorValue::Dense(a), VectorValue::Dense(b)) => a == b,
            (
                VectorValue::Sparse {
                    indices: idx_a,
                    values: val_a,
                },
                VectorValue::Sparse {
                    indices: idx_b,
                    values: val_b,
                },
            ) => idx_a == idx_b && val_a == val_b,
            _ => {
                // Compare dense representations for mixed types
                self.to_dense() == other.to_dense()
            }
        }
    }
}

impl Eq for VectorValue {}

impl Hash for VectorValue {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match self {
            VectorValue::Dense(data) => {
                "dense".hash(state);
                // Hash f32 values by their bit representation
                for &val in data {
                    val.to_bits().hash(state);
                }
            }
            VectorValue::Sparse { indices, values } => {
                "sparse".hash(state);
                indices.hash(state);
                // Hash f32 values by their bit representation
                for &val in values {
                    val.to_bits().hash(state);
                }
            }
        }
    }
}

impl std::fmt::Display for VectorValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VectorValue::Dense(data) => {
                write!(f, "[")?;
                for (i, val) in data.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{:.6}", val)?;
                }
                write!(f, "]")
            }
            VectorValue::Sparse { indices, values } => {
                write!(f, "sparse[")?;
                for (i, (&idx, &val)) in indices.iter().zip(values.iter()).enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}:{:.6}", idx, val)?;
                }
                write!(f, "]")
            }
        }
    }
}

/// Vector error types
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VectorError {
    DimensionMismatch { expected: usize, actual: usize },
    InvalidSparseIndices,
    OutOfBounds { index: usize, dimension: usize },
    InvalidOperation(String),
}

impl std::fmt::Display for VectorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VectorError::DimensionMismatch { expected, actual } => {
                write!(
                    f,
                    "Dimension mismatch: expected {}, got {}",
                    expected, actual
                )
            }
            VectorError::InvalidSparseIndices => {
                write!(f, "Invalid sparse vector indices")
            }
            VectorError::OutOfBounds { index, dimension } => {
                write!(
                    f,
                    "Index {} out of bounds for dimension {}",
                    index, dimension
                )
            }
            VectorError::InvalidOperation(op) => {
                write!(f, "Invalid vector operation: {}", op)
            }
        }
    }
}

impl std::error::Error for VectorError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dense_vector_creation() {
        let vec = VectorValue::dense(vec![1.0, 2.0, 3.0]);
        assert_eq!(vec.dimension(), 3);
        assert!(vec.is_dense());
        assert!(!vec.is_sparse());
    }

    #[test]
    fn test_sparse_vector_creation() {
        let vec = VectorValue::sparse(vec![0, 2, 5], vec![1.0, 2.0, 3.0]);
        assert_eq!(vec.dimension(), 6);
        assert_eq!(vec.nnz(), 3);
        assert!(vec.is_sparse());
    }

    #[test]
    fn test_vector_dimension() {
        let dense = VectorValue::dense(vec![1.0; 1536]);
        assert_eq!(dense.dimension(), 1536);

        let sparse = VectorValue::sparse(vec![0, 100, 1535], vec![1.0, 2.0, 3.0]);
        assert_eq!(sparse.dimension(), 1536);
    }

    #[test]
    fn test_vector_dot_product() {
        let a = VectorValue::dense(vec![1.0, 2.0, 3.0]);
        let b = VectorValue::dense(vec![4.0, 5.0, 6.0]);
        let dot = a.dot(&b).unwrap();
        assert!((dot - 32.0).abs() < 1e-6);
    }

    #[test]
    fn test_vector_cosine_similarity() {
        let a = VectorValue::dense(vec![1.0, 0.0, 0.0]);
        let b = VectorValue::dense(vec![0.0, 1.0, 0.0]);
        let similarity = a.cosine_similarity(&b).unwrap();
        assert!(similarity.abs() < 1e-6);

        let c = VectorValue::dense(vec![1.0, 0.0, 0.0]);
        let similarity_same = a.cosine_similarity(&c).unwrap();
        assert!((similarity_same - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_vector_to_dense() {
        let sparse = VectorValue::sparse(vec![0, 2], vec![1.0, 3.0]);
        let dense = sparse.to_dense();
        assert_eq!(dense, vec![1.0, 0.0, 3.0]);
    }

    #[test]
    fn test_vector_equality() {
        let a = VectorValue::dense(vec![1.0, 2.0, 3.0]);
        let b = VectorValue::dense(vec![1.0, 2.0, 3.0]);
        assert_eq!(a, b);

        let sparse_a = VectorValue::sparse(vec![0, 2], vec![1.0, 3.0]);
        let sparse_b = VectorValue::sparse(vec![0, 2], vec![1.0, 3.0]);
        assert_eq!(sparse_a, sparse_b);
    }

    #[test]
    fn test_vector_memory_usage() {
        let dense = VectorValue::dense(vec![1.0; 1000]);
        let size = dense.estimated_size();
        assert!(size > 0);
        assert!(size < 10000); // Should be around 4KB + overhead
    }
}
