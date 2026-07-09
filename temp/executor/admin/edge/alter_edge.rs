//! AlterEdgeExecutor - Alter Edge Type Executor
//!
//! Responsible for modifying attribute definitions for already existing edge types.

use parking_lot::RwLock;
use std::sync::Arc;

use crate::core::types::PropertyDef;
use crate::core::DataType;
use crate::query::executor::admin::SchemaCompatibilityChecker;
use crate::query::executor::base::{BaseExecutor, ExecutionResult, Executor, HasStorage};
use crate::query::validator::context::ExpressionAnalysisContext;
use crate::storage::{ChangeDetails, PropertyChange, StorageClient};

/// Edge type modification operation type
#[derive(Debug, Clone)]
pub enum AlterEdgeOp {
    Add,
    Drop,
    Change,
}

/// Side Type Modifiers
#[derive(Debug, Clone)]
pub struct AlterEdgeItem {
    pub op: AlterEdgeOp,
    pub property: Option<PropertyDef>,
    pub property_name: Option<String>,
}

impl AlterEdgeItem {
    pub fn add_property(property: PropertyDef) -> Self {
        Self {
            op: AlterEdgeOp::Add,
            property: Some(property),
            property_name: None,
        }
    }

    pub fn drop_property(property_name: String) -> Self {
        Self {
            op: AlterEdgeOp::Drop,
            property: None,
            property_name: Some(property_name),
        }
    }
}

/// Edge type modification information
#[derive(Debug, Clone)]
pub struct AlterEdgeInfo {
    pub space_name: String,
    pub edge_name: String,
    pub items: Vec<AlterEdgeItem>,
    pub comment: Option<String>,
}

impl AlterEdgeInfo {
    pub fn new(space_name: String, edge_name: String) -> Self {
        Self {
            space_name,
            edge_name,
            items: Vec::new(),
            comment: None,
        }
    }

    pub fn with_items(mut self, items: Vec<AlterEdgeItem>) -> Self {
        self.items = items;
        self
    }

    pub fn with_comment(mut self, comment: String) -> Self {
        self.comment = Some(comment);
        self
    }
}

/// Modify edge type actuator
///
/// This executor is responsible for modifying the attribute definitions of already existing edge types.
#[derive(Debug)]
pub struct AlterEdgeExecutor<S: StorageClient> {
    base: BaseExecutor<S>,
    alter_info: AlterEdgeInfo,
}

impl<S: StorageClient> AlterEdgeExecutor<S> {
    /// Creating a new AlterEdgeExecutor
    pub fn new(
        id: i64,
        storage: Arc<RwLock<S>>,
        alter_info: AlterEdgeInfo,
        expr_context: Arc<ExpressionAnalysisContext>,
    ) -> Self {
        Self {
            base: BaseExecutor::new(id, "AlterEdgeExecutor".to_string(), storage, expr_context),
            alter_info,
        }
    }
}

impl<S: StorageClient + Send + Sync + 'static> Executor<S> for AlterEdgeExecutor<S> {
    fn execute(&mut self) -> crate::query::executor::base::DBResult<ExecutionResult> {
        let storage = self.get_storage();

        // Step 1: Compatibility check (read-only, before write lock)
        if !self.alter_info.items.is_empty() {
            let reader = storage.read();
            let edge_type = reader
                .get_edge_type(&self.alter_info.space_name, &self.alter_info.edge_name)
                .ok()
                .flatten();
            if let Some(edge_type) = edge_type {
                let property_changes: Vec<PropertyChange> = self
                    .alter_info
                    .items
                    .iter()
                    .filter_map(|item| {
                        let details = match item.op {
                            AlterEdgeOp::Add => {
                                let prop = item.property.as_ref()?;
                                ChangeDetails::PropertyAdded {
                                    name: prop.name.clone(),
                                    data_type: prop.data_type.clone(),
                                    nullable: prop.nullable,
                                    default_value: prop.default.clone(),
                                }
                            }
                            AlterEdgeOp::Drop => {
                                let name = item.property_name.as_ref()?;
                                let data_type = edge_type
                                    .properties
                                    .iter()
                                    .find(|p| p.name == *name)
                                    .map(|p| p.data_type.clone())
                                    .unwrap_or(DataType::String);
                                ChangeDetails::PropertyRemoved {
                                    name: name.clone(),
                                    data_type,
                                }
                            }
                            AlterEdgeOp::Change => {
                                let old_name = item.property_name.as_ref()?;
                                let new_name = item.property.as_ref().map(|p| p.name.clone())?;
                                ChangeDetails::PropertyRenamed {
                                    old_name: old_name.clone(),
                                    new_name,
                                }
                            }
                        };
                        Some(PropertyChange {
                            version: 0,
                            timestamp_ms: 0,
                            details,
                        })
                    })
                    .collect();

                if !property_changes.is_empty() {
                    if let Ok(report) = SchemaCompatibilityChecker::edge_compatibility(
                        &*reader,
                        &self.alter_info.space_name,
                        &self.alter_info.edge_name,
                        &property_changes,
                    ) {
                        if report.has_breaking_changes {
                            log::warn!(
                                "Breaking schema changes for edge type '{}': {:?}",
                                self.alter_info.edge_name,
                                report.breaking_changes
                            );
                        }
                        if !report.warnings.is_empty() {
                            log::warn!(
                                "Schema change warnings for edge type '{}': {:?}",
                                self.alter_info.edge_name,
                                report.warnings
                            );
                        }
                    }
                }
            }
        }

        // Step 2: Acquire write lock and perform the alter
        let mut storage_guard = storage.write();

        let additions: Vec<PropertyDef> = self
            .alter_info
            .items
            .iter()
            .filter_map(|item| match item.op {
                AlterEdgeOp::Add => item.property.clone(),
                _ => None,
            })
            .collect();

        let deletions: Vec<String> = self
            .alter_info
            .items
            .iter()
            .filter_map(|item| match item.op {
                AlterEdgeOp::Drop => item.property_name.clone(),
                AlterEdgeOp::Change => item.property_name.clone(),
                _ => None,
            })
            .collect();

        if !deletions.is_empty() {
            let edge_info = storage_guard
                .get_edge_type(&self.alter_info.space_name, &self.alter_info.edge_name);
            if let Ok(Some(edge)) = edge_info {
                for del_name in &deletions {
                    if !edge.properties.iter().any(|p| &p.name == del_name) {
                        return Ok(ExecutionResult::Error(format!(
                            "Property '{}' not found in edge type '{}'",
                            del_name, self.alter_info.edge_name
                        )));
                    }
                }
            }
        }

        let result = storage_guard.alter_edge_type(
            &self.alter_info.space_name,
            &self.alter_info.edge_name,
            additions,
            deletions,
        );

        match result {
            Ok(true) => Ok(ExecutionResult::Success),
            Ok(false) => Ok(ExecutionResult::Error(format!(
                "Edge type '{}' not found in space '{}'",
                self.alter_info.edge_name, self.alter_info.space_name
            ))),
            Err(e) => Ok(ExecutionResult::Error(format!(
                "Failed to alter edge type: {}",
                e
            ))),
        }
    }

    fn open(&mut self) -> crate::query::executor::base::DBResult<()> {
        self.base.open()
    }

    fn close(&mut self) -> crate::query::executor::base::DBResult<()> {
        self.base.close()
    }

    fn is_open(&self) -> bool {
        self.base.is_open()
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        "AlterEdgeExecutor"
    }

    fn description(&self) -> &str {
        "Alters an edge type"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageClient> crate::query::executor::base::HasStorage<S> for AlterEdgeExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
