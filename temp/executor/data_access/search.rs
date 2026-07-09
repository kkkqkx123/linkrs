//! Search for actuators
//!
//! Includes search-related executables such as index scanning

use std::sync::Arc;

use super::super::base::{BaseExecutor, ExecutorConfig, IndexScanConfig};
use crate::core::error::DBError;
use crate::core::types::VertexId;
use crate::core::{NullType, Value};
use crate::query::executor::base::{DBResult, ExecutionResult, Executor, HasStorage};
use crate::query::executor::expression::evaluator::traits::ExpressionContext;
use crate::query::DataSet;
use crate::storage::StorageReader;
use parking_lot::RwLock;

/// IndexScanExecutor - Index Scan Executor
///
/// Used to perform index-based scanning operations, supporting complex index queries
pub struct IndexScanExecutor<S: StorageReader + Send + 'static> {
    base: BaseExecutor<S>,
    space_id: u64,
    tag_id: i32,
    index_id: i32,
    index_name: String,
    schema_name: String,
    scan_type: String,
    scan_limits: Vec<crate::query::planning::plan::core::nodes::access::IndexLimit>,
    filter: Option<crate::core::Expression>,
    return_columns: Vec<String>,
    limit: Option<usize>,
    is_edge: bool,
}

impl<S: StorageReader> IndexScanExecutor<S> {
    pub fn new(base_config: ExecutorConfig<S>, scan_config: IndexScanConfig) -> Self {
        Self {
            base: BaseExecutor::new(
                base_config.id,
                "IndexScanExecutor".to_string(),
                base_config.storage,
                base_config.expr_context,
            ),
            space_id: scan_config.space_id,
            tag_id: scan_config.tag_id,
            index_id: scan_config.index_id,
            index_name: scan_config.index_name,
            schema_name: scan_config.schema_name,
            scan_type: scan_config.scan_type,
            scan_limits: scan_config.scan_limits,
            filter: scan_config.filter,
            return_columns: scan_config.return_columns,
            limit: scan_config.limit,
            is_edge: scan_config.is_edge,
        }
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

    pub fn scan_type(&self) -> &str {
        &self.scan_type
    }

    pub fn scan_limits(&self) -> &[crate::query::planning::plan::core::nodes::access::IndexLimit] {
        &self.scan_limits
    }

    pub fn return_columns(&self) -> &[String] {
        &self.return_columns
    }

    pub fn is_edge(&self) -> bool {
        self.is_edge
    }

    /// Get space name
    fn get_space_name(&self, storage: &S) -> DBResult<String> {
        log::debug!(
            "IndexScanExecutor: Getting space name for space_id: {}",
            self.space_id
        );
        if let Ok(Some(space_info)) = storage.get_space_by_id(self.space_id) {
            log::debug!(
                "IndexScanExecutor: Found space name: {}",
                space_info.space_name
            );
            Ok(space_info.space_name)
        } else {
            log::debug!("IndexScanExecutor: Space not found, using default");
            Ok("default".to_string())
        }
    }

    /// Perform an index lookup
    fn lookup_by_index(&self, storage: &S) -> DBResult<Vec<Value>> {
        let space_name = self.get_space_name(storage)?;
        log::debug!(
            "IndexScanExecutor: space_name={}, scan_type={}, schema_name={}, index_name={}",
            space_name,
            self.scan_type,
            self.schema_name,
            self.index_name
        );
        log::debug!("IndexScanExecutor: scan_limits={:?}", self.scan_limits);

        match self.scan_type.as_str() {
            "UNIQUE" => {
                // Unique Index Lookup - handles both single and multi-condition queries
                // For multi-condition queries, UNIQUE path uses the first equality column for index lookup,
                // and subsequent FilterNode applies remaining conditions in memory
                if let Some(first_limit) = self.scan_limits.first() {
                    // Find the first equality condition (where begin_value == end_value)
                    let equality_limit = self.scan_limits.iter().find(|limit| {
                        limit.begin_value.is_some()
                            && limit.end_value.is_some()
                            && limit.begin_value == limit.end_value
                    });

                    let value = if let Some(eq_limit) = equality_limit {
                        // Use the equality condition for precise index lookup
                        eq_limit
                            .begin_value
                            .as_ref()
                            .map(|v| Value::String(v.clone()))
                            .unwrap_or(Value::Null(NullType::Null))
                    } else {
                        // Fallback to first limit if no equality condition found
                        first_limit
                            .begin_value
                            .as_ref()
                            .map(|v| Value::String(v.clone()))
                            .unwrap_or(Value::Null(NullType::Null))
                    };

                    storage
                        .lookup_index(&space_name, &self.index_name, &value)
                        .map_err(DBError::from)
                } else {
                    Ok(Vec::new())
                }
            }
            "PREFIX" => {
                // prefix index lookup
                if let Some(first_limit) = self.scan_limits.first() {
                    let prefix = first_limit
                        .begin_value
                        .as_ref()
                        .map(|v| Value::String(v.clone()))
                        .unwrap_or(Value::Null(NullType::Null));
                    storage
                        .lookup_index(&space_name, &self.index_name, &prefix)
                        .map_err(DBError::from)
                } else {
                    Ok(Vec::new())
                }
            }
            "RANGE" => {
                if let Some(first_limit) = self.scan_limits.first() {
                    let column_name = &first_limit.column;
                    let include_begin = first_limit.include_begin;
                    let include_end = first_limit.include_end;

                    if self.is_edge {
                        // Scan edges by type
                        let edges = storage
                            .scan_edges_by_type(&space_name, &self.schema_name)
                            .map_err(DBError::from)?;

                        let mut results = Vec::new();
                        for edge in &edges {
                            let prop_value = self.get_property_from_edge(edge, column_name);
                            if let Some(prop_value) = prop_value {
                                let start_val = first_limit
                                    .begin_value
                                    .as_ref()
                                    .map(|v| Self::coerce_value(&prop_value, v));
                                let end_val = first_limit
                                    .end_value
                                    .as_ref()
                                    .map(|v| Self::coerce_value(&prop_value, v));

                                let passes_start = match &start_val {
                                    Some(sv) => {
                                        let cmp = Self::compare_values(&prop_value, sv);
                                        match cmp {
                                            Some(std::cmp::Ordering::Greater) => true,
                                            Some(std::cmp::Ordering::Equal) => include_begin,
                                            Some(std::cmp::Ordering::Less) => false,
                                            None => false,
                                        }
                                    }
                                    None => true,
                                };

                                if !passes_start {
                                    continue;
                                }

                                let passes_end = match &end_val {
                                    Some(ev) => {
                                        let cmp = Self::compare_values(&prop_value, ev);
                                        match cmp {
                                            Some(std::cmp::Ordering::Less) => true,
                                            Some(std::cmp::Ordering::Equal) => include_end,
                                            Some(std::cmp::Ordering::Greater) => false,
                                            None => false,
                                        }
                                    }
                                    None => true,
                                };

                                if passes_end {
                                    // Create edge key: src:dst:ranking
                                    let edge_key =
                                        format!("{}:{}:{}", edge.src, edge.dst, edge.ranking);
                                    results.push(Value::String(edge_key));
                                }
                            }
                        }

                        Ok(results)
                    } else {
                        let vertices = storage
                            .scan_vertices_by_tag(&space_name, &self.schema_name)
                            .map_err(DBError::from)?;

                        let mut results = Vec::new();
                        for vertex in vertices {
                            let prop_value = self.get_property_from_vertex(&vertex, column_name);
                            if let Some(prop_value) = prop_value {
                                let start_val = first_limit
                                    .begin_value
                                    .as_ref()
                                    .map(|v| Self::coerce_value(&prop_value, v));
                                let end_val = first_limit
                                    .end_value
                                    .as_ref()
                                    .map(|v| Self::coerce_value(&prop_value, v));

                                let passes_start = match &start_val {
                                    Some(sv) => {
                                        let cmp = Self::compare_values(&prop_value, sv);
                                        match cmp {
                                            Some(std::cmp::Ordering::Greater) => true,
                                            Some(std::cmp::Ordering::Equal) => include_begin,
                                            Some(std::cmp::Ordering::Less) => false,
                                            None => false,
                                        }
                                    }
                                    None => true,
                                };

                                if !passes_start {
                                    continue;
                                }

                                let passes_end = match &end_val {
                                    Some(ev) => {
                                        let cmp = Self::compare_values(&prop_value, ev);
                                        match cmp {
                                            Some(std::cmp::Ordering::Less) => true,
                                            Some(std::cmp::Ordering::Equal) => include_end,
                                            Some(std::cmp::Ordering::Greater) => false,
                                            None => false,
                                        }
                                    }
                                    None => true,
                                };

                                if passes_end {
                                    let vid_value = if vertex.vid.as_int64().is_some() {
                                        Value::BigInt(vertex.vid.as_int64().unwrap_or(0))
                                    } else if vertex.vid.as_str().is_some() {
                                        Value::String(vertex.vid.as_str().unwrap().to_string())
                                    } else {
                                        Value::BigInt(0)
                                    };
                                    results.push(vid_value);
                                }
                            }
                        }

                        Ok(results)
                    }
                } else {
                    Ok(Vec::new())
                }
            }
            "FULL" => {
                if self.is_edge {
                    let edges = storage
                        .scan_edges_by_type(&space_name, &self.schema_name)
                        .map_err(DBError::from)?;
                    let results: Vec<Value> = edges
                        .iter()
                        .map(|edge| {
                            Value::String(format!("{}:{}:{}", edge.src, edge.dst, edge.ranking))
                        })
                        .collect();
                    Ok(results)
                } else {
                    let vertices = storage
                        .scan_vertices_by_tag(&space_name, &self.schema_name)
                        .map_err(DBError::from)?;
                    let results: Vec<Value> = vertices
                        .iter()
                        .map(|v| {
                            if v.vid.as_int64().is_some() {
                                Value::BigInt(v.vid.as_int64().unwrap_or(0))
                            } else if v.vid.as_str().is_some() {
                                Value::String(v.vid.as_str().unwrap().to_string())
                            } else {
                                Value::BigInt(0)
                            }
                        })
                        .collect();
                    Ok(results)
                }
            }
            _ => Ok(Vec::new()),
        }
    }

    fn coerce_value(target_type: &Value, str_val: &str) -> Value {
        match target_type {
            Value::Int(_) => str_val
                .parse::<i32>()
                .map(Value::Int)
                .unwrap_or(Value::String(str_val.to_string())),
            Value::BigInt(_) => str_val
                .parse::<i64>()
                .map(Value::BigInt)
                .unwrap_or(Value::String(str_val.to_string())),
            Value::Float(_) => str_val
                .parse::<f32>()
                .map(Value::Float)
                .unwrap_or(Value::String(str_val.to_string())),
            Value::Double(_) => str_val
                .parse::<f64>()
                .map(Value::Double)
                .unwrap_or(Value::String(str_val.to_string())),
            Value::Bool(_) => str_val
                .parse::<bool>()
                .map(Value::Bool)
                .unwrap_or(Value::String(str_val.to_string())),
            _ => Value::String(str_val.to_string()),
        }
    }

    fn get_property_from_vertex(
        &self,
        vertex: &crate::core::Vertex,
        column_name: &str,
    ) -> Option<Value> {
        if let Some(value) = vertex.properties.get(column_name) {
            return Some(value.clone());
        }
        for tag in &vertex.tags {
            if let Some(value) = tag.properties.get(column_name) {
                return Some(value.clone());
            }
        }
        match column_name {
            "vid" => Some(Value::BigInt(vertex.vid.as_int64().unwrap_or(0))),
            "id" => Some(Value::BigInt(vertex.id)),
            _ => None,
        }
    }

    fn get_property_from_edge(&self, edge: &crate::core::Edge, column_name: &str) -> Option<Value> {
        if let Some(value) = edge.properties().get(column_name) {
            return Some(value.clone());
        }
        match column_name {
            "src" => Some(Value::BigInt(edge.src.as_int64().unwrap_or(0))),
            "dst" => Some(Value::BigInt(edge.dst.as_int64().unwrap_or(0))),
            "ranking" => Some(Value::BigInt(edge.ranking)),
            _ => None,
        }
    }

    /// Get the complete vertex or edge based on the ID list
    fn fetch_entities(&self, storage: &S, ids: Vec<Value>) -> DBResult<Vec<Value>> {
        let space_name = self.get_space_name(storage)?;
        // Use self.schema_name directly instead of get_schema_name, since schema_name is already set correctly
        let schema_name = &self.schema_name;

        let mut results = Vec::new();

        for id in ids {
            if self.is_edge {
                // Edge type: ID format should be src:dst:ranking
                if let Value::String(edge_key) = &id {
                    let parts: Vec<&str> = edge_key.split(':').collect();
                    if parts.len() >= 2 {
                        // Try to parse as integers first (since that's how they were inserted)
                        let src = if let Ok(src_int) = parts[0].parse::<i64>() {
                            VertexId::from_int64(src_int)
                        } else {
                            VertexId::from_string(parts[0])
                        };
                        let dst = if let Ok(dst_int) = parts[1].parse::<i64>() {
                            VertexId::from_int64(dst_int)
                        } else {
                            VertexId::from_string(parts[1])
                        };
                        let rank = if parts.len() >= 3 {
                            parts[2].parse::<i64>().unwrap_or(0)
                        } else {
                            0
                        };
                        if let Some(edge) = storage
                            .get_edge(&space_name, &src, &dst, schema_name, rank)
                            .map_err(DBError::from)?
                        {
                            results.push(Value::edge(edge));
                        }
                    }
                }
            } else {
                // Vertex Type
                let vid = VertexId::try_from(&id).map_err(DBError::from)?;
                if let Some(vertex) = storage
                    .get_vertex(&space_name, &vid)
                    .map_err(DBError::from)?
                {
                    results.push(Value::Vertex(Box::new(vertex)));
                }
            }
        }

        Ok(results)
    }

    /// Application filters
    fn apply_filter(&self, entities: Vec<Value>) -> Vec<Value> {
        if let Some(ref filter_expr) = self.filter {
            let mut context = crate::query::executor::expression::DefaultExpressionContext::new();
            entities
                .into_iter()
                .filter(|entity| {
                    context.set_variable("entity".to_string(), entity.clone());
                    match crate::query::executor::expression::evaluator::expression_evaluator::ExpressionEvaluator::evaluate(filter_expr, &mut context) {
                        Ok(value) => match &value {
                            Value::Bool(true) => true,
                            Value::Int(i) => *i != 0,
                            Value::Float(f) => *f != 0.0,
                            Value::String(s) => !s.is_empty(),
                            Value::List(l) => !l.is_empty(),
                            Value::Map(m) => !m.is_empty(),
                            _ => false,
                        },
                        Err(_) => true,
                    }
                })
                .collect()
        } else {
            entities
        }
    }

    /// Projected return columns
    fn project_columns(&self, entities: Vec<Value>) -> Vec<Value> {
        if self.return_columns.is_empty() || self.return_columns.contains(&"*".to_string()) {
            return entities;
        }

        let schema_prefix = if self.schema_name.is_empty() {
            String::new()
        } else {
            format!("{}.", self.schema_name)
        };

        entities
            .into_iter()
            .map(|entity| match entity {
                Value::Vertex(vertex) => {
                    let mut props = std::collections::HashMap::new();
                    for col in &self.return_columns {
                        let key = format!("{}{}", schema_prefix, col);
                        match col.as_str() {
                            "vid" => {
                                props
                                    .insert(key, Value::BigInt(vertex.vid.as_int64().unwrap_or(0)));
                            }
                            "id" => {
                                props.insert(key, Value::BigInt(vertex.id));
                            }
                            "*" => {
                                for (k, v) in &vertex.properties {
                                    let full_key = format!("{}{}", schema_prefix, k);
                                    props.insert(full_key, v.clone());
                                }
                            }
                            _ => {
                                if let Some(v) = vertex.properties.get(col) {
                                    props.insert(key, v.clone());
                                }
                            }
                        }
                    }
                    Value::map(props)
                }
                Value::Edge(edge) => {
                    let mut props = std::collections::HashMap::new();
                    for col in &self.return_columns {
                        let key = format!("{}{}", schema_prefix, col);
                        match col.as_str() {
                            "src" => {
                                props.insert(key, Value::BigInt(edge.src.as_int64().unwrap_or(0)));
                            }
                            "dst" => {
                                props.insert(key, Value::BigInt(edge.dst.as_int64().unwrap_or(0)));
                            }
                            "edge_type" => {
                                props.insert(key, Value::String(edge.edge_type.clone()));
                            }
                            "ranking" => {
                                props.insert(key, Value::BigInt(edge.ranking));
                            }
                            "*" => {
                                for (k, v) in &edge.props {
                                    let full_key = format!("{}{}", schema_prefix, k);
                                    props.insert(full_key, v.clone());
                                }
                            }
                            _ => {
                                if let Some(v) = edge.props.get(col) {
                                    props.insert(key, v.clone());
                                }
                            }
                        }
                    }
                    Value::map(props)
                }
                _ => entity,
            })
            .collect()
    }

    /// Comparing two values
    fn compare_values(a: &Value, b: &Value) -> Option<std::cmp::Ordering> {
        match (a, b) {
            (Value::Int(a_i), Value::Int(b_i)) => Some(a_i.cmp(b_i)),
            (Value::BigInt(a_i), Value::BigInt(b_i)) => Some(a_i.cmp(b_i)),
            (Value::Float(a_f), Value::Float(b_f)) => a_f.partial_cmp(b_f),
            (Value::Double(a_f), Value::Double(b_f)) => a_f.partial_cmp(b_f),
            (Value::Int(a_i), Value::Double(b_f)) => (*a_i as f64).partial_cmp(b_f),
            (Value::Double(a_f), Value::Int(b_i)) => a_f.partial_cmp(&(*b_i as f64)),
            (Value::String(a_s), Value::String(b_s)) => Some(a_s.cmp(b_s)),
            _ => None,
        }
    }
}

impl<S: StorageReader + Send + Sync + 'static> Executor<S> for IndexScanExecutor<S> {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        let storage = self.get_storage().read();

