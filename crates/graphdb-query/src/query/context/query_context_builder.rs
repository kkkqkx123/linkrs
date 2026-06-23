//! QueryContext Builder
//!
//! Provide a fluent API for building QueryContext, simplifying the process of creating complex configurations.
//!
//! # Example
//!
//! ```rust,ignore
//! use graphdb::query::context::{QueryContextBuilder, QueryRequestContext};
//! use std::sync::Arc;
//!
//! let rctx = Arc::new(QueryRequestContext::new("MATCH (n) RETURN n".to_string()));
//!
//! let query_context = QueryContextBuilder::new(rctx)
//!     .with_space_info(space_info)
//!     .with_charset_info(charset_info)
//!     .with_arena()
//!     .build();
//! ```

use crate::core::types::{CharsetInfo, SpaceInfo, SpaceSummary};
use crate::utils::{Arena, IdGenerator};
use std::sync::Arc;

use super::{QueryContext, QueryExecutionManager, QueryRequestContext};

/// QueryContext Builder
///
/// Provides a fluent API for constructing QueryContext with complex configurations.
///
/// # Example
///
/// ```rust,ignore
/// let rctx = Arc::new(QueryRequestContext::new("MATCH (n) RETURN n".to_string()));
///
/// let query_context = QueryContextBuilder::new(rctx)
///     .with_space_info(space_info)
///     .with_charset_info(charset_info)
///     .with_start_id(100)
///     .with_arena()
///     .build();
/// ```
#[derive(Default)]
pub struct QueryContextBuilder {
    rctx: Option<Arc<QueryRequestContext>>,
    execution_manager: Option<QueryExecutionManager>,
    id_gen: Option<IdGenerator>,
    space_info: Option<SpaceInfo>,
    charset_info: Option<Box<CharsetInfo>>,
    arena: Option<Arena>,
}

impl QueryContextBuilder {
    /// Create a new builder with the required QueryRequestContext.
    pub fn new(rctx: Arc<QueryRequestContext>) -> Self {
        Self {
            rctx: Some(rctx),
            execution_manager: None,
            id_gen: None,
            space_info: None,
            charset_info: None,
            arena: None,
        }
    }

    /// Create a builder from session context.
    ///
    /// This simplifies the common pattern of creating a QueryContext
    /// from a ClientSession, automatically extracting space information.
    pub fn from_session(rctx: Arc<QueryRequestContext>, space: Option<SpaceSummary>) -> Self {
        Self {
            rctx: Some(rctx),
            execution_manager: None,
            id_gen: None,
            space_info: space.map(SpaceInfo::from),
            charset_info: None,
            arena: None,
        }
    }

    /// Set the execution manager.
    pub fn with_execution_manager(mut self, execution_manager: QueryExecutionManager) -> Self {
        self.execution_manager = Some(execution_manager);
        self
    }

    /// Set the space information.
    pub fn with_space_info(mut self, space_info: SpaceInfo) -> Self {
        self.space_info = Some(space_info);
        self
    }

    /// Set the charset information.
    pub fn with_charset_info(mut self, charset_info: CharsetInfo) -> Self {
        self.charset_info = Some(Box::new(charset_info));
        self
    }

    /// Set the initial value for the ID generator.
    pub fn with_start_id(mut self, start_id: i64) -> Self {
        self.id_gen = Some(IdGenerator::new(start_id));
        self
    }

    /// Enable arena allocation with default capacity.
    ///
    /// Arena allocation is beneficial for queries that create many
    /// temporary data structures during execution.
    pub fn with_arena(mut self) -> Self {
        self.arena = Some(Arena::new());
        self
    }

    /// Enable arena allocation with custom capacity.
    pub fn with_arena_capacity(mut self, capacity: usize) -> Self {
        self.arena = Some(Arena::with_capacity(capacity));
        self
    }

