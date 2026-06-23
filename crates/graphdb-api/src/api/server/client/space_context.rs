use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::types::SpaceSummary;

#[derive(Debug)]
pub struct SpaceContext {
    space: Arc<RwLock<Option<SpaceSummary>>>,
}

impl Default for SpaceContext {
    fn default() -> Self {
        Self::new()
    }
}

impl SpaceContext {
    pub fn new() -> Self {
        Self {
            space: Arc::new(RwLock::new(None)),
        }
    }

    pub fn space(&self) -> Option<SpaceSummary> {
        self.space.read().clone()
    }

    pub fn set_space(&self, space: SpaceSummary) {
        *self.space.write() = Some(space);
    }

    pub fn clear_space(&self) {
        *self.space.write() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::DataType;

    #[test]
    fn test_space_context() {
        let context = SpaceContext::new();
        assert!(context.space().is_none());

        let space_info = SpaceSummary::new(456, "test_space".to_string(), DataType::BigInt);
        context.set_space(space_info.clone());

        let space = context.space().expect("space should exist");
        assert_eq!(space.id, 456);
        assert_eq!(space.name, "test_space");

        context.clear_space();
        assert!(context.space().is_none());
    }
}
