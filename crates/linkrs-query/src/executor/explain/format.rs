//! Plan Formatting Utilities (execution side)
//!
//! Execution side retains only stats-aware formatting over `PlanDescription`.
//! The pipeline (`pipeline::diagnostics`) is the sole assembly entry and must
//! go through [`render_plan`] instead of branching on formats inline. Row
//! estimates (`est_rows` pairs) are built once in `physical_plan_explain`;
//! actual rows arrive later as `ProfilingStats.rows` via the pipeline overlay.

use crate::planning::plan::explain::{PlanDescription, PlanNodeDescription};

/// Single format dispatch for EXPLAIN-family assembly.
///
/// Replaces the three inline `match format { Table, Dot }` branches in the
/// pipeline with one entry point.
pub fn render_plan(
    plan_desc: &PlanDescription,
    format: crate::parser::ast::stmt::ExplainFormat,
) -> String {
    match format {
        crate::parser::ast::stmt::ExplainFormat::Table => format_plan_as_table(plan_desc),
        crate::parser::ast::stmt::ExplainFormat::Dot => format_plan_as_dot(plan_desc),
    }
}

/// Shared spill suffix for per-operator profile rows.
///
/// Single source for the `(spilled_bytes, spill_count, spilled_rows)` guard
/// previously duplicated across table/dot/profile formatters.
pub fn format_spill_suffix(other_stats: &std::collections::HashMap<String, String>) -> String {
    let spilled = other_stats.get("spilled_bytes").map(String::as_str);
    let count = other_stats.get("spill_count").map(String::as_str);
    let rows = other_stats.get("spilled_rows").map(String::as_str);
    match (spilled, count, rows) {
        (Some(b), Some(c), Some(r)) if b != "0" || c != "0" || r != "0" => {
            format!(",spilled_rows:{r},spilled_bytes:{b},spill_count:{c}")
        }
        _ => String::new(),
    }
}

/// Shared parallel header for plan-level metadata.
///
/// Single source for the `requested/actual_workers, wall/work time, buffered
/// peaks, fallback reason, CBO notes, statistics, columnar` rendering
/// previously tripled across table/dot/profile paths.
pub fn format_parallel_header(plan_desc: &PlanDescription) -> String {
    let mut header = format!(
        "Parallel Workers: requested={}",
        plan_desc.requested_workers,
    );
    if plan_desc.actual_workers > 0 {
        header.push_str(&format!(
            ", actual={}, wall_time={}us, work_time={}us, buffered_chunks_peak={}, buffered_bytes_peak={}",
            plan_desc.actual_workers,
            plan_desc.parallel_wall_time_us,
            plan_desc.parallel_work_time_us,
            plan_desc.parallel_buffered_chunks_peak,
            plan_desc.parallel_buffered_bytes_peak,
        ));
    }
    if !plan_desc.parallel_fallback_reason.is_empty() {
        header.push_str(&format!(
            ", fallback_reason=\"{}\"",
            plan_desc.parallel_fallback_reason,
        ));
    }
    if !plan_desc.cbo_notes.is_empty() {
        header.push_str("\nCBO decisions: ");
        header.push_str(&plan_desc.cbo_notes.join("; "));
    }
    if !plan_desc.statistics_summary.is_empty() {
        header.push_str(&format!("\nStatistics: {}", plan_desc.statistics_summary));
    }
    if !plan_desc.columnar_summary.is_empty() {
        header.push_str(&format!("\nColumnar: {}", plan_desc.columnar_summary));
    }
    header
}

