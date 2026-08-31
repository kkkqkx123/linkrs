use graphdb_core::StorageResult;
use graphdb_storage::{ChangeDetails, PropertyChange, StorageReader};

use crate::error::MigrationError;
use crate::plan::{MigrationPlan, MigrationStep, MigrationTarget, SafetyLevel, VersionRange};

pub fn generate_vertex_plan<R: StorageReader + ?Sized>(
    reader: &R,
    space: &str,
    tag: &str,
    from_version: u64,
    to_version: u64,
) -> Result<MigrationPlan, MigrationError> {
    let changes = reader.get_vertex_schema_changes(space, tag, from_version, to_version)?;

    let steps: Vec<MigrationStep> = changes
        .iter()
        .map(step_from_change)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();

    let overall_safety = calculate_safety(&steps);
    let estimated_rows = estimate_vertex_rows(reader, space, tag).unwrap_or(0);

    let target = MigrationTarget {
        space: space.to_string(),
        label: tag.to_string(),
        is_edge: false,
    };
    let version_range = VersionRange {
        from: from_version,
        to: to_version,
    };

    let rollback_plan = if overall_safety != SafetyLevel::Dangerous {
        let rollback_steps: Vec<MigrationStep> = steps.iter().filter_map(|s| s.reverse()).collect();
        if rollback_steps.is_empty() {
            None
        } else {
            let safety = calculate_safety(&rollback_steps);
            Some(Box::new(MigrationPlan::new(
                target.clone(),
                version_range.clone(),
                rollback_steps,
                estimated_rows,
                safety,
                None,
            )))
        }
    } else {
        None
    };

    Ok(MigrationPlan::new(
        target,
        version_range,
        steps,
        estimated_rows,
        overall_safety,
        rollback_plan,
    ))
}

pub fn generate_edge_plan<R: StorageReader + ?Sized>(
    reader: &R,
    space: &str,
    edge_type: &str,
    from_version: u64,
    to_version: u64,
) -> Result<MigrationPlan, MigrationError> {
    let changes = reader.get_edge_schema_changes(space, edge_type, from_version, to_version)?;

    let steps: Vec<MigrationStep> = changes
        .iter()
        .map(step_from_change)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();

    let overall_safety = calculate_safety(&steps);
    let estimated_rows = estimate_edge_rows(reader, space, edge_type).unwrap_or(0);

    let target = MigrationTarget {
        space: space.to_string(),
        label: edge_type.to_string(),
        is_edge: true,
    };
    let version_range = VersionRange {
        from: from_version,
        to: to_version,
    };

    let rollback_plan = if overall_safety != SafetyLevel::Dangerous {
        let rollback_steps: Vec<MigrationStep> = steps.iter().filter_map(|s| s.reverse()).collect();
        if rollback_steps.is_empty() {
            None
        } else {
            let safety = calculate_safety(&rollback_steps);
            Some(Box::new(MigrationPlan::new(
                target.clone(),
                version_range.clone(),
                rollback_steps,
                estimated_rows,
                safety,
                None,
            )))
        }
    } else {
        None
    };

    Ok(MigrationPlan::new(
        target,
        version_range,
        steps,
        estimated_rows,
        overall_safety,
        rollback_plan,
    ))
}

pub fn generate_vertex_plan_with_expand<R: StorageReader + ?Sized>(
    reader: &R,
    space: &str,
    tag: &str,
    from_version: u64,
    to_version: u64,
    expand_contract: bool,
) -> Result<MigrationPlan, MigrationError> {
    let changes = reader.get_vertex_schema_changes(space, tag, from_version, to_version)?;
    let steps: Vec<MigrationStep> = changes
        .iter()
        .map(|c| step_from_change_with_expand(c, expand_contract))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    let overall_safety = calculate_safety(&steps);
    let estimated_rows = estimate_vertex_rows(reader, space, tag).unwrap_or(0);
    let target = MigrationTarget {
        space: space.to_string(),
        label: tag.to_string(),
        is_edge: false,
    };
    let version_range = VersionRange {
        from: from_version,
        to: to_version,
    };
    let rollback_plan = if overall_safety != SafetyLevel::Dangerous {
        let rollback_steps: Vec<MigrationStep> = steps.iter().filter_map(|s| s.reverse()).collect();
        if rollback_steps.is_empty() {
            None
        } else {
            let safety = calculate_safety(&rollback_steps);
            Some(Box::new(MigrationPlan::new(
                target.clone(),
                version_range.clone(),
                rollback_steps,
                estimated_rows,
                safety,
                None,
            )))
        }
    } else {
        None
    };
    let mut plan = MigrationPlan::new(target, version_range, steps, estimated_rows, overall_safety, rollback_plan);
    plan.expand_contract = Some(expand_contract);
    plan.refresh_hash();
    Ok(plan)
}