        let index_results = self.lookup_by_index(&storage)?;

        let entities = self.fetch_entities(&storage, index_results)?;

        let filtered = self.apply_filter(entities);

        let projected = self.project_columns(filtered);

        let limited: Vec<Value> = if let Some(limit) = self.limit {
            projected.into_iter().take(limit).collect()
        } else {
            projected
        };

        let mut dataset = DataSet::new();

        // First pass: collect all column names from all entities
        for value in &limited {
            match value {
                Value::Vertex(vertex) => {
                    // Add vid column if not exists
                    let vid_key = format!(
                        "{}vid",
                        if self.schema_name.is_empty() {
                            "".to_string()
                        } else {
                            format!("{}.", self.schema_name)
                        }
                    );
                    if !dataset.col_names.contains(&vid_key) {
                        dataset.col_names.push(vid_key);
                    }

                    // Add property columns from tags
                    for tag in &vertex.tags {
                        for k in tag.properties.keys() {
                            let key = format!("{}.{}", tag.name, k);
                            if !dataset.col_names.contains(&key) {
                                dataset.col_names.push(key);
                            }
                        }
                    }
                }
                Value::Edge(edge) => {
                    // Add edge property columns with schema prefix (e.g., KNOWS.since)
                    for k in edge.props.keys() {
                        let key = format!("{}.{}", self.schema_name, k);
                        if !dataset.col_names.contains(&key) {
                            dataset.col_names.push(key);
                        }
                    }
                }
                Value::Map(props) => {
                    // Add all keys from Map
                    for k in props.keys() {
                        if !dataset.col_names.contains(k) {
                            dataset.col_names.push(k.clone());
                        }
                    }
                }
                _ => {}
            }
        }