/// Format dependencies with the single `","` separator used by all plan
/// formatters (pipeline PROFILE rows use the same helper).
pub fn format_deps(dependencies: Option<&Vec<i64>>) -> String {
    dependencies
        .map(|d| {
            if d.is_empty() {
                "-".to_string()
            } else {
                d.iter()
                    .map(|id| id.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            }
        })
        .unwrap_or_else(|| "-".to_string())
}

/// Format plan description as a table
pub fn format_plan_as_table(plan_desc: &PlanDescription) -> String {
    let mut output = String::new();

    // Header with plan-level metadata (single shared helper).
    output.push_str("Execution Model: Batch Pull\n");
    if let Some(ref part) = plan_desc.partition_spec_description {
        output.push_str(&format!("Partitioning: {}\n", part));
    }
    output.push_str(&format_parallel_header(plan_desc));
    output.push('\n');
    output.push('\n');

    // Header with clear column names
    output.push_str("+------+------------------+------------+--------------------------------------------------+--------------------------------------------------+------------------+\n");
    output.push_str("| id   | name             | deps       | profiling_data                                    | operator_info                                    | output_var       |\n");
    output.push_str("+------+------------------+------------+--------------------------------------------------+--------------------------------------------------+------------------+\n");

    // Rows
    for node in &plan_desc.plan_node_descs {
        let id = format!("{:>4}", node.id);
        let name = truncate_or_pad(&node.name, 16);

        // Format dependencies (single separator helper).
        let deps = truncate_or_pad(&format_deps(node.dependencies.as_ref()), 10);

        // Format profiling data (single spill helper).
        let profile = if let Some(ref profiles) = node.profiles {
            profiles
                .iter()
                .map(|p| {
                    let base = format!("rows:{},time:{}us", p.rows, p.exec_duration_in_us);
                    format!("{base}{}", format_spill_suffix(&p.other_stats))
                })
                .collect::<Vec<_>>()
                .join(";")
        } else {
            "-".to_string()
        };
        let profile = truncate_or_pad(&profile, 48);

        // Format operator info
        let info = node
            .description
            .as_ref()
            .map(|descs| {
                descs
                    .iter()
                    .map(|p| format!("{}:{}", p.key, p.value))
                    .collect::<Vec<_>>()
                    .join(",")
            })
            .unwrap_or_else(|| "-".to_string());
        let info = truncate_or_pad(&info, 48);

        // Format output variable
        let output_var_str = if node.output_var.is_empty() {
            "-"
        } else {
            &node.output_var
        };
        let output_var = truncate_or_pad(output_var_str, 16);

        output.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} |\n",
            id, name, deps, profile, info, output_var
        ));
        output.push_str("+------+------------------+------------+------------------+--------------------------------------------------+------------------+\n");
    }

    output
}

/// Format plan description as DOT (Graphviz) format
pub fn format_plan_as_dot(plan_desc: &PlanDescription) -> String {
    let mut output = String::new();

    output.push_str("digraph G {\n");
    output.push_str("    rankdir=BT;\n"); // Bottom to top layout for better flow visualization
    let parallel_str = if plan_desc.actual_workers > 0 {
        format!(
            "Parallel: {} workers, wall={}us",
            plan_desc.actual_workers, plan_desc.parallel_wall_time_us,
        )
    } else if !plan_desc.parallel_fallback_reason.is_empty() {
        format!(
            "Parallel: fallback (requested={}, {})",
            plan_desc.requested_workers, plan_desc.parallel_fallback_reason,
        )
    } else {
        format!("Parallel: requested={}", plan_desc.requested_workers)
    };
    output.push_str(&format!(
        "    label=\"Execution Model: Batch Pull | {} | {}\";\n",
        plan_desc
            .partition_spec_description
            .as_deref()
            .unwrap_or("no partitioning"),
        parallel_str,
    ));
    output.push_str("    labelloc=t;\n");
    output.push_str("    fontsize=12;\n");
    output
        .push_str("    node[shape=box, style=filled, fillcolor=lightblue, fontname=\"Arial\"];\n");
    output.push_str("    edge[arrowhead=none, fontname=\"Arial\"];\n\n");

    // Find root nodes (nodes that are not dependencies of any other node)
    let mut all_deps = std::collections::HashSet::new();
    for node in &plan_desc.plan_node_descs {
        if let Some(ref deps) = node.dependencies {
            for dep_id in deps {
                all_deps.insert(*dep_id);
            }
        }
    }

    // Nodes
    for node in &plan_desc.plan_node_descs {
        let label = format_plan_node_label(node);
        let is_root = !all_deps.contains(&node.id);
        let fillcolor = if is_root { "lightgreen" } else { "lightblue" };
        output.push_str(&format!(
            "    {}[label={}, fillcolor={}];\n",
            node.id,
            escape_dot_label(&label),
            fillcolor
        ));
    }

    output.push('\n');

    // Edges with labels showing the relationship
    for node in &plan_desc.plan_node_descs {
        if let Some(ref deps) = node.dependencies {
            for (idx, dep_id) in deps.iter().enumerate() {
                let edge_label = if deps.len() > 1 {
                    format!("label=\"input{}\"", idx + 1)
                } else {
                    "".to_string()
                };
                if edge_label.is_empty() {
                    output.push_str(&format!("    {} -> {};\n", node.id, dep_id));
                } else {
                    output.push_str(&format!(
                        "    {} -> {} [{}];\n",
                        node.id, dep_id, edge_label
                    ));
                }
            }
        }
    }

    output.push('}');
    output
}

