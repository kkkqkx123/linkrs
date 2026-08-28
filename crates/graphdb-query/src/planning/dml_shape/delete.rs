//! DELETE statement shape renderer.

use graphdb_core::Value;
use crate::parser::ast::{DeleteStmt, DeleteTarget};

use super::{render_contextual, ContextualExpression};

/// Render a canonical DELETE template, appending literal values to `values`.
pub(crate) fn render_delete(delete: &DeleteStmt, values: &mut Vec<Value>) -> Option<String> {
    // The parser never produces a WHERE clause for DELETE. If one is present
    // we cannot re-render it faithfully, so skip shape caching.
    if delete.where_clause.is_some() {
        return None;
    }
    let mut out = String::from("DELETE ");
    match &delete.target {
        DeleteTarget::Vertices(vids) => {
            out.push_str("VERTEX ");
            render_expr_list(&mut out, values, vids)?;
        }
        DeleteTarget::Edges { edge_type, edges } => {
            // Canonical syntax 1: DELETE EDGE <edge_type> <src> -> <dst> [@rank].
            let edge_type = edge_type.as_deref()?;
            out.push_str("EDGE ");
            out.push_str(edge_type);
            for (edge_index, (src, dst, rank)) in edges.iter().enumerate() {
                out.push(' ');
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
            }
        }
        DeleteTarget::Tags {
            tag_names,
            vertex_ids,
            is_all_tags,
        } => {
            out.push_str("TAG ");
            if *is_all_tags {
                out.push('*');
            } else {
                out.push_str(&tag_names.join(", "));
            }
            out.push_str(" FROM ");
            render_expr_list(&mut out, values, vertex_ids)?;
        }
        DeleteTarget::Index(name) => {
            out.push_str("INDEX ");
            out.push_str(name);
        }
    }
    if delete.with_edge {
        out.push_str(" WITH EDGE");
    }
    Some(out)
}

/// Render a comma-separated list of expression positions.
fn render_expr_list(
    out: &mut String,
    values: &mut Vec<Value>,
    exprs: &[ContextualExpression],
) -> Option<()> {
    for (index, expr) in exprs.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        render_contextual(out, values, expr)?;
    }
    Some(())
}
