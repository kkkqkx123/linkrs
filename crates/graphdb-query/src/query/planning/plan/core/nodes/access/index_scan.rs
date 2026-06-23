//! Plan nodes related to index scanning
//! Search-related operations, including index scanning

use crate::core::types::expr::contextual::ContextualExpression;
use crate::core::types::graph_schema::OrderDirection;
use crate::define_plan_node;
use crate::query::planning::plan::core::node_id_generator::next_node_id;
use crate::query::planning::plan::core::nodes::base::plan_node_visitor::PlanNodeVisitor;

/// Definition of sorting items
#[derive(Debug, Clone, PartialEq)]
pub struct OrderByItem {
    pub column: String,
    pub direction: OrderDirection,
}

impl OrderByItem {
    pub fn new(column: impl Into<String>, direction: OrderDirection) -> Self {
        Self {
            column: column.into(),
            direction,
        }
    }

    pub fn asc(column: impl Into<String>) -> Self {
        Self::new(column, OrderDirection::Asc)
    }

    pub fn desc(column: impl Into<String>) -> Self {
        Self::new(column, OrderDirection::Desc)
    }
}

/// Index scan type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanType {
    /// The only match (equivalent query)
    #[default]
    Unique,
    /// Prefix matching
    Prefix,
    /// Range query
    Range,
    /// Full table scan
    Full,
}

impl std::str::FromStr for ScanType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "UNIQUE" => Ok(ScanType::Unique),
            "PREFIX" => Ok(ScanType::Prefix),
            "RANGE" => Ok(ScanType::Range),
            "FULL" => Ok(ScanType::Full),
            _ => Err(format!("Unknown scan type: {}", s)),
        }
    }
}

impl ScanType {
    /// Parse the scan type from a string (with default values)
    pub fn from_str_with_default(s: &str) -> Self {
        std::str::FromStr::from_str(s).unwrap_or(ScanType::Range)
    }

    /// Translate the following text into a string:
    pub fn as_str(&self) -> &'static str {
        match self {
            ScanType::Unique => "UNIQUE",
            ScanType::Prefix => "PREFIX",
            ScanType::Range => "RANGE",
            ScanType::Full => "FULL",
        }
    }
}

/// Index scan limitation criteria
#[derive(Debug, Clone)]
pub struct IndexLimit {
    pub column: String,
    pub begin_value: Option<String>,
    pub end_value: Option<String>,
    /// Does it include a starting value?
    pub include_begin: bool,
    /// Does it include an end value?
    pub include_end: bool,
    /// Scan type
    pub scan_type: ScanType,
}

impl IndexLimit {
    /// Create restrictions for equivalent queries
    pub fn equal(column: impl Into<String>, value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            column: column.into(),
            begin_value: Some(value.clone()),
            end_value: Some(value),
            include_begin: true,
            include_end: true,
            scan_type: ScanType::Unique,
        }
    }

    /// Create range query limits.
    pub fn range(
        column: impl Into<String>,
        begin: Option<impl Into<String>>,
        end: Option<impl Into<String>>,
        include_begin: bool,
        include_end: bool,
    ) -> Self {
        Self {
            column: column.into(),
            begin_value: begin.map(|v| v.into()),
            end_value: end.map(|v| v.into()),
            include_begin,
            include_end,
            scan_type: ScanType::Range,
        }
    }

    /// Create restrictions for prefix queries
    pub fn prefix(column: impl Into<String>, prefix: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            begin_value: Some(prefix.into()),
            end_value: None,
            include_begin: true,
            include_end: false,
            scan_type: ScanType::Prefix,
        }
    }
}

define_plan_node! {
    /// Index Scan Plan Node
    pub struct IndexScanNode {
        space_id: u64,
        tag_id: i32,
        index_id: i32,
        index_name: String,
        schema_name: String,
        scan_type: ScanType,
        scan_limits: Vec<IndexLimit>,
        filter: Option<ContextualExpression>,
        return_columns: Vec<String>,
        limit: Option<i64>,
        order_by: Vec<OrderByItem>,
    }
    enum: IndexScan
    input: ZeroInputNode
}

impl IndexScanNode {
    pub fn new(
        space_id: u64,
        tag_id: i32,
        index_id: i32,
        index_name: String,
        schema_name: String,
        scan_type: ScanType,
    ) -> Self {
        Self {
            id: next_node_id(),
            space_id,
            tag_id,
            index_id,
            index_name,
            schema_name,
            scan_type,
            scan_limits: Vec::new(),
            filter: None,
            return_columns: Vec::new(),
            limit: None,
            order_by: Vec::new(),
            output_var: None,
            col_names: Vec::new(),
        }
    }

    pub fn new_with_str(
        space_id: u64,
        tag_id: i32,
        index_id: i32,
        index_name: &str,
        schema_name: &str,
        scan_type: &str,
    ) -> Self {
        Self::new(
            space_id,
            tag_id,
            index_id,
            index_name.to_string(),
            schema_name.to_string(),
            ScanType::from_str_with_default(scan_type),
        )
    }

    pub fn set_limit(&mut self, limit: i64) {
        self.limit = Some(limit);
    }

    pub fn set_order_by(&mut self, order_by: Vec<OrderByItem>) {
        self.order_by = order_by;
    }

    pub fn has_effective_filter(&self) -> bool {
        self.filter.is_some() || !self.scan_limits.is_empty()
    }

    pub fn is_tag_scan(&self) -> bool {
        self.tag_id > 0
    }

    pub fn is_edge_scan(&self) -> bool {
        self.tag_id <= 0
    }

    pub fn index_name(&self) -> &str {
        &self.index_name
    }

    pub fn schema_name(&self) -> &str {
        &self.schema_name
    }

    pub fn space_id(&self) -> u64 {
        self.space_id
    }

    pub fn tag_id(&self) -> i32 {
        self.tag_id
    }

    pub fn index_id(&self) -> i32 {
        self.index_id
    }

    pub fn scan_type(&self) -> ScanType {
        self.scan_type
    }

    pub fn scan_limits(&self) -> &[IndexLimit] {
        &self.scan_limits
    }

    pub fn set_scan_limits(&mut self, limits: Vec<IndexLimit>) {
        self.scan_limits = limits;
    }

    pub fn filter(&self) -> Option<&ContextualExpression> {
        self.filter.as_ref()
    }

    pub fn set_filter(&mut self, filter: ContextualExpression) {
        self.filter = Some(filter);
    }

    pub fn return_columns(&self) -> &[String] {
        &self.return_columns
    }

    pub fn set_return_columns(&mut self, columns: Vec<String>) {
        self.return_columns = columns;
    }

    pub fn limit(&self) -> Option<i64> {
        self.limit
    }

    pub fn order_by(&self) -> &[OrderByItem] {
        &self.order_by
    }

    pub fn accept<V>(&self, visitor: &mut V) -> V::Result
    where
        V: PlanNodeVisitor,
    {
        visitor.visit_index_scan(self)
    }
}
