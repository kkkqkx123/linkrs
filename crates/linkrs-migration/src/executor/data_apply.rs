//! Row-level migration step application for vertices and edges.

use std::collections::HashMap;
use std::sync::Arc;

use linkrs_core::{Edge, Value, Vertex};

use crate::converter::convert_value;
use crate::plan::MigrationStep;

pub(super) fn apply_step_to_vertex(
    vertex: &Vertex,
    label: &str,
    step: &MigrationStep,
) -> Result<Option<Vertex>, String> {
    let mut v = vertex.clone();
    if v.tag.name != label {
        return Err(format!("vertex {} does not carry tag '{}'", v.vid, label));
    }
    let tag = &mut v.tag;

    match step {
        MigrationStep::RenameColumn { old_name, new_name } => {
            let value = match tag.properties.remove(old_name.as_str()) {
                Some(v) => v,
                None => return Ok(None),
            };
            tag.properties.insert(new_name.as_str().into(), value);
        }
        MigrationStep::ConvertType {
            name,
            from_type: _,
            to_type,
        } => {
            let value = match tag.properties.get(name.as_str()) {
                Some(v) => v.clone(),
                None => return Ok(None),
            };
            let converted = convert_value(&value, to_type).map_err(|e| e.message)?;
            tag.properties.insert(name.as_str().into(), converted);
        }
        MigrationStep::DropColumn { name } => {
            if !tag.properties.contains_key(name.as_str()) {
                return Ok(None);
            }
            tag.properties.remove(name.as_str());
        }
        MigrationStep::SetDefault {
            name,
            default_value,
        } => {
            if tag.properties.contains_key(name.as_str()) {
                return Ok(None);
            }
            tag.properties.insert(
                name.as_str().into(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(linkrs_core::value::null::NullType::Null)),
            );
        }
        MigrationStep::ChangeNullability {
            name,
            was_nullable,
            now_nullable,
        } => {
            if *was_nullable && !now_nullable {
                for (prop_name, val) in tag.properties.iter() {
                    if &**prop_name == name.as_str() && matches!(val, Value::Null(_)) {
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
            if tag.properties.contains_key(name.as_str()) {
                return Ok(None);
            }
            if let Some(default) = default_value {
                if let Some(current) = tag.properties.get(name.as_str()) {
                    if current == default {
                        return Ok(None);
                    }
                }
            }
            tag.properties.insert(
                name.as_str().into(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(linkrs_core::value::null::NullType::Null)),
            );
        }
        MigrationStep::CreateLabel { .. }
        | MigrationStep::DropLabel { .. }
        | MigrationStep::CreateEdgeType { .. }
        | MigrationStep::DropEdgeType { .. } => return Ok(None),
    }

    Ok(Some(v))
}

pub(super) fn apply_step_to_edge(
    edge: &Edge,
    step: &MigrationStep,
) -> Result<HashMap<Arc<str>, Value>, String> {
    match step {
        MigrationStep::RenameColumn { old_name, new_name } => {
            let value = match edge.props.get(old_name.as_str()) {
                Some(v) => v.clone(),
                None => return Err(format!("Property '{}' not found on edge", old_name)),
            };
            let mut props = edge.props.clone();
            props.remove(old_name.as_str());
            props.insert(new_name.as_str().into(), value);
            Ok(props)
        }
        MigrationStep::ConvertType {
            name,
            from_type: _,
            to_type,
        } => {
            let value = match edge.props.get(name.as_str()) {
                Some(v) => v,
                None => return Err(format!("Property '{}' not found on edge", name)),
            };
            let converted = convert_value(value, to_type).map_err(|e| e.message)?;
            let mut props = edge.props.clone();
            props.insert(name.as_str().into(), converted);
            Ok(props)
        }
        MigrationStep::DropColumn { name } => {
            let mut props = edge.props.clone();
            props.remove(name.as_str());
            Ok(props)
        }
        MigrationStep::SetDefault {
            name,
            default_value,
        } => {
            if edge.props.contains_key(name.as_str()) {
                return Ok(edge.props.clone());
            }
            let mut props = edge.props.clone();
            props.insert(
                name.as_str().into(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(linkrs_core::value::null::NullType::Null)),
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
                    if &**prop_name == name.as_str() && matches!(val, Value::Null(_)) {
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
            if edge.props.contains_key(name.as_str()) {
                return Ok(edge.props.clone());
            }
            let mut props = edge.props.clone();
            props.insert(
                name.as_str().into(),
                default_value
                    .clone()
                    .unwrap_or(Value::Null(linkrs_core::value::null::NullType::Null)),
            );
            Ok(props)
        }
        MigrationStep::CreateLabel { .. }
        | MigrationStep::DropLabel { .. }
        | MigrationStep::CreateEdgeType { .. }
        | MigrationStep::DropEdgeType { .. } => Ok(edge.props.clone()),
    }
}
