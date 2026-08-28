//! UPDATE / UPSERT statement shape renderer.

use graphdb_core::Value;
use crate::parser::ast::{Assignment, UpdateStmt, UpdateTarget};

use super::render_contextual;

/// Render a canonical UPDATE/UPSERT template, appending literal values.
pub(crate) fn render_update(update: &UpdateStmt, values: &mut Vec<Value>) -> Option<String> {
    // A YIELD clause is not re-rendered here; skip shape caching when present.
    if update.yield_clause.is_some() {
        return None;
    }
    let mut out = String::new();
    out.push_str(if update.is_upsert {
        "UPSERT "
    } else {
        "UPDATE "
    });

    match &update.target {
        UpdateTarget::Vertex(vid) => {
            out.push_str("VERTEX ");
            render_contextual(&mut out, values, vid)?;
        }
        UpdateTarget::Edge {
            src,
            dst,
            edge_type,
            rank,
        } => {
            let edge_type = edge_type.as_deref()?;
            if update.is_upsert {
                // UPSERT EDGE only accepts the short form (src -> dst @rank OF type).
                out.push_str("EDGE ");
                render_contextual(&mut out, values, src)?;
                out.push_str(" -> ");
                render_contextual(&mut out, values, dst)?;
                if let Some(rank) = rank {
                    out.push_str(" @");
                    render_contextual(&mut out, values, rank)?;
                }
                out.push_str(" OF ");
                out.push_str(edge_type);
            } else {
                // Non-upsert uses the deterministic long form
                // (OF <edge_type> FROM <src> TO <dst> [@rank]).
                out.push_str("EDGE OF ");
                out.push_str(edge_type);
                out.push_str(" FROM ");
                render_contextual(&mut out, values, src)?;
                out.push_str(" TO ");
                render_contextual(&mut out, values, dst)?;
                if let Some(rank) = rank {
                    out.push_str(" @");
                    render_contextual(&mut out, values, rank)?;
                }
            }
        }
        UpdateTarget::Tag(_) => return None,
        UpdateTarget::TagOnVertex { vid, tag_name } => {
            // TagOnVertex is only produced by the parser in upsert mode, and
            // only the bare form (UPSERT <vid> ON <tag>) round-trips.
            if !update.is_upsert {
                return None;
            }
            render_contextual(&mut out, values, vid)?;
            out.push_str(" ON ");
            out.push_str(tag_name);
        }
    }

    if !update.set_clause.assignments.is_empty() {
        out.push_str(" SET ");
        render_set_assignments(&mut out, values, &update.set_clause.assignments)?;
    }
    if let Some(where_expr) = &update.where_clause {
        out.push_str(" WHERE ");
        render_contextual(&mut out, values, where_expr)?;
    }
    Some(out)
}

/// Render a `property = value` assignment list.
pub(crate) fn render_set_assignments(
    out: &mut String,
    values: &mut Vec<Value>,
    assignments: &[Assignment],
) -> Option<()> {
    for (index, assignment) in assignments.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        // The literal-object target (e.g. SET 1.age = ...) cannot be
        // re-rendered faithfully from the AST; skip shape caching.
        if assignment.target.is_some() || assignment.object.is_some() {
            return None;
        }
        out.push_str(&assignment.property);
        out.push_str(" = ");
        render_contextual(out, values, &assignment.value)?;
    }
    Some(())
}
