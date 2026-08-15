use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlanType {
    IndexScan,
    SeqScan,
    Filter,
    Project,
    Join,
    Aggregate,
    Sort,
    Limit,
    HashJoin,
    MergeJoin,
    NestedLoop,
    Unknown(String),
}

impl PlanType {
    pub fn as_str(&self) -> &str {
        match self {
            PlanType::IndexScan => "IndexScan",
            PlanType::SeqScan => "SeqScan",
            PlanType::Filter => "Filter",
            PlanType::Project => "Project",
            PlanType::Join => "Join",
            PlanType::Aggregate => "Aggregate",
            PlanType::Sort => "Sort",
            PlanType::Limit => "Limit",
            PlanType::HashJoin => "HashJoin",
            PlanType::MergeJoin => "MergeJoin",
            PlanType::NestedLoop => "NestedLoop",
            PlanType::Unknown(s) => s.as_str(),
        }
    }
}

impl std::str::FromStr for PlanType {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "IndexScan" => PlanType::IndexScan,
            "SeqScan" => PlanType::SeqScan,
            "Filter" => PlanType::Filter,
            "Project" => PlanType::Project,
            "Join" => PlanType::Join,
            "Aggregate" => PlanType::Aggregate,
            "Sort" => PlanType::Sort,
            "Limit" => PlanType::Limit,
            "HashJoin" => PlanType::HashJoin,
            "MergeJoin" => PlanType::MergeJoin,
            "NestedLoop" => PlanType::NestedLoop,
            other => PlanType::Unknown(other.to_string()),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryPlan {
    pub plan_type: PlanType,
    pub cost: f64,
    pub rows: usize,
    pub width: usize,
    pub children: Vec<QueryPlan>,
    pub details: HashMap<String, String>,
    #[serde(default)]
    pub actual_rows: Option<usize>,
    #[serde(default)]
    pub actual_time_ms: Option<f64>,
}

impl QueryPlan {
    pub fn new(plan_type: PlanType) -> Self {
        Self {
            plan_type,
            cost: 0.0,
            rows: 0,
            width: 0,
            children: Vec::new(),
            details: HashMap::new(),
            actual_rows: None,
            actual_time_ms: None,
        }
    }

    pub fn with_cost(mut self, cost: f64) -> Self {
        self.cost = cost;
        self
    }

    pub fn with_rows(mut self, rows: usize) -> Self {
        self.rows = rows;
        self
    }

    pub fn with_width(mut self, width: usize) -> Self {
        self.width = width;
        self
    }

    pub fn add_child(mut self, child: QueryPlan) -> Self {
        self.children.push(child);
        self
    }

    pub fn add_detail(mut self, key: &str, value: &str) -> Self {
        self.details.insert(key.to_string(), value.to_string());
        self
    }

    pub fn format_tree(&self) -> String {
        self.format_tree_with_indent(0)
    }

    fn format_tree_with_indent(&self, indent: usize) -> String {
        let mut output = String::new();
        let prefix = "  ".repeat(indent);

        let mut line = format!(
            "{}{}  (cost={:.2}..{:.2} rows={} width={})",
            prefix,
            self.plan_type.as_str(),
            self.cost,
            self.cost + self.rows as f64 * 0.1,
            self.rows,
            self.width
        );

        if let (Some(actual_rows), Some(actual_time)) = (self.actual_rows, self.actual_time_ms) {
            line.push_str(&format!(
                " (actual rows={} time={:.3}ms)",
                actual_rows, actual_time
            ));
        }

        output.push_str(&line);
        output.push('\n');

        for (key, value) in &self.details {
            output.push_str(&format!("{}  {}: {}\n", prefix, key, value));
        }

        for child in &self.children {
            output.push_str(&child.format_tree_with_indent(indent + 1));
        }

        output
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "type": self.plan_type.as_str(),
            "cost": self.cost,
            "rows": self.rows,
            "width": self.width,
            "details": self.details,
            "actual_rows": self.actual_rows,
            "actual_time_ms": self.actual_time_ms,
            "children": self.children.iter().map(|c| c.to_json()).collect::<Vec<_>>()
        })
    }

    pub fn total_cost(&self) -> f64 {
        let mut total = self.cost;
        for child in &self.children {
            total += child.total_cost();
        }
        total
    }

    pub fn max_depth(&self) -> usize {
        if self.children.is_empty() {
            1
        } else {
            1 + self
                .children
                .iter()
                .map(|c| c.max_depth())
                .max()
                .unwrap_or(0)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ExplainFormat {
    Text,
    Json,
    Dot,
}

impl std::str::FromStr for ExplainFormat {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_lowercase().as_str() {
            "json" => ExplainFormat::Json,
            "dot" => ExplainFormat::Dot,
            _ => ExplainFormat::Text,
        })
    }
}
