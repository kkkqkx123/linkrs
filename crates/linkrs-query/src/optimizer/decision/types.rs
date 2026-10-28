//! Optimize the definition of decision types
//!
//! Only execution choices consumed by the plan and the executor are kept.
//! The former aggregate decision container and its traversal, index, and
//! join-order companions had no production consumer: the engine records
//! join choices in the plan join algorithm map and observable notes instead.

/// Connection algorithms
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum JoinAlgorithm {
    /// Hash link
    HashJoin {
        /// Constructing the names for the side variables
        build_side: String,
        /// Variable name on the detection side
        probe_side: String,
    },
    /// Nested loop connection
    NestedLoopJoin {
        /// Variable name for the appearance
        outer: String,
        /// Internal table variable name
        inner: String,
    },
    /// Index join
    IndexJoin {
        /// Variable name on the side with the index
        indexed_side: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_join_algorithm_variants() {
        let hash = JoinAlgorithm::HashJoin {
            build_side: "a".to_string(),
            probe_side: "b".to_string(),
        };
        let nested = JoinAlgorithm::NestedLoopJoin {
            outer: "a".to_string(),
            inner: "b".to_string(),
        };
        assert_ne!(hash, nested);
        assert!(matches!(
            JoinAlgorithm::IndexJoin {
                indexed_side: "a".to_string()
            },
            JoinAlgorithm::IndexJoin { .. }
        ));
    }
}