/// Format plan description as a tree structure
pub fn format_plan_as_tree(plan_desc: &PlanDescription) -> String {
    let mut output = String::new();

    // Build a map of node id to node for quick lookup
    let node_map: std::collections::HashMap<i64, &PlanNodeDescription> = plan_desc
        .plan_node_descs
        .iter()
        .map(|n| (n.id, n))
        .collect();

    // Build parent relationships
    let mut parent_map: std::collections::HashMap<i64, Vec<i64>> = std::collections::HashMap::new();
    for node in &plan_desc.plan_node_descs {
        if let Some(ref deps) = node.dependencies {
            for dep_id in deps {
                parent_map.entry(*dep_id).or_default().push(node.id);
            }
        }
    }

    // Find root nodes (nodes that are not dependencies of any other node)
    let mut root_nodes: Vec<&PlanNodeDescription> = Vec::new();
    for node in &plan_desc.plan_node_descs {
        let is_root = parent_map
            .get(&node.id)
            .map(|v| v.is_empty())
            .unwrap_or(true)
            && node
                .dependencies
                .as_ref()
                .map(|d| !d.is_empty())
                .unwrap_or(false);
        if is_root || !parent_map.contains_key(&node.id) {
            root_nodes.push(node);
        }
    }

    // If no clear root found, use nodes with no parents
    if root_nodes.is_empty() {
        for node in &plan_desc.plan_node_descs {
            if !parent_map.contains_key(&node.id) {
                root_nodes.push(node);
            }
        }
    }

    // Format tree starting from root nodes
    for (idx, root) in root_nodes.iter().enumerate() {
        if idx > 0 {
            output.push('\n');
        }
        format_tree_node(
            &mut output,
            root,
            &node_map,
            0,
            true,
            &std::collections::HashSet::new(),
        );
    }

    output
}

/// Recursively format a tree node
fn format_tree_node(
    output: &mut String,
    node: &PlanNodeDescription,
    node_map: &std::collections::HashMap<i64, &PlanNodeDescription>,
    depth: usize,
    is_last: bool,
    visited: &std::collections::HashSet<i64>,
) {
    // Check for cycles
    if visited.contains(&node.id) {
        let indent = "  ".repeat(depth);
        output.push_str(&format!(
            "{}[{}] {} (cycle detected)\n",
            indent, node.id, node.name
        ));
        return;
    }

    // Format current node
    let _indent = if depth > 0 {
        let prefix = "  ".repeat(depth - 1);
        if is_last {
            format!("{}└── ", prefix)
        } else {
            format!("{}├── ", prefix)
        }
    } else {
        String::new()
    };

    // Build node info string
    let mut info_parts = vec![format!("[{}] {}", node.id, node.name)];

    if let Some(ref desc) = node.description {
        for pair in desc.iter().take(3) {
            info_parts.push(format!("{}:{}", pair.key, pair.value));
        }
    }

    if !node.output_var.is_empty() {
        info_parts.push(format!("-> {}", node.output_var));
    }

    output.push_str(&format!("{}\n", info_parts.join(" | ")));

    // Mark as visited
    let mut new_visited = visited.clone();
    new_visited.insert(node.id);

    // Format children (dependencies)
    if let Some(ref deps) = node.dependencies {
        let dep_count = deps.len();
        for (idx, dep_id) in deps.iter().enumerate() {
            if let Some(dep_node) = node_map.get(dep_id) {
                format_tree_node(
                    output,
                    dep_node,
                    node_map,
                    depth + 1,
                    idx == dep_count - 1,
                    &new_visited,
                );
            }
        }
    }
}

/// Format a single plan node label for DOT output
fn format_plan_node_label(node: &PlanNodeDescription) -> String {
    let mut lines = vec![node.name.clone()];

    if let Some(ref profiles) = node.profiles {
        for profile in profiles {
            lines.push(format!("rows: {}", profile.rows));
            lines.push(format!("time: {}us", profile.exec_duration_in_us));
            let spill = format_spill_suffix(&profile.other_stats);
            if !spill.is_empty() {
                for part in spill.trim_start_matches(',').split(',') {
                    let pretty = part.replace(':', ": ");
                    lines.push(pretty);
                }
            }
        }
    }

    if let Some(ref desc) = node.description {
        for pair in desc {
            lines.push(format!("{}: {}", pair.key, pair.value));
        }
    }

    lines.join("\\n")
}

/// Escape a string for use in DOT label
fn escape_dot_label(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\\\""))
}