pub fn generate_edge_plan_with_expand<R: StorageReader + ?Sized>(
    reader: &R,
    space: &str,
    edge_type: &str,
    from_version: u64,
    to_version: u64,
    expand_contract: bool,
) -> Result<MigrationPlan, MigrationError> {
    let changes = reader.get_edge_schema_changes(space, edge_type, from_version, to_version)?;
    let steps: Vec<MigrationStep> = changes
        .iter()
        .map(|c| step_from_change_with_expand(c, expand_contract))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    let overall_safety = calculate_safety(&steps);
    let estimated_rows = estimate_edge_rows(reader, space, edge_type).unwrap_or(0);
    let target = MigrationTarget {
        space: space.to_string(),
        label: edge_type.to_string(),
        is_edge: true,
    };
    let version_range = VersionRange {
        from: from_version,
        to: to_version,
    };
    let rollback_plan = if overall_safety != SafetyLevel::Dangerous {
        let rollback_steps: Vec<MigrationStep> = steps.iter().filter_map(|s| s.reverse()).collect();
        if rollback_steps.is_empty() {
            None
        } else {
            let safety = calculate_safety(&rollback_steps);
            Some(Box::new(MigrationPlan::new(
                target.clone(),
                version_range.clone(),
                rollback_steps,
                estimated_rows,
                safety,
                None,
            )))
        }
    } else {
        None
    };
    let mut plan = MigrationPlan::new(target, version_range, steps, estimated_rows, overall_safety, rollback_plan);
    plan.expand_contract = Some(expand_contract);
    plan.refresh_hash();
    Ok(plan)
}

fn estimate_vertex_rows<R: StorageReader + ?Sized>(
    reader: &R,
    space: &str,
    tag: &str,
) -> StorageResult<u64> {
    reader.count_vertices_by_tag(space, tag)
}

fn estimate_edge_rows<R: StorageReader + ?Sized>(
    reader: &R,
    space: &str,
    edge_type: &str,
) -> StorageResult<u64> {
    reader.count_edges_by_type(space, edge_type)
}

fn step_from_change_with_expand(
    change: &PropertyChange,
    expand_contract: bool,
) -> Result<Vec<MigrationStep>, MigrationError> {
    match &change.details {
        ChangeDetails::PropertyAdded {
            name,
            data_type,
            nullable,
            default_value,
        } => Ok(vec![MigrationStep::AddColumn {
            name: name.clone(),
            data_type: data_type.clone(),
            nullable: *nullable,
            default_value: default_value.clone(),
        }]),
        ChangeDetails::PropertyRemoved { name, data_type: _ } => {
            Ok(vec![MigrationStep::DropColumn { name: name.clone() }])
        }
        ChangeDetails::PropertyRenamed { old_name, new_name } => {
            if expand_contract {
                // Expand-contract: Add new column, copy data, then drop old.
                // Represented as AddColumn (new) + DropColumn (old) with implicit copy via executor.
                // For now generate AddColumn (new, String) and later executor will copy.
                Ok(vec![
                    MigrationStep::AddColumn {
                        name: new_name.clone(),
                        data_type: graphdb_core::DataType::String,
                        nullable: true,
                        default_value: None,
                    },
                    MigrationStep::RenameColumn {
                        old_name: old_name.clone(),
                        new_name: new_name.clone(),
                    },
                    MigrationStep::DropColumn {
                        name: old_name.clone(),
                    },
                ])
            } else {
                Ok(vec![MigrationStep::RenameColumn {
                    old_name: old_name.clone(),
                    new_name: new_name.clone(),
                }])
            }
        }
        ChangeDetails::PropertyTypeModified {
            name,
            old_type,
            new_type,
        } => Ok(vec![MigrationStep::ConvertType {
            name: name.clone(),
            from_type: old_type.clone(),
            to_type: new_type.clone(),
        }]),
        ChangeDetails::PropertyNullabilityChanged {
            name,
            was_nullable,
            now_nullable,
        } => Ok(vec![MigrationStep::ChangeNullability {
            name: name.clone(),
            was_nullable: *was_nullable,
            now_nullable: *now_nullable,
        }]),
        ChangeDetails::PropertyDefaultValueChanged {
            name,
            old_default: _,
            new_default,
        } => Ok(vec![MigrationStep::SetDefault {
            name: name.clone(),
            default_value: new_default.clone(),
        }]),
        // A primary key change cannot be expressed as a property-level
        // migration step; silently ignoring it would leave the target schema
        // inconsistent with the declared change, so surface an explicit error.
        ChangeDetails::PrimaryKeyChanged {
            old_property,
            new_property,
        } => Err(MigrationError::Plan(format!(
            "Primary key changes are not supported by migration plans ('{}' -> '{}')",
            old_property, new_property
        ))),
    }
}

