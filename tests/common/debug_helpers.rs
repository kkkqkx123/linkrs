//! Debug Helpers for Integration Tests
//!
//! This module provides debugging utilities for query execution analysis.
//! All code in this module is conditionally compiled with `#[cfg(test)]`
//! to avoid polluting production code.

use graphdb::core::Value;
use graphdb::query::planning::plan::ExecutionPlan;
use graphdb::query::DataSet;

/// Formats a query plan into a human-readable string representation
pub fn format_query_plan(plan: &ExecutionPlan) -> String {
    let mut output = String::new();
    output.push_str("Query Plan:\n");
    output.push_str("===========\n");

    if let Some(root) = plan.root() {
        format_plan_node(&mut output, root, 0);
    } else {
        output.push_str("(empty plan)\n");
    }

    output
}

fn format_plan_node(
    output: &mut String,
    node: &graphdb::query::planning::plan::PlanNodeEnum,
    indent: usize,
) {
    let prefix = "  ".repeat(indent);

    // Get node information
    let node_name = node.name();
    let node_id = node.id();
    let output_var = node.output_var().unwrap_or("None");
    let col_names = node.col_names();

    output.push_str(&format!(
        "{}[{}] {} (id={}, output_var={})\n",
        prefix, indent, node_name, node_id, output_var
    ));

    // Add column names if available
    if !col_names.is_empty() {
        output.push_str(&format!("{}    columns: {:?}\n", prefix, col_names));
    }

    // Add node-specific details
    if let Some(expand_all) = node.as_expand_all() {
        let input_var = expand_all.get_input_var().unwrap_or("None");
        let edge_types = expand_all.edge_types();
        let direction = expand_all.direction();
        output.push_str(&format!(
            "{}    input_var: {}, edge_types: {:?}, direction: {}\n",
            prefix, input_var, edge_types, direction
        ));
    }

    if let Some(scan) = node.as_scan_vertices() {
        let space = scan.space_name();
        output.push_str(&format!("{}    space: {}\n", prefix, space));
    }

    if let Some(_filter) = node.as_filter() {
        // Try to get filter condition as string
        output.push_str(&format!("{}    (filter condition)\n", prefix));
    }

    // Recursively format children
    let children = node.children();
    for (i, child) in children.iter().enumerate() {
        output.push_str(&format!("{}  child[{}]:\n", prefix, i));
        format_plan_node(output, child, indent + 2);
    }
}

/// Formats a DataSet into a human-readable table
pub fn format_dataset(dataset: &DataSet) -> String {
    let mut output = String::new();

    // Header
    output.push_str("Columns: ");
    output.push_str(&dataset.col_names.join(", "));
    output.push('\n');

    // Separator
    output.push_str(&"-".repeat(50));
    output.push('\n');

    // Rows
    for (i, row) in dataset.rows.iter().enumerate() {
        output.push_str(&format!("Row {}: ", i));
        let values: Vec<String> = row.iter().map(format_value).collect();
        output.push_str(&values.join(" | "));
        output.push('\n');
    }

    output.push_str(&format!("Total rows: {}\n", dataset.rows.len()));
    output
}

fn format_value(value: &Value) -> String {
    match value {
        Value::Null(_) => "NULL".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::String(s) => format!("'{}'", s),
        Value::List(l) => {
            let items: Vec<String> = l.iter().map(format_value).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Map(m) => {
            let items: Vec<String> = m
                .iter()
                .map(|(k, v)| format!("{}: {}", k, format_value(v)))
                .collect();
            format!("{{{}}}", items.join(", "))
        }
        Value::Vertex(v) => format!("Vertex({})", v.vid),
        Value::Edge(e) => format!("Edge({} -> {})", e.src, e.dst),
        Value::Path(p) => format!("Path({} steps)", p.steps.len()),
        Value::DataSet(d) => format!("DataSet({} rows)", d.rows.len()),
        _ => format!("{:?}", value),
    }
}

/// Prints a query plan for debugging purposes
#[cfg(test)]
pub fn print_query_plan(plan: &ExecutionPlan) {
    eprintln!("\n{}", format_query_plan(plan));
}

/// Prints a dataset for debugging purposes
#[cfg(test)]
pub fn print_dataset(dataset: &DataSet) {
    eprintln!("\n{}", format_dataset(dataset));
}

/// Debug helper to trace query execution
pub struct QueryExecutionTracer {
    query: String,
    steps: Vec<ExecutionStep>,
}

#[derive(Debug)]
struct ExecutionStep {
    name: String,
    description: String,
}

impl QueryExecutionTracer {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            steps: Vec::new(),
        }
    }

    pub fn add_step(&mut self, name: impl Into<String>, description: impl Into<String>) {
        self.steps.push(ExecutionStep {
            name: name.into(),
            description: description.into(),
        });
    }

    #[cfg(test)]
    pub fn print_trace(&self) {
        eprintln!("\nQuery Execution Trace for: {}", self.query);
        eprintln!("{}", "=".repeat(60));
        for (i, step) in self.steps.iter().enumerate() {
            eprintln!("Step {}: {}", i + 1, step.name);
            eprintln!("  {}", step.description);
        }
    }
}

/// Macro to assert and print debug information on failure
#[macro_export]
macro_rules! assert_with_debug {
    ($condition:expr, $plan:expr, $dataset:expr, $msg:expr) => {
        if !$condition {
            eprintln!("\nAssertion failed: {}", $msg);
            eprintln!("\nQuery Plan:");
            $crate::common::debug_helpers::print_query_plan($plan);
            eprintln!("\nResult Dataset:");
            $crate::common::debug_helpers::print_dataset($dataset);
            panic!("{}", $msg);
        }
    };
}
