//! INSERT statement shape renderer.

use crate::parser::ast::{InsertStmt, InsertTarget};
use graphdb_core::Value;

use super::render_contextual;

/// Render a canonical INSERT template, appending literal values to `values`.
pub(crate) fn render_insert(insert: &InsertStmt, values: &mut Vec<Value>) -> Option<String> {
    let mut out = String::from("INSERT ");
    match &insert.target {
        InsertTarget::Vertices { tags, values: rows } => {
            out.push_str("VERTEX ");
            if insert.if_not_exists {
                out.push_str("IF NOT EXISTS ");
            }
            for (index, tag) in tags.iter().enumerate() {
                if index > 0 {
                    out.push_str(", ");
                }
                out.push_str(&tag.tag_name);
                if !tag.prop_names.is_empty() {
                    out.push('(');
                    out.push_str(&tag.prop_names.join(", "));
                    out.push(')');
                }
            }
            out.push_str(" VALUES ");
            for (row_index, row) in rows.iter().enumerate() {
                if row_index > 0 {
                    out.push_str(", ");
                }
                render_contextual(&mut out, values, &row.vid)?;
                out.push_str(": ");
                for (group_index, group) in row.tag_values.iter().enumerate() {
                    if group_index > 0 {
                        out.push_str(": ");
                    }
                    out.push('(');
                    for (value_index, value) in group.iter().enumerate() {
                        if value_index > 0 {
                            out.push_str(", ");
                        }
                        render_contextual(&mut out, values, value)?;
                    }
                    out.push(')');
                }
            }
        }
        InsertTarget::Edge {
            edge_name,
            prop_names,
            edges,
        } => {
            out.push_str("EDGE ");
            if insert.if_not_exists {
                out.push_str("IF NOT EXISTS ");
            }
            out.push_str(edge_name);
            if !prop_names.is_empty() {
                out.push('(');
                out.push_str(&prop_names.join(", "));
                out.push(')');
            }
            out.push_str(" VALUES ");
            for (edge_index, (src, dst, rank, properties)) in edges.iter().enumerate() {
                if edge_index > 0 {
                    out.push_str(", ");
                }
                render_contextual(&mut out, values, src)?;
                out.push_str(" -> ");
                render_contextual(&mut out, values, dst)?;
                if let Some(rank) = rank {
                    out.push_str(" @");
                    render_contextual(&mut out, values, rank)?;
                }
                if !properties.is_empty() {
                    out.push_str(": (");
                    for (value_index, property) in properties.iter().enumerate() {
                        if value_index > 0 {
                            out.push_str(", ");
                        }
                        render_contextual(&mut out, values, property)?;
                    }
                    out.push(')');
                }
            }
        }
    }
    Some(out)
}