    /// Build the QueryContext.
    ///
    /// # Panics
    ///
    /// Panics if QueryRequestContext was not provided.
    pub fn build(self) -> QueryContext {
        let rctx = self.rctx.expect("QueryRequestContext is required");
        let execution_manager = self.execution_manager.unwrap_or_default();
        let id_gen = self.id_gen.unwrap_or_else(|| IdGenerator::new(0));

        QueryContext::from_builder(
            rctx,
            execution_manager,
            id_gen,
            self.space_info,
            self.charset_info,
            self.arena,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::types::{DataType, EngineType, IsolationLevel, MetadataVersion, SpaceStatus};
    use std::collections::HashMap;

    #[test]
    fn test_builder_basic() {
        let rctx = Arc::new(QueryRequestContext {
            session_id: None,
            user_name: None,
            space_name: None,
            query: "MATCH (n) RETURN n".to_string(),
            parameters: HashMap::new(),
        });

        let query_context = QueryContextBuilder::new(rctx).build();

        assert_eq!(query_context.query(), "MATCH (n) RETURN n");
    }

    #[test]
    fn test_builder_with_space_info() {
        let rctx = Arc::new(QueryRequestContext {
            session_id: None,
            user_name: None,
            space_name: None,
            query: "MATCH (n) RETURN n".to_string(),
            parameters: HashMap::new(),
        });

        let space_info = SpaceInfo {
            space_id: 1,
            space_name: "test_space".to_string(),
            vid_type: DataType::BigInt,
            tags: Vec::new(),
            edge_types: Vec::new(),
            version: MetadataVersion::default(),
            comment: None,
            storage_path: None,
            isolation_level: IsolationLevel::default(),
            partition_num: 100,
            replica_factor: 1,
            engine_type: EngineType::default(),
            status: SpaceStatus::Online,
        };

        let query_context = QueryContextBuilder::new(rctx)
            .with_space_info(space_info)
            .build();

        assert_eq!(query_context.space_id(), Some(1));
        assert_eq!(query_context.space_name(), Some("test_space".to_string()));
    }

    #[test]
    fn test_builder_with_start_id() {
        let rctx = Arc::new(QueryRequestContext {
            session_id: None,
            user_name: None,
            space_name: None,
            query: "MATCH (n) RETURN n".to_string(),
            parameters: HashMap::new(),
        });

        let query_context = QueryContextBuilder::new(rctx).with_start_id(100).build();

        assert_eq!(query_context.current_id(), 100);
    }

    #[test]
    fn test_builder_with_arena() {
        let rctx = Arc::new(QueryRequestContext {
            session_id: None,
            user_name: None,
            space_name: None,
            query: "MATCH (n) RETURN n".to_string(),
            parameters: HashMap::new(),
        });

        let query_context = QueryContextBuilder::new(rctx).with_arena().build();

        assert!(query_context.has_arena());
    }

    #[test]
    fn test_builder_chaining() {
        let rctx = Arc::new(QueryRequestContext {
            session_id: None,
            user_name: None,
            space_name: None,
            query: "MATCH (n) RETURN n".to_string(),
            parameters: HashMap::new(),
        });

        let space_info = SpaceInfo {
            space_id: 1,
            space_name: "test_space".to_string(),
            vid_type: DataType::BigInt,
            tags: Vec::new(),
            edge_types: Vec::new(),
            version: MetadataVersion::default(),
            comment: None,
            storage_path: None,
            isolation_level: IsolationLevel::default(),
            partition_num: 100,
            replica_factor: 1,
            engine_type: EngineType::default(),
            status: SpaceStatus::Online,
        };

        let query_context = QueryContextBuilder::new(rctx)
            .with_space_info(space_info)
            .with_start_id(100)
            .with_arena()
            .build();

        assert_eq!(query_context.space_id(), Some(1));
        assert_eq!(query_context.current_id(), 100);
        assert!(query_context.has_arena());
    }
}
