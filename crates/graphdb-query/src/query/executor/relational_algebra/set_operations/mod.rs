//! Set Operation Executor Module
//!
//! Include all executors related to set operations, including:
//! Union (union set, with duplicates removed)
//! UnionAll (Union; duplicates are retained)
//! Intersect (intersection)
//! “Minus/Except” refers to the mathematical concept of the difference set (also known as the set difference). In set theory, the difference set of two sets A and B, denoted by A ∆ B, consists of all elements that are in set A but not in set B. In other words, it contains the elements that are common to both sets A and B (i.e., A ∩ B) and the elements that are only in set A.

// Basic Set Operations Executor
pub mod base;
pub use base::SetExecutor;

// Union operations (union, deduplication)
pub mod union;
pub use union::UnionExecutor;

// The UnionAll operation (union, with duplicates retained)
pub mod union_all;
pub use union_all::UnionAllExecutor;

// The Intersect operation
pub mod intersect;
pub use intersect::IntersectExecutor;

// The “Minus” operation (difference set)
pub mod minus;
pub use minus::MinusExecutor;
