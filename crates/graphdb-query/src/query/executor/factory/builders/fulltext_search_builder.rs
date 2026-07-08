//! Full-Text Search Executor Builder
//!
//! Responsible for creating full-text search related executors.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::core::error::query::QueryError;
use crate::core::types::span::Span;
use crate::query::executor::admin::{
    AlterFulltextIndexExecutor, CreateFulltextIndexConfig, CreateFulltextIndexExecutor,
    DescribeFulltextIndexExecutor, DropFulltextIndexExecutor, ShowFulltextIndexExecutor,
};
use crate::query::executor::base::{ExecutionContext, ExecutorEnum, FulltextManageExecutor};
use crate::query::executor::data_access::{
    FulltextScanConfig, FulltextScanExecutor, FulltextSearchExecutor, FulltextSearchExecutorParams,
    MatchFulltextExecutor,
};
use crate::query::parser::ast::SearchStatement;
use crate::query::planning::plan::core::nodes::base::plan_node_traits::PlanNode;
use crate::query::planning::plan::core::nodes::search::fulltext::data_access::FulltextSearchNode;
use crate::query::planning::plan::core::nodes::{
    AlterFulltextIndexNode, CreateFulltextIndexNode, DescribeFulltextIndexNode,
    DropFulltextIndexNode, FulltextLookupNode, MatchFulltextNode, ShowFulltextIndexNode,
};
use crate::storage::StorageClient;
use crate::sync::SyncManager;

/// Full-Text Search Executor Builder
///
/// Handles the creation of all full-text search related executors.
pub struct FulltextSearchBuilder<S: StorageClient + Send + 'static> {
    _phantom: std::marker::PhantomData<S>,
}

