//! Node ID generator
//!
//! Provide a mechanism for the allocation of globally unique plan node IDs.

use std::sync::atomic::{AtomicI64, Ordering};

/// Node ID generator
///
/// Using the singleton pattern to provide a globally unique node ID assignment.
pub struct NodeIdGenerator {
    counter: AtomicI64,
}

impl NodeIdGenerator {
    /// Obtain a global singleton instance.
    pub fn instance() -> &'static Self {
        static INSTANCE: NodeIdGenerator = NodeIdGenerator {
            counter: AtomicI64::new(1), // Starting from 1, 0 is reserved as an invalid ID.
        };
        &INSTANCE
    }

    /// Obtain the next unique ID.
    pub fn next_id(&self) -> i64 {
        self.counter.fetch_add(1, Ordering::SeqCst)
    }

    /// Reset the counter (for testing purposes only)
    #[cfg(test)]
    pub fn reset(&self) {
        self.counter.store(1, Ordering::SeqCst);
    }
}

/// A convenient function for assigning new IDs to nodes
pub fn next_node_id() -> i64 {
    NodeIdGenerator::instance().next_id()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_id_generation() {
        NodeIdGenerator::instance().reset();

        let id1 = next_node_id();
        let id2 = next_node_id();
        let id3 = next_node_id();

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_singleton() {
        NodeIdGenerator::instance().reset();

        let id1 = NodeIdGenerator::instance().next_id();
        let id2 = NodeIdGenerator::instance().next_id();

        assert_eq!(id2, id1 + 1);
    }
}
