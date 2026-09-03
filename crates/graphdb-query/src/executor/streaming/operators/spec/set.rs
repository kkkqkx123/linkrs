//! Immutable configuration for set operators.

/// Immutable config for set operators.
#[derive(Debug, Clone)]
pub enum SetSpec {
    Union,
    UnionAll,
    Intersect,
    Except,
    Minus,
}
