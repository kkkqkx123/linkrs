//! Row-level migration step application for vertices and edges.

use std::collections::HashMap;

use graphdb_core::{Edge, Tag, Value, Vertex};

use crate::converter::convert_value;
use crate::plan::MigrationStep;

pub(super) fn apply_step_to_vertex(
    vertex: &Vertex,
    label: &str,
    step: &MigrationStep,
) -> Result<Option<Vertex>, String> {
    let mut v = vertex.clone();
    let tag = match v.tags.iter_mut().find(|t| t.name == label) {
        Some(t) => t,
        None => return Ok(None),
    };

    match step {
        MigrationStep::RenameColumn { old_name, new_name } => {
            let value = match tag.properties.remove(old_name) {
                Some(v) => v,
                None => return Ok(None),
            };
            tag.properties.insert(new_name.clone(), value);
        }
        MigrationStep::ConvertType {
            name,
            from_type: _,
            to_type,
        } => {
            let value = match tag.properties.get(name) {
                Some(v) => v.clone(),
                None => return Ok(None),
            };
            let converted = convert_value(&value, to_type).map_err(|e| e.message)?;
            tag.properties.insert(name.clone(), converted);
        }
        MigrationStep::DropColumn { name } => {
            if !tag.properties.contains_key(name) {
                return Ok(None);
            }
            tag.properties.remove(name);
        }
        MigrationStep::SetDefault {
            name,
            default_value,
        } => {
            if tag.properties.contains_key(name) {
                return Ok(None);
            }
            tag.properties.insert(
                name.clone(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(graphdb_core::value::null::NullType::Null)),
            );
        }
        MigrationStep::ChangeNullability {
            name,
            was_nullable,
            now_nullable,
        } => {
            if *was_nullable && !now_nullable {
                for (prop_name, val) in tag.properties.iter() {
                    if prop_name == name && matches!(val, Value::Null(_)) {
                        return Err(format!(
                            "cannot set column '{}' NOT NULL: found NULL values",
                            name
                        ));
                    }
                }
            }
            return Ok(None);
        }
        MigrationStep::AddColumn {
            name,
            data_type: _,
            nullable: _,
            default_value,
        } => {
            if tag.properties.contains_key(name) {
                return Ok(None);
            }
            if let Some(default) = default_value {
                if tag.properties.contains_key(name) {
                    let current = tag.properties.get(name).unwrap();
                    if current == default {
                        return Ok(None);
                    }
                }
            }
            tag.properties.insert(
                name.clone(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(graphdb_core::value::null::NullType::Null)),
            );
        }
        MigrationStep::CreateLabel { .. }
        | MigrationStep::DropLabel { .. }
        | MigrationStep::CreateEdgeType { .. }
        | MigrationStep::DropEdgeType { .. } => return Ok(None),
    }

    v.properties = merge_vertex_properties(&v.tags);
    Ok(Some(v))
}

pub(super) fn apply_step_to_edge(
    edge: &Edge,
    step: &MigrationStep,
) -> Result<HashMap<String, Value>, String> {
    match step {
        MigrationStep::RenameColumn { old_name, new_name } => {
            let value = match edge.props.get(old_name) {
                Some(v) => v.clone(),
                None => return Err(format!("Property '{}' not found on edge", old_name)),
            };
            let mut props = edge.props.clone();
            props.remove(old_name);
            props.insert(new_name.clone(), value);
            Ok(props)
        }
        MigrationStep::ConvertType {
            name,
            from_type: _,
            to_type,
        } => {
            let value = match edge.props.get(name) {
                Some(v) => v,
                None => return Err(format!("Property '{}' not found on edge", name)),
            };
            let converted = convert_value(value, to_type).map_err(|e| e.message)?;
            let mut props = edge.props.clone();
            props.insert(name.clone(), converted);
            Ok(props)
        }
        MigrationStep::DropColumn { name } => {
            let mut props = edge.props.clone();
            props.remove(name);
            Ok(props)
        }
        MigrationStep::SetDefault {
            name,
            default_value,
        } => {
            if edge.props.contains_key(name) {
                return Ok(edge.props.clone());
            }
            let mut props = edge.props.clone();
            props.insert(
                name.clone(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(graphdb_core::value::null::NullType::Null)),
            );
            Ok(props)
        }
        MigrationStep::ChangeNullability {
            name,
            was_nullable,
            now_nullable,
        } => {
            if *was_nullable && !now_nullable {
                for (prop_name, val) in edge.props.iter() {
                    if prop_name == name && matches!(val, Value::Null(_)) {
                        return Err(format!(
                            "cannot set column '{}' NOT NULL: found NULL values",
                            name
                        ));
                    }
                }
            }
            Ok(edge.props.clone())
        }
        MigrationStep::AddColumn {
            name,
            data_type: _,
            nullable: _,
            default_value,
        } => {
            if edge.props.contains_key(name) {
                return Ok(edge.props.clone());
            }
            let mut props = edge.props.clone();
            props.insert(
                name.clone(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(graphdb_core::value::null::NullType::Null)),
            );
            Ok(props)
        }
        MigrationStep::CreateLabel { .. }
        | MigrationStep::DropLabel { .. }
        | MigrationStep::CreateEdgeType { .. }
        | MigrationStep::DropEdgeType { .. } => Ok(edge.props.clone()),
    }
}

pub(super) fn merge_vertex_properties(tags: &[Tag]) -> HashMap<String, Value> {
    let mut merged = HashMap::new();
    for tag in tags {
        for (k, v) in &tag.properties {
            merged.insert(k.clone(), v.clone());
        }
    }
    merged
}