        // Second pass: build rows
        for value in &limited {
            match value {
                Value::Map(props) => {
                    let row: Vec<Value> = dataset
                        .col_names
                        .iter()
                        .map(|col| {
                            props
                                .get(col)
                                .cloned()
                                .unwrap_or(Value::Null(NullType::Null))
                        })
                        .collect();
                    dataset.rows.push(row);
                }
                Value::Vertex(vertex) => {
                    let mut row_map = std::collections::HashMap::new();

                    // Add vid
                    let vid_key = format!(
                        "{}vid",
                        if self.schema_name.is_empty() {
                            "".to_string()
                        } else {
                            format!("{}.", self.schema_name)
                        }
                    );
                    row_map.insert(vid_key, Value::BigInt(vertex.vid.as_int64().unwrap_or(0)));

                    // Add properties from tags
                    for tag in &vertex.tags {
                        for (k, v) in &tag.properties {
                            let key = format!("{}.{}", tag.name, k);
                            row_map.insert(key, v.clone());
                        }
                    }

                    // Build row in column order
                    let row: Vec<Value> = dataset
                        .col_names
                        .iter()
                        .map(|col| {
                            row_map
                                .get(col)
                                .cloned()
                                .unwrap_or(Value::Null(NullType::Null))
                        })
                        .collect();
                    dataset.rows.push(row);
                }
                Value::Edge(edge) => {
                    let mut row_map = std::collections::HashMap::new();

                    // Add edge properties with schema prefix
                    for (k, v) in &edge.props {
                        let key = format!("{}.{}", self.schema_name, k);
                        row_map.insert(key, v.clone());
                    }

                    // Build row in column order
                    let row: Vec<Value> = dataset
                        .col_names
                        .iter()
                        .map(|col| {
                            row_map
                                .get(col)
                                .cloned()
                                .unwrap_or(Value::Null(NullType::Null))
                        })
                        .collect();
                    dataset.rows.push(row);
                }
                _ => {
                    dataset.rows.push(vec![value.clone()]);
                }
            }
        }

        Ok(ExecutionResult::DataSet(dataset))
    }

    fn open(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn close(&mut self) -> DBResult<()> {
        Ok(())
    }

    fn is_open(&self) -> bool {
        true
    }

    fn id(&self) -> i64 {
        self.base.id
    }

    fn name(&self) -> &str {
        &self.base.name
    }

    fn description(&self) -> &str {
        "Index scan executor - scans vertices using index"
    }

    fn stats(&self) -> &crate::query::executor::base::ExecutorStats {
        self.base.get_stats()
    }

    fn stats_mut(&mut self) -> &mut crate::query::executor::base::ExecutorStats {
        self.base.get_stats_mut()
    }
}

impl<S: StorageReader> HasStorage<S> for IndexScanExecutor<S> {
    fn get_storage(&self) -> &Arc<RwLock<S>> {
        self.base.get_storage()
    }
}