impl<S: StorageClient + Send + 'static> FulltextSearchBuilder<S> {
    /// Create a new full-text search builder.
    pub fn new() -> Self {
        Self {
            _phantom: std::marker::PhantomData,
        }
    }

    /// Build CreateFulltextIndex executor
    pub fn build_create_fulltext_index(
        node: &CreateFulltextIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
        sync_manager: Option<&Arc<SyncManager>>,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let fulltext_manager = sync_manager
            .ok_or_else(|| QueryError::execution("Sync manager not available".to_string()))?
            .fulltext_manager();

        let space_id = node.space_id;

        let executor = CreateFulltextIndexExecutor::new(
            node.id(),
            storage,
            CreateFulltextIndexConfig {
                index_name: node.index_name.clone(),
                schema_name: node.schema_name.clone(),
                fields: node.fields.clone(),
                engine_type: node.engine_type,
                options: node.options.clone(),
                if_not_exists: node.if_not_exists,
                space_id,
            },
            context.expression_context().clone(),
            fulltext_manager,
        );
        Ok(ExecutorEnum::FulltextManage(
            FulltextManageExecutor::Create(executor),
        ))
    }

    /// Build DropFulltextIndex executor
    pub fn build_drop_fulltext_index(
        node: &DropFulltextIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
        sync_manager: Option<&Arc<SyncManager>>,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let fulltext_manager = sync_manager
            .ok_or_else(|| QueryError::execution("Sync manager not available".to_string()))?
            .fulltext_manager();

        let space_id = context.current_space_id().unwrap_or(0);

        let executor = DropFulltextIndexExecutor::new(
            node.id(),
            storage,
            node.index_name.clone(),
            node.if_exists,
            space_id,
            context.expression_context().clone(),
            fulltext_manager,
        );
        Ok(ExecutorEnum::FulltextManage(FulltextManageExecutor::Drop(
            executor,
        )))
    }

    /// Build AlterFulltextIndex executor
    pub fn build_alter_fulltext_index(
        node: &AlterFulltextIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
        sync_manager: Option<&Arc<SyncManager>>,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let fulltext_manager = sync_manager
            .ok_or_else(|| QueryError::execution("Sync manager not available".to_string()))?
            .fulltext_manager();

        let executor = AlterFulltextIndexExecutor::new(
            node.id(),
            storage,
            node.index_name.clone(),
            node.actions.clone(),
            context.expression_context().clone(),
            fulltext_manager,
        );
        Ok(ExecutorEnum::FulltextManage(FulltextManageExecutor::Alter(
            executor,
        )))
    }

    /// Build ShowFulltextIndex executor
    pub fn build_show_fulltext_index(
        node: &ShowFulltextIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
        sync_manager: Option<&Arc<SyncManager>>,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let fulltext_manager = sync_manager
            .ok_or_else(|| QueryError::execution("Sync manager not available".to_string()))?
            .fulltext_manager();

        let executor = ShowFulltextIndexExecutor::new(
            node.id(),
            storage,
            context.expression_context().clone(),
            fulltext_manager,
        );
        Ok(ExecutorEnum::FulltextManage(FulltextManageExecutor::Show(
            executor,
        )))
    }

    /// Build DescribeFulltextIndex executor
    pub fn build_describe_fulltext_index(
        node: &DescribeFulltextIndexNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
        sync_manager: Option<&Arc<SyncManager>>,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let fulltext_manager = sync_manager
            .ok_or_else(|| QueryError::execution("Sync manager not available".to_string()))?
            .fulltext_manager();

        let space_id = context.current_space_id().unwrap_or(0);

        let executor = DescribeFulltextIndexExecutor::new(
            node.id(),
            storage,
            node.index_name.clone(),
            space_id,
            context.expression_context().clone(),
            fulltext_manager,
        );
        Ok(ExecutorEnum::FulltextManage(
            FulltextManageExecutor::Describe(executor),
        ))
    }

    /// Build FulltextSearch executor
    pub fn build_fulltext_search(
        node: &FulltextSearchNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
        sync_manager: Option<&Arc<SyncManager>>,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let statement = SearchStatement {
            span: Span::default(),
            index_name: node.index_name.clone(),
            query: node.query.clone(),
            yield_clause: node.yield_clause.clone(),
            where_clause: node.where_clause.clone(),
            order_clause: node.order_clause.clone(),
            limit: node.limit,
            offset: node.offset,
        };

        let fulltext_manager = sync_manager
            .ok_or_else(|| QueryError::execution("Sync manager not available".to_string()))?
            .fulltext_manager();

        let executor = if !node.tag_name.is_empty() && !node.field_name.is_empty() {
            let params = FulltextSearchExecutorParams {
                id: node.id(),
                statement,
                storage,
                expr_context: context.expression_context().clone(),
                fulltext_manager,
                space_id: node.space_id,
                tag_name: node.tag_name.clone(),
                field_name: node.field_name.clone(),
            };
            FulltextSearchExecutor::with_metadata(params)
        } else {
            FulltextSearchExecutor::new(
                node.id(),
                statement,
                storage,
                context.expression_context().clone(),
                fulltext_manager,
            )
        };
        Ok(ExecutorEnum::FulltextSearch(executor))
    }

    /// Build FulltextLookup executor
    pub fn build_fulltext_lookup(
        node: &FulltextLookupNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
        sync_manager: Option<&Arc<SyncManager>>,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let fulltext_manager = sync_manager
            .ok_or_else(|| QueryError::execution("Sync manager not available".to_string()))?
            .fulltext_manager();

        let executor = FulltextScanExecutor::new(
            node.id(),
            FulltextScanConfig {
                index_name: node.index_name.clone(),
                query: node.query.clone(),
                limit: node.limit,
                space_id: node.space_id,
                tag_name: node.tag_name.clone(),
                field_name: node.field_name.clone(),
            },
            storage,
            context.expression_context().clone(),
            fulltext_manager,
        );
        Ok(ExecutorEnum::FulltextLookup(executor))
    }

    /// Build MatchFulltext executor
    pub fn build_match_fulltext(
        node: &MatchFulltextNode,
        storage: Arc<RwLock<S>>,
        context: &ExecutionContext,
        sync_manager: Option<&Arc<SyncManager>>,
    ) -> Result<ExecutorEnum<S>, QueryError> {
        let fulltext_manager = sync_manager
            .ok_or_else(|| QueryError::execution("Sync manager not available".to_string()))?
            .fulltext_manager();

        let executor = MatchFulltextExecutor::new(
            node.id(),
            storage,
            node.fulltext_condition.clone(),
            node.yield_clause.clone(),
            context.expression_context().clone(),
            fulltext_manager,
        )
        .with_metadata(
            node.space_id,
            node.tag_name.clone(),
            node.field_name.clone(),
        );
        Ok(ExecutorEnum::MatchFulltext(executor))
    }
}

impl<S: StorageClient + 'static> Default for FulltextSearchBuilder<S> {
    fn default() -> Self {
        Self::new()
    }
}