fn step_from_change(change: &PropertyChange) -> Result<Vec<MigrationStep>, MigrationError> {
    step_from_change_with_expand(change, false)
}

fn calculate_safety(steps: &[MigrationStep]) -> SafetyLevel {
    let mut has_dangerous = false;
    for step in steps {
        match step.safety_level() {
            SafetyLevel::Dangerous => has_dangerous = true,
            SafetyLevel::Warning => {}
            SafetyLevel::Safe => {}
        }
    }
    if has_dangerous {
        SafetyLevel::Dangerous
    } else if steps
        .iter()
        .any(|s| s.safety_level() == SafetyLevel::Warning)
    {
        SafetyLevel::Warning
    } else {
        SafetyLevel::Safe
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use graphdb_core::{DataType, Value};
    use graphdb_storage::{LabelVersionHistory, StorageReader};
    use graphdb_core::types::{EdgeTypeInfo, Index, SpaceInfo, TagInfo, VertexId};
    use graphdb_core::{Edge, EdgeDirection, StorageError, Vertex};
    use std::collections::HashMap;

    #[derive(Debug)]
    struct MockReader {
        vertex_changes: HashMap<(String, String, u64, u64), Vec<PropertyChange>>,
        edge_changes: HashMap<(String, String, u64, u64), Vec<PropertyChange>>,
        count_vertices: u64,
        count_edges: u64,
    }

    impl MockReader {
        fn new() -> Self {
            Self {
                vertex_changes: HashMap::new(),
                edge_changes: HashMap::new(),
                count_vertices: 10,
                count_edges: 5,
            }
        }
        fn with_vertex_change(mut self, space: &str, tag: &str, from: u64, to: u64, details: ChangeDetails) -> Self {
            let key = (space.to_string(), tag.to_string(), from, to);
            let pc = PropertyChange {
                version: to,
                timestamp_ms: 0,
                details,
            };
            self.vertex_changes.entry(key).or_default().push(pc);
            self
        }
        fn with_edge_change(mut self, space: &str, edge: &str, from: u64, to: u64, details: ChangeDetails) -> Self {
            let key = (space.to_string(), edge.to_string(), from, to);
            let pc = PropertyChange {
                version: to,
                timestamp_ms: 0,
                details,
            };
            self.edge_changes.entry(key).or_default().push(pc);
            self
        }
    }

    impl StorageReader for MockReader {
        fn get_vertex(&self, _space: &str, _id: &VertexId) -> Result<Option<Vertex>, StorageError> { Ok(None) }
        fn scan_vertices(&self, _space: &str) -> Result<Vec<Vertex>, StorageError> { Ok(Vec::new()) }
        fn scan_vertices_by_tag(&self, _space: &str, _tag: &str) -> Result<Vec<Vertex>, StorageError> { Ok(Vec::new()) }
        fn scan_vertices_by_prop(&self, _space: &str, _tag: &str, _prop: &str, _value: &Value) -> Result<Vec<Vertex>, StorageError> { Ok(Vec::new()) }
        fn get_edge(&self, _space: &str, _src: &VertexId, _dst: &VertexId, _edge_type: &str, _rank: i64) -> Result<Option<Edge>, StorageError> { Ok(None) }
        fn get_node_edges(&self, _space: &str, _node_id: &VertexId, _direction: EdgeDirection) -> Result<Vec<Edge>, StorageError> { Ok(Vec::new()) }
        fn neighbor_dst_ids_batch(&self, _space: &str, _src_ids: &[VertexId], _direction: EdgeDirection, _edge_types: &[String]) -> Result<Vec<Vec<VertexId>>, StorageError> { Ok(Vec::new()) }
        fn out_degree_batch(&self, _space: &str, _src_ids: &[VertexId], _direction: EdgeDirection, _edge_types: &[String]) -> Result<Vec<usize>, StorageError> { Ok(Vec::new()) }
        fn scan_edges_by_type(&self, _space: &str, _edge_type: &str) -> Result<Vec<Edge>, StorageError> { Ok(Vec::new()) }
        fn scan_all_edges(&self, _space: &str) -> Result<Vec<Edge>, StorageError> { Ok(Vec::new()) }
        fn count_vertices_by_tag(&self, _space: &str, _tag: &str) -> Result<u64, StorageError> { Ok(self.count_vertices) }
        fn count_edges_by_type(&self, _space: &str, _edge_type: &str) -> Result<u64, StorageError> { Ok(self.count_edges) }
        fn lookup_index(&self, _space: &str, _index: &str, _value: &Value) -> Result<Vec<Value>, StorageError> { Ok(Vec::new()) }
        fn get_vertex_with_schema(&self, _space: &str, _tag: &str, _id: &Value) -> Result<Option<(TagInfo, Vec<u8>)>, StorageError> { Ok(None) }
        fn get_edge_with_schema(&self, _space: &str, _edge_type: &str, _src: &Value, _dst: &Value) -> Result<Option<(EdgeTypeInfo, Vec<u8>)>, StorageError> { Ok(None) }
        fn scan_vertices_with_schema(&self, _space: &str, _tag: &str) -> Result<Vec<(TagInfo, Vec<u8>)>, StorageError> { Ok(Vec::new()) }
        fn scan_edges_with_schema(&self, _space: &str, _edge_type: &str) -> Result<Vec<(EdgeTypeInfo, Vec<u8>)>, StorageError> { Ok(Vec::new()) }
        fn get_space(&self, _space: &str) -> Result<Option<SpaceInfo>, StorageError> { Ok(None) }
        fn get_space_by_id(&self, _space_id: u64) -> Result<Option<SpaceInfo>, StorageError> { Ok(None) }
        fn list_spaces(&self) -> Result<Vec<SpaceInfo>, StorageError> { Ok(Vec::new()) }
        fn get_space_id(&self, _space: &str) -> Result<u64, StorageError> { Ok(1) }
        fn space_exists(&self, _space: &str) -> bool { false }
        fn get_tag(&self, _space: &str, _tag: &str) -> Result<Option<TagInfo>, StorageError> { Ok(None) }
        fn list_tags(&self, _space: &str) -> Result<Vec<TagInfo>, StorageError> { Ok(Vec::new()) }
        fn get_edge_type(&self, _space: &str, _edge_type: &str) -> Result<Option<EdgeTypeInfo>, StorageError> { Ok(None) }
        fn list_edge_types(&self, _space: &str) -> Result<Vec<EdgeTypeInfo>, StorageError> { Ok(Vec::new()) }
        fn get_tag_index(&self, _space: &str, _index: &str) -> Result<Option<Index>, StorageError> { Ok(None) }
        fn list_tag_indexes(&self, _space: &str) -> Result<Vec<Index>, StorageError> { Ok(Vec::new()) }
        fn get_edge_index(&self, _space: &str, _index: &str) -> Result<Option<Index>, StorageError> { Ok(None) }
        fn list_edge_indexes(&self, _space: &str) -> Result<Vec<Index>, StorageError> { Ok(Vec::new()) }
        fn get_vertex_version_history(&self, _space: &str, _tag: &str) -> Result<Option<LabelVersionHistory>, StorageError> { Ok(None) }
        fn get_edge_version_history(&self, _space: &str, _edge_type: &str) -> Result<Option<LabelVersionHistory>, StorageError> { Ok(None) }
        fn get_vertex_schema_changes(&self, space: &str, tag: &str, from_version: u64, to_version: u64) -> Result<Vec<PropertyChange>, StorageError> {
            let key = (space.to_string(), tag.to_string(), from_version, to_version);
            Ok(self.vertex_changes.get(&key).cloned().unwrap_or_default())
        }
        fn get_edge_schema_changes(&self, space: &str, edge_type: &str, from_version: u64, to_version: u64) -> Result<Vec<PropertyChange>, StorageError> {
            let key = (space.to_string(), edge_type.to_string(), from_version, to_version);
            Ok(self.edge_changes.get(&key).cloned().unwrap_or_default())
        }
        fn detect_vertex_breaking_changes(&self, _space: &str, _tag: &str, _from_version: u64, _to_version: u64) -> Result<Vec<PropertyChange>, StorageError> { Ok(Vec::new()) }
        fn detect_edge_breaking_changes(&self, _space: &str, _edge_type: &str, _from_version: u64, _to_version: u64) -> Result<Vec<PropertyChange>, StorageError> { Ok(Vec::new()) }
    }

    #[test]
    fn test_generate_add_column_plan() {
        let reader = MockReader::new().with_vertex_change("s", "User", 1, 2, ChangeDetails::PropertyAdded {
            name: "email".into(),
            data_type: DataType::String,
            nullable: true,
            default_value: None,
        });
        let plan = generate_vertex_plan(&reader, "s", "User", 1, 2).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert!(matches!(plan.steps[0], MigrationStep::AddColumn { ref name, .. } if name == "email"));
        assert_eq!(plan.overall_safety, SafetyLevel::Safe);
        assert_eq!(plan.estimated_rows, 10);
    }

    #[test]
    fn test_generate_drop_column_plan() {
        let reader = MockReader::new().with_vertex_change("s", "User", 1, 2, ChangeDetails::PropertyRemoved { name: "old".into(), data_type: DataType::String });
        let plan = generate_vertex_plan(&reader, "s", "User", 1, 2).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert!(matches!(plan.steps[0], MigrationStep::DropColumn { ref name } if name == "old"));
        assert_eq!(plan.overall_safety, SafetyLevel::Dangerous);
        assert!(plan.rollback_plan.is_none());
    }

    #[test]
    fn test_generate_rename_plan() {
        let reader = MockReader::new().with_vertex_change("s", "User", 1, 2, ChangeDetails::PropertyRenamed { old_name: "a".into(), new_name: "b".into() });
        let plan = generate_vertex_plan(&reader, "s", "User", 1, 2).unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert!(matches!(plan.steps[0], MigrationStep::RenameColumn { ref old_name, ref new_name } if old_name=="a" && new_name=="b"));
        assert_eq!(plan.overall_safety, SafetyLevel::Warning);
    }

    #[test]
    fn test_generate_expand_contract_rename_plan() {
        let reader = MockReader::new().with_vertex_change("s", "User", 1, 2, ChangeDetails::PropertyRenamed { old_name: "a".into(), new_name: "b".into() });
        let plan = generate_vertex_plan_with_expand(&reader, "s", "User", 1, 2, true).unwrap();
        assert_eq!(plan.steps.len(), 3);
        assert!(matches!(plan.steps[0], MigrationStep::AddColumn { ref name, .. } if name == "b"));
        assert!(matches!(plan.steps[1], MigrationStep::RenameColumn { ref old_name, ref new_name } if old_name == "a" && new_name == "b"));
        assert!(matches!(plan.steps[2], MigrationStep::DropColumn { ref name } if name == "a"));
        assert_eq!(plan.overall_safety, SafetyLevel::Dangerous);
        assert_eq!(plan.expand_contract, Some(true));
    }

    #[test]
    fn test_generate_type_convert_plan() {
        let reader = MockReader::new().with_vertex_change("s", "User", 1, 2, ChangeDetails::PropertyTypeModified { name: "age".into(), old_type: DataType::Int, new_type: DataType::BigInt });
        let plan = generate_vertex_plan(&reader, "s", "User", 1, 2).unwrap();
        assert!(matches!(plan.steps[0], MigrationStep::ConvertType { ref name, .. } if name=="age"));
        assert_eq!(plan.overall_safety, SafetyLevel::Warning);
    }

    #[test]
    fn test_generate_nullability_plan() {
        let reader = MockReader::new().with_vertex_change("s", "User", 1, 2, ChangeDetails::PropertyNullabilityChanged { name: "x".into(), was_nullable: true, now_nullable: false });
        let plan = generate_vertex_plan(&reader, "s", "User", 1, 2).unwrap();
        assert!(matches!(plan.steps[0], MigrationStep::ChangeNullability { ref name, .. } if name=="x"));
    }

    #[test]
    fn test_generate_default_plan() {
        let reader = MockReader::new().with_vertex_change("s", "User", 1, 2, ChangeDetails::PropertyDefaultValueChanged { name: "y".into(), old_default: None, new_default: Some(Value::Int(5)) });
        let plan = generate_vertex_plan(&reader, "s", "User", 1, 2).unwrap();
        assert!(matches!(plan.steps[0], MigrationStep::SetDefault { ref name, .. } if name=="y"));
        assert_eq!(plan.overall_safety, SafetyLevel::Safe);
    }

    #[test]
    fn test_primary_key_changed_error() {
        let reader = MockReader::new().with_vertex_change("s", "User", 1, 2, ChangeDetails::PrimaryKeyChanged { old_property: "id1".into(), new_property: "id2".into() });
        let result = generate_vertex_plan(&reader, "s", "User", 1, 2);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Primary key"));
    }

    #[test]
    fn test_safety_level_calculation_mixed() {
        let reader = MockReader::new()
            .with_vertex_change("s", "User", 1, 2, ChangeDetails::PropertyAdded { name: "a".into(), data_type: DataType::String, nullable: true, default_value: None })
            .with_vertex_change("s", "User", 1, 2, ChangeDetails::PropertyRenamed { old_name: "b".into(), new_name: "c".into() });
        // manually inject second change via second call: need to push both; our mock with_vertex_change for same key will append.
        let plan = generate_vertex_plan(&reader, "s", "User", 1, 2).unwrap();
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(plan.overall_safety, SafetyLevel::Warning);
    }

    #[test]
    fn test_generate_edge_plan() {
        let reader = MockReader::new().with_edge_change("s", "knows", 1, 2, ChangeDetails::PropertyAdded { name: "since".into(), data_type: DataType::String, nullable: true, default_value: None });
        let plan = generate_edge_plan(&reader, "s", "knows", 1, 2).unwrap();
        assert!(plan.target.is_edge);
        assert_eq!(plan.steps.len(), 1);
    }

    #[test]
    fn test_rollback_plan_generated_for_safe() {
        let reader = MockReader::new().with_vertex_change("s", "User", 1, 2, ChangeDetails::PropertyAdded { name: "email".into(), data_type: DataType::String, nullable: true, default_value: None });
        let plan = generate_vertex_plan(&reader, "s", "User", 1, 2).unwrap();
        assert!(plan.rollback_plan.is_some());
        let rollback = plan.rollback_plan.unwrap();
        assert!(matches!(rollback.steps[0], MigrationStep::DropColumn { .. }));
    }

    #[test]
    fn test_no_rollback_for_dangerous() {
        let reader = MockReader::new().with_vertex_change("s", "User", 1, 2, ChangeDetails::PropertyRemoved { name: "old".into(), data_type: DataType::String });
        let plan = generate_vertex_plan(&reader, "s", "User", 1, 2).unwrap();
        assert!(plan.rollback_plan.is_none());
    }
}