/// Truncate or pad a string to fit in a fixed-width column
fn truncate_or_pad(s: &str, width: usize) -> String {
    if s.len() > width {
        format!("{}...", &s[..width.saturating_sub(3)])
    } else {
        format!("{:width$}", s, width = width)
    }
}

/// Compatibility shim over the single table formatter.
///
/// Retained only for external callers that depend on the previous
/// `Result` signature; new code must use `format_plan_as_table`.
pub fn format_plan_with_output_table(
    plan_desc: &PlanDescription,
) -> Result<String, serde_json::Error> {
    Ok(format_plan_as_table(plan_desc))
}

/// Format plan description as JSON
pub fn format_plan_as_json(
    plan_desc: &PlanDescription,
    pretty: bool,
) -> Result<String, serde_json::Error> {
    use serde::Serialize;

    #[derive(Serialize)]
    struct SerializablePlanDescription {
        format: String,
        execution_model: &'static str,
        partition_spec_description: Option<String>,
        requested_workers: usize,
        actual_workers: usize,
        parallel_wall_time_us: u64,
        parallel_work_time_us: u64,
        parallel_buffered_chunks_peak: usize,
        parallel_buffered_bytes_peak: usize,
        parallel_fallback_reason: String,
        statistics_summary: String,
        plan_node_descs: Vec<SerializablePlanNodeDescription>,
    }

    #[derive(Serialize)]
    struct SerializablePlanNodeDescription {
        id: i64,
        name: String,
        dependencies: Option<Vec<i64>>,
        profiles: Option<Vec<SerializableProfilingStats>>,
        description: Option<Vec<SerializableKeyValue>>,
        output_var: String,
    }

    #[derive(Serialize)]
    struct SerializableProfilingStats {
        rows: i64,
        exec_duration_in_us: i64,
        total_duration_in_us: i64,
        spilled_rows: u64,
        spilled_bytes: u64,
        spill_count: u64,
    }

    #[derive(Serialize)]
    struct SerializableKeyValue {
        key: String,
        value: String,
    }

    let serializable_nodes: Vec<SerializablePlanNodeDescription> = plan_desc
        .plan_node_descs
        .iter()
        .map(|node| SerializablePlanNodeDescription {
            id: node.id,
            name: node.name.clone(),
            dependencies: node.dependencies.clone(),
            profiles: node.profiles.as_ref().map(|profiles| {
                profiles
                    .iter()
                    .map(|p| SerializableProfilingStats {
                        rows: p.rows,
                        exec_duration_in_us: p.exec_duration_in_us,
                        total_duration_in_us: p.total_duration_in_us,
                        spilled_rows: p
                            .other_stats
                            .get("spilled_rows")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0),
                        spilled_bytes: p
                            .other_stats
                            .get("spilled_bytes")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0),
                        spill_count: p
                            .other_stats
                            .get("spill_count")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0),
                    })
                    .collect()
            }),
            description: node.description.as_ref().map(|descs| {
                descs
                    .iter()
                    .map(|p| SerializableKeyValue {
                        key: p.key.clone(),
                        value: p.value.clone(),
                    })
                    .collect()
            }),
            output_var: node.output_var.clone(),
        })
        .collect();

    let serializable_plan = SerializablePlanDescription {
        format: plan_desc.format.clone(),
        execution_model: "batch_pull",
        partition_spec_description: plan_desc.partition_spec_description.clone(),
        requested_workers: plan_desc.requested_workers,
        actual_workers: plan_desc.actual_workers,
        parallel_wall_time_us: plan_desc.parallel_wall_time_us,
        parallel_work_time_us: plan_desc.parallel_work_time_us,
        parallel_buffered_chunks_peak: plan_desc.parallel_buffered_chunks_peak,
        parallel_buffered_bytes_peak: plan_desc.parallel_buffered_bytes_peak,
        parallel_fallback_reason: plan_desc.parallel_fallback_reason.clone(),
        statistics_summary: plan_desc.statistics_summary.clone(),
        plan_node_descs: serializable_nodes,
    };

    if pretty {
        serde_json::to_string_pretty(&serializable_plan)
    } else {
        serde_json::to_string(&serializable_plan)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planning::plan::explain::PlanNodeDescription;

    #[test]
    fn test_format_plan_as_table() {
        let mut plan_desc = PlanDescription::new();
        let node = PlanNodeDescription::new("ScanVertices", 1).with_description("table", "Person");
        plan_desc.add_node_desc(node);

        let output = format_plan_as_table(&plan_desc);
        assert!(output.contains("ScanVertices"));
        assert!(output.contains("Person"));
        assert!(output.contains("Parallel Workers: requested=1"));
    }

    #[test]
    fn test_format_plan_as_table_with_p8_profile() {
        let mut plan_desc = PlanDescription::new();
        plan_desc.requested_workers = 4;
        plan_desc.actual_workers = 2;
        plan_desc.parallel_wall_time_us = 12345;
        plan_desc.parallel_work_time_us = 23456;
        plan_desc.parallel_buffered_chunks_peak = 5;
        plan_desc.parallel_buffered_bytes_peak = 65536;
        let node = PlanNodeDescription::new("ScanVertices", 1);
        plan_desc.add_node_desc(node);

        let output = format_plan_as_table(&plan_desc);
        assert!(output.contains("requested=4"));
        assert!(output.contains("actual=2"));
        assert!(output.contains("wall_time=12345us"));
        assert!(output.contains("work_time=23456us"));
        assert!(output.contains("buffered_chunks_peak=5"));
        assert!(output.contains("buffered_bytes_peak=65536"));
    }

    #[test]
    fn test_format_plan_as_table_with_p8_fallback() {
        let mut plan_desc = PlanDescription::new();
        plan_desc.requested_workers = 4;
        plan_desc.actual_workers = 0;
        plan_desc.parallel_fallback_reason = "parallel disabled (max_workers=4)".to_string();
        let node = PlanNodeDescription::new("ScanVertices", 1);
        plan_desc.add_node_desc(node);

        let output = format_plan_as_table(&plan_desc);
        assert!(output.contains("requested=4"));
        assert!(output.contains("fallback_reason"));
        assert!(output.contains("max_workers"));
    }

    #[test]
    fn test_format_plan_as_dot() {
        let mut plan_desc = PlanDescription::new();
        let node = PlanNodeDescription::new("ScanVertices", 1).with_description("table", "Person");
        plan_desc.add_node_desc(node);

        let output = format_plan_as_dot(&plan_desc);
        assert!(output.contains("digraph G"));
        assert!(output.contains("ScanVertices"));
    }

    #[test]
    fn test_format_plan_as_dot_with_p8() {
        let mut plan_desc = PlanDescription::new();
        plan_desc.requested_workers = 2;
        plan_desc.actual_workers = 2;
        plan_desc.parallel_wall_time_us = 9999;
        let node = PlanNodeDescription::new("ScanVertices", 1);
        plan_desc.add_node_desc(node);

        let output = format_plan_as_dot(&plan_desc);
        assert!(output.contains("Parallel: 2 workers"));
        assert!(output.contains("wall=9999us"));
    }

    #[test]
    fn test_format_json_includes_p8_fields() {
        let mut plan_desc = PlanDescription::new();
        plan_desc.requested_workers = 2;
        plan_desc.actual_workers = 2;
        plan_desc.parallel_wall_time_us = 100;
        plan_desc.parallel_work_time_us = 200;
        plan_desc.parallel_buffered_chunks_peak = 3;
        plan_desc.parallel_buffered_bytes_peak = 4096;
        let node = PlanNodeDescription::new("Scan", 1);
        plan_desc.add_node_desc(node);

        let json = format_plan_as_json(&plan_desc, false).expect("json output");
        assert!(json.contains("\"requested_workers\":2"));
        assert!(json.contains("\"actual_workers\":2"));
        assert!(json.contains("\"parallel_wall_time_us\":100"));
        assert!(json.contains("\"parallel_work_time_us\":200"));
        assert!(json.contains("\"parallel_buffered_chunks_peak\":3"));
        assert!(json.contains("\"parallel_buffered_bytes_peak\":4096"));
    }

    #[test]
    fn test_format_plan_as_table_with_statistics_summary() {
        let mut plan_desc = PlanDescription::new();
        plan_desc.statistics_summary = "sample_limit=500, min_epoch_delta=10".to_string();
        let node = PlanNodeDescription::new("ScanVertices", 1);
        plan_desc.add_node_desc(node);

        let output = format_plan_as_table(&plan_desc);
        assert!(output.contains("Statistics: sample_limit=500, min_epoch_delta=10"));

        let empty = format_plan_as_table(&PlanDescription::new());
        assert!(!empty.contains("Statistics:"));
    }

    #[test]
    fn test_truncate_or_pad() {
        assert_eq!(truncate_or_pad("short", 10), "short     ");
        assert_eq!(truncate_or_pad("very long string", 10), "very lo...");
    }
}
