//! Actuator Macro Module
//!
//! Provide a declaration macro for simplifying the implementation of the ExecutorEnum trait

/// Generate macro methods for the `ExecutorEnum` that implement the `Executor` trait
///
/// This macro automatically generates `match` statements that call the same method for all variants.
///
/// # Usage
/// ```
/// delegate_executor_method! {
///     fn method_name(&self) -> ReturnType;
/// }
/// ```
#[macro_export]
macro_rules! delegate_executor_method {
    // Immutable self: no parameters, returns a value
    ($method:ident, $return_type:ty) => {
        fn $method(&self) -> $return_type {
            match self {
                ExecutorEnum::Start(exec) => exec.$method(),
                ExecutorEnum::Base(exec) => exec.$method(),
                ExecutorEnum::GetVertices(exec) => exec.$method(),
                ExecutorEnum::GetEdges(exec) => exec.$method(),
                ExecutorEnum::GetNeighbors(exec) => exec.$method(),
                ExecutorEnum::GetProp(exec) => exec.$method(),
                ExecutorEnum::AllPaths(exec) => exec.$method(),
                ExecutorEnum::Expand(exec) => exec.$method(),
                ExecutorEnum::ExpandAll(exec) => exec.$method(),
                ExecutorEnum::Traverse(exec) => exec.$method(),
                ExecutorEnum::BiExpand(exec) => exec.$method(),
                ExecutorEnum::BiTraverse(exec) => exec.$method(),
                ExecutorEnum::ShortestPath(exec) => exec.$method(),
                ExecutorEnum::MultiShortestPath(exec) => exec.$method(),
                ExecutorEnum::InnerJoin(exec) => exec.$method(),
                ExecutorEnum::HashInnerJoin(exec) => exec.$method(),
                ExecutorEnum::LeftJoin(exec) => exec.$method(),
                ExecutorEnum::HashLeftJoin(exec) => exec.$method(),
                ExecutorEnum::FullOuterJoin(exec) => exec.$method(),
                ExecutorEnum::CrossJoin(exec) => exec.$method(),
                ExecutorEnum::Union(exec) => exec.$method(),
                ExecutorEnum::UnionAll(exec) => exec.$method(),
                ExecutorEnum::Minus(exec) => exec.$method(),
                ExecutorEnum::Intersect(exec) => exec.$method(),
                ExecutorEnum::Filter(exec) => exec.$method(),
                ExecutorEnum::Project(exec) => exec.$method(),
                ExecutorEnum::Limit(exec) => exec.$method(),
                ExecutorEnum::Sort(exec) => exec.$method(),
                ExecutorEnum::TopN(exec) => exec.$method(),
                ExecutorEnum::Sample(exec) => exec.$method(),
                ExecutorEnum::Aggregate(exec) => exec.$method(),
                ExecutorEnum::GroupBy(exec) => exec.$method(),
                ExecutorEnum::Having(exec) => exec.$method(),
                ExecutorEnum::Dedup(exec) => exec.$method(),
                ExecutorEnum::Unwind(exec) => exec.$method(),
                ExecutorEnum::Assign(exec) => exec.$method(),
                ExecutorEnum::Materialize(exec) => exec.$method(),
                ExecutorEnum::AppendVertices(exec) => exec.$method(),
                ExecutorEnum::RollUpApply(exec) => exec.$method(),
                ExecutorEnum::PatternApply(exec) => exec.$method(),
                ExecutorEnum::Remove(exec) => exec.$method(),
                ExecutorEnum::Delete(exec) => exec.$method(),
                ExecutorEnum::PipeDelete(exec) => exec.$method(),
                ExecutorEnum::Update(exec) => exec.$method(),
                ExecutorEnum::InsertVertices(exec) => exec.$method(),
                ExecutorEnum::InsertEdges(exec) => exec.$method(),
                ExecutorEnum::Loop(exec) => exec.$method(),
                ExecutorEnum::ForLoop(exec) => exec.$method(),
                ExecutorEnum::WhileLoop(exec) => exec.$method(),
                ExecutorEnum::Select(exec) => exec.$method(),
                ExecutorEnum::ScanEdges(exec) => exec.$method(),
                ExecutorEnum::ScanVertices(exec) => exec.$method(),
                ExecutorEnum::IndexScan(exec) => exec.$method(),
                ExecutorEnum::Argument(exec) => exec.$method(),
                ExecutorEnum::PassThrough(exec) => exec.$method(),
                ExecutorEnum::DataCollect(exec) => exec.$method(),
                ExecutorEnum::BFSShortest(exec) => exec.$method(),
                // Management Executors (parameterized)
                ExecutorEnum::SpaceManage(exec) => exec.$method(),
                ExecutorEnum::TagManage(exec) => exec.$method(),
                ExecutorEnum::EdgeManage(exec) => exec.$method(),
                ExecutorEnum::IndexManage(exec) => exec.$method(),
                ExecutorEnum::UserManage(exec) => exec.$method(),
                ExecutorEnum::FulltextManage(exec) => exec.$method(),
                ExecutorEnum::VectorManage(exec) => exec.$method(),
                // Statistics
                ExecutorEnum::ShowStats(exec) => exec.$method(),
                ExecutorEnum::Analyze(exec) => exec.$method(),
                // Full-text Search Executors (data access)
                ExecutorEnum::FulltextSearch(exec) => exec.$method(),
                ExecutorEnum::FulltextLookup(exec) => exec.$method(),
                ExecutorEnum::MatchFulltext(exec) => exec.$method(),
                // Vector Search Executors (data access)
                ExecutorEnum::VectorSearch(exec) => exec.$method(),
                ExecutorEnum::VectorLookup(exec) => exec.$method(),
                ExecutorEnum::VectorMatch(exec) => exec.$method(),
            }
        }
    };

    // Variable self, no parameters, returns a value
    ($method:ident, mut $return_type:ty) => {
        fn $method(&mut self) -> $return_type {
            match self {
                ExecutorEnum::Start(exec) => exec.$method(),
                ExecutorEnum::Base(exec) => exec.$method(),
                ExecutorEnum::GetVertices(exec) => exec.$method(),
                ExecutorEnum::GetEdges(exec) => exec.$method(),
                ExecutorEnum::GetNeighbors(exec) => exec.$method(),
                ExecutorEnum::GetProp(exec) => exec.$method(),
                ExecutorEnum::AllPaths(exec) => exec.$method(),
                ExecutorEnum::Expand(exec) => exec.$method(),
                ExecutorEnum::ExpandAll(exec) => exec.$method(),
                ExecutorEnum::Traverse(exec) => exec.$method(),
                ExecutorEnum::BiExpand(exec) => exec.$method(),
                ExecutorEnum::BiTraverse(exec) => exec.$method(),
                ExecutorEnum::ShortestPath(exec) => exec.$method(),
                ExecutorEnum::MultiShortestPath(exec) => exec.$method(),
                ExecutorEnum::InnerJoin(exec) => exec.$method(),
                ExecutorEnum::HashInnerJoin(exec) => exec.$method(),
                ExecutorEnum::LeftJoin(exec) => exec.$method(),
                ExecutorEnum::HashLeftJoin(exec) => exec.$method(),
                ExecutorEnum::FullOuterJoin(exec) => exec.$method(),
                ExecutorEnum::CrossJoin(exec) => exec.$method(),
                ExecutorEnum::Union(exec) => exec.$method(),
                ExecutorEnum::UnionAll(exec) => exec.$method(),
                ExecutorEnum::Minus(exec) => exec.$method(),
                ExecutorEnum::Intersect(exec) => exec.$method(),
                ExecutorEnum::Filter(exec) => exec.$method(),
                ExecutorEnum::Project(exec) => exec.$method(),
                ExecutorEnum::Limit(exec) => exec.$method(),
                ExecutorEnum::Sort(exec) => exec.$method(),
                ExecutorEnum::TopN(exec) => exec.$method(),
                ExecutorEnum::Sample(exec) => exec.$method(),
                ExecutorEnum::Aggregate(exec) => exec.$method(),
                ExecutorEnum::GroupBy(exec) => exec.$method(),
                ExecutorEnum::Having(exec) => exec.$method(),
                ExecutorEnum::Dedup(exec) => exec.$method(),
                ExecutorEnum::Unwind(exec) => exec.$method(),
                ExecutorEnum::Assign(exec) => exec.$method(),
                ExecutorEnum::Materialize(exec) => exec.$method(),
                ExecutorEnum::AppendVertices(exec) => exec.$method(),
                ExecutorEnum::RollUpApply(exec) => exec.$method(),
                ExecutorEnum::PatternApply(exec) => exec.$method(),
                ExecutorEnum::Remove(exec) => exec.$method(),
                ExecutorEnum::Delete(exec) => exec.$method(),
                ExecutorEnum::PipeDelete(exec) => exec.$method(),
                ExecutorEnum::Update(exec) => exec.$method(),
                ExecutorEnum::InsertVertices(exec) => exec.$method(),
                ExecutorEnum::InsertEdges(exec) => exec.$method(),
                ExecutorEnum::Loop(exec) => exec.$method(),
                ExecutorEnum::ForLoop(exec) => exec.$method(),
                ExecutorEnum::WhileLoop(exec) => exec.$method(),
                ExecutorEnum::Select(exec) => exec.$method(),
                ExecutorEnum::ScanEdges(exec) => exec.$method(),
                ExecutorEnum::ScanVertices(exec) => exec.$method(),
                ExecutorEnum::IndexScan(exec) => exec.$method(),
                ExecutorEnum::Argument(exec) => exec.$method(),
                ExecutorEnum::PassThrough(exec) => exec.$method(),
                ExecutorEnum::DataCollect(exec) => exec.$method(),
                ExecutorEnum::BFSShortest(exec) => exec.$method(),
                // Management Executors (parameterized)
                ExecutorEnum::SpaceManage(exec) => exec.$method(),
                ExecutorEnum::TagManage(exec) => exec.$method(),
                ExecutorEnum::EdgeManage(exec) => exec.$method(),
                ExecutorEnum::IndexManage(exec) => exec.$method(),
                ExecutorEnum::UserManage(exec) => exec.$method(),
                ExecutorEnum::FulltextManage(exec) => exec.$method(),
                ExecutorEnum::VectorManage(exec) => exec.$method(),
                // Statistics
                ExecutorEnum::ShowStats(exec) => exec.$method(),
                ExecutorEnum::Analyze(exec) => exec.$method(),
                // Full-text Search Executors (data access)
                ExecutorEnum::FulltextSearch(exec) => exec.$method(),
                ExecutorEnum::FulltextLookup(exec) => exec.$method(),
                ExecutorEnum::MatchFulltext(exec) => exec.$method(),
                // Vector Search Executors (data access)
                ExecutorEnum::VectorSearch(exec) => exec.$method(),
                ExecutorEnum::VectorLookup(exec) => exec.$method(),
                ExecutorEnum::VectorMatch(exec) => exec.$method(),
            }
        }
    };
}

/// Generate a macro for the InputExecutor trait method of ExecutorEnum
///
/// This macro is used to generate the `set_input` and `get_input` methods.
/// Support generating actual implementations for executors with input parameters, as well as default implementations for executors without input parameters.
#[macro_export]
macro_rules! delegate_input_executor_method {
    // The `set_input` method – distinguishes between actuators with and without input data
    (set_input, $input:ty) => {
        fn set_input(&mut self, input: $input) {
            match self {
                ExecutorEnum::Filter(exec) => exec.set_input(input),
                ExecutorEnum::Project(exec) => exec.set_input(input),
                ExecutorEnum::Limit(exec) => exec.set_input(input),
                ExecutorEnum::Sort(exec) => exec.set_input(input),
                ExecutorEnum::TopN(exec) => exec.set_input(input),
                ExecutorEnum::Sample(exec) => exec.set_input(input),
                ExecutorEnum::Dedup(exec) => exec.set_input(input),
                ExecutorEnum::Expand(exec) => exec.set_input(input),
                ExecutorEnum::ExpandAll(exec) => exec.set_input(input),
                ExecutorEnum::Traverse(exec) => exec.set_input(input),
                ExecutorEnum::ShortestPath(exec) => exec.set_input(input),
                ExecutorEnum::Aggregate(exec) => exec.set_input(input),
                ExecutorEnum::GroupBy(exec) => exec.set_input(input),
                ExecutorEnum::Having(exec) => exec.set_input(input),
                ExecutorEnum::Remove(exec) => exec.set_input(input),
                ExecutorEnum::Materialize(exec) => exec.set_input(input),
                ExecutorEnum::Unwind(exec) => exec.set_input(input),
                ExecutorEnum::PipeDelete(exec) => exec.set_input(input),
                _ => {}
            }
        }
    };

    // `get_input` method – Distinguishes between executors with and without input
    (get_input, $return_type:ty) => {
        fn get_input(&self) -> $return_type {
            match self {
                ExecutorEnum::Filter(exec) => exec.get_input(),
                ExecutorEnum::Project(exec) => exec.get_input(),
                ExecutorEnum::Limit(exec) => exec.get_input(),
                ExecutorEnum::Sort(exec) => exec.get_input(),
                ExecutorEnum::TopN(exec) => exec.get_input(),
                ExecutorEnum::Sample(exec) => exec.get_input(),
                ExecutorEnum::Dedup(exec) => exec.get_input(),
                ExecutorEnum::Expand(exec) => exec.get_input(),
                ExecutorEnum::ExpandAll(exec) => exec.get_input(),
                ExecutorEnum::Traverse(exec) => exec.get_input(),
                ExecutorEnum::ShortestPath(exec) => exec.get_input(),
                ExecutorEnum::MultiShortestPath(exec) => exec.get_input(),
                ExecutorEnum::Aggregate(exec) => exec.get_input(),
                ExecutorEnum::GroupBy(exec) => exec.get_input(),
                ExecutorEnum::Having(exec) => exec.get_input(),
                ExecutorEnum::Remove(exec) => exec.get_input(),
                ExecutorEnum::Materialize(exec) => exec.get_input(),
                ExecutorEnum::Unwind(exec) => exec.get_input(),
                ExecutorEnum::PipeDelete(exec) => exec.get_input(),
                _ => None,
            }
        }
    };
}

/// Macro for generating the Debug trait implementation for ExecutorEnum
#[macro_export]
macro_rules! delegate_debug_fmt {
    () => {
        fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
            let (variant_name, exec_name) = match self {
                ExecutorEnum::Start(exec) => ("Start", exec.name()),
                ExecutorEnum::Base(exec) => ("Base", exec.name()),
                ExecutorEnum::GetVertices(exec) => ("GetVertices", exec.name()),
                ExecutorEnum::GetEdges(exec) => ("GetEdges", exec.name()),
                ExecutorEnum::GetNeighbors(exec) => ("GetNeighbors", exec.name()),
                ExecutorEnum::GetProp(exec) => ("GetProp", exec.name()),
                ExecutorEnum::AllPaths(exec) => ("AllPaths", exec.name()),
                ExecutorEnum::Expand(exec) => ("Expand", exec.name()),
                ExecutorEnum::ExpandAll(exec) => ("ExpandAll", exec.name()),
                ExecutorEnum::Traverse(exec) => ("Traverse", exec.name()),
                ExecutorEnum::BiExpand(exec) => ("BiExpand", exec.name()),
                ExecutorEnum::BiTraverse(exec) => ("BiTraverse", exec.name()),
                ExecutorEnum::ShortestPath(exec) => ("ShortestPath", exec.name()),
                ExecutorEnum::MultiShortestPath(exec) => ("MultiShortestPath", exec.name()),
                ExecutorEnum::InnerJoin(exec) => ("InnerJoin", exec.name()),
                ExecutorEnum::HashInnerJoin(exec) => ("HashInnerJoin", exec.name()),
                ExecutorEnum::LeftJoin(exec) => ("LeftJoin", exec.name()),
                ExecutorEnum::HashLeftJoin(exec) => ("HashLeftJoin", exec.name()),
                ExecutorEnum::FullOuterJoin(exec) => ("FullOuterJoin", exec.name()),
                ExecutorEnum::CrossJoin(exec) => ("CrossJoin", exec.name()),
                ExecutorEnum::Union(exec) => ("Union", exec.name()),
                ExecutorEnum::UnionAll(exec) => ("UnionAll", exec.name()),
                ExecutorEnum::Minus(exec) => ("Minus", exec.name()),
                ExecutorEnum::Intersect(exec) => ("Intersect", exec.name()),
                ExecutorEnum::Filter(exec) => ("Filter", exec.name()),
                ExecutorEnum::Project(exec) => ("Project", exec.name()),
                ExecutorEnum::Limit(exec) => ("Limit", exec.name()),
                ExecutorEnum::Sort(exec) => ("Sort", exec.name()),
                ExecutorEnum::TopN(exec) => ("TopN", exec.name()),
                ExecutorEnum::Sample(exec) => ("Sample", exec.name()),
                ExecutorEnum::Aggregate(exec) => ("Aggregate", exec.name()),
                ExecutorEnum::GroupBy(exec) => ("GroupBy", exec.name()),
                ExecutorEnum::Having(exec) => ("Having", exec.name()),
                ExecutorEnum::Dedup(exec) => ("Dedup", exec.name()),
                ExecutorEnum::Unwind(exec) => ("Unwind", exec.name()),
                ExecutorEnum::Assign(exec) => ("Assign", exec.name()),
                ExecutorEnum::Materialize(exec) => ("Materialize", exec.name()),
                ExecutorEnum::AppendVertices(exec) => ("AppendVertices", exec.name()),
                ExecutorEnum::RollUpApply(exec) => ("RollUpApply", exec.name()),
                ExecutorEnum::PatternApply(exec) => ("PatternApply", exec.name()),
                ExecutorEnum::Remove(exec) => ("Remove", exec.name()),
                ExecutorEnum::Delete(exec) => ("Delete", exec.name()),
                ExecutorEnum::PipeDelete(exec) => ("PipeDelete", exec.name()),
                ExecutorEnum::Update(exec) => ("Update", exec.name()),
                ExecutorEnum::InsertVertices(exec) => ("InsertVertices", exec.name()),
                ExecutorEnum::InsertEdges(exec) => ("InsertEdges", exec.name()),
                ExecutorEnum::Loop(exec) => ("Loop", exec.name()),
                ExecutorEnum::ForLoop(exec) => ("ForLoop", exec.name()),
                ExecutorEnum::WhileLoop(exec) => ("WhileLoop", exec.name()),
                ExecutorEnum::Select(exec) => ("Select", exec.name()),
                ExecutorEnum::ScanEdges(exec) => ("ScanEdges", exec.name()),
                ExecutorEnum::ScanVertices(exec) => ("ScanVertices", exec.name()),
                ExecutorEnum::IndexScan(exec) => ("IndexScan", exec.name()),
                ExecutorEnum::Argument(exec) => ("Argument", exec.name()),
                ExecutorEnum::PassThrough(exec) => ("PassThrough", exec.name()),
                ExecutorEnum::DataCollect(exec) => ("DataCollect", exec.name()),
                ExecutorEnum::BFSShortest(exec) => ("BFSShortest", exec.name()),
                // Management Executors (parameterized)
                ExecutorEnum::SpaceManage(exec) => ("SpaceManage", exec.name()),
                ExecutorEnum::TagManage(exec) => ("TagManage", exec.name()),
                ExecutorEnum::EdgeManage(exec) => ("EdgeManage", exec.name()),
                ExecutorEnum::IndexManage(exec) => ("IndexManage", exec.name()),
                ExecutorEnum::UserManage(exec) => ("UserManage", exec.name()),
                ExecutorEnum::FulltextManage(exec) => ("FulltextManage", exec.name()),
                ExecutorEnum::VectorManage(exec) => ("VectorManage", exec.name()),
                // Statistics
                ExecutorEnum::ShowStats(exec) => ("ShowStats", exec.name()),
                ExecutorEnum::Analyze(exec) => ("Analyze", exec.name()),
                // Full-text Search Executors (data access)
                ExecutorEnum::FulltextSearch(exec) => ("FulltextSearch", exec.name()),
                ExecutorEnum::FulltextLookup(exec) => ("FulltextLookup", exec.name()),
                ExecutorEnum::MatchFulltext(exec) => ("MatchFulltext", exec.name()),
                // Vector Search Executors (data access)
                ExecutorEnum::VectorSearch(exec) => ("VectorSearch", exec.name()),
                ExecutorEnum::VectorLookup(exec) => ("VectorLookup", exec.name()),
                ExecutorEnum::VectorMatch(exec) => ("VectorMatch", exec.name()),
            };
            f.write_str(&format!("ExecutorEnum::{}({})", variant_name, exec_name))
        }
    };
}

/// Macro for generating the NodeType trait implementation for ExecutorEnum
#[macro_export]
macro_rules! delegate_node_type_id {
    () => {
        fn node_type_id(&self) -> &'static str {
            match self {
                ExecutorEnum::Start(_) => "start",
                ExecutorEnum::Base(_) => "base",
                ExecutorEnum::GetVertices(_) => "get_vertices",
                ExecutorEnum::GetEdges(_) => "get_edges",
                ExecutorEnum::GetNeighbors(_) => "get_neighbors",
                ExecutorEnum::GetProp(_) => "get_prop",
                ExecutorEnum::AllPaths(_) => "all_paths",
                ExecutorEnum::Expand(_) => "expand",
                ExecutorEnum::ExpandAll(_) => "expand_all",
                ExecutorEnum::Traverse(_) => "traverse",
                ExecutorEnum::BiExpand(_) => "bi_expand",
                ExecutorEnum::BiTraverse(_) => "bi_traverse",
                ExecutorEnum::ShortestPath(_) => "shortest_path",
                ExecutorEnum::MultiShortestPath(_) => "multi_shortest_path",
                ExecutorEnum::InnerJoin(_) => "inner_join",
                ExecutorEnum::HashInnerJoin(_) => "hash_inner_join",
                ExecutorEnum::LeftJoin(_) => "left_join",
                ExecutorEnum::HashLeftJoin(_) => "hash_left_join",
                ExecutorEnum::FullOuterJoin(_) => "full_outer_join",
                ExecutorEnum::CrossJoin(_) => "cross_join",
                ExecutorEnum::Union(_) => "union",
                ExecutorEnum::UnionAll(_) => "union_all",
                ExecutorEnum::Minus(_) => "minus",
                ExecutorEnum::Intersect(_) => "intersect",
                ExecutorEnum::Filter(_) => "filter",
                ExecutorEnum::Project(_) => "project",
                ExecutorEnum::Limit(_) => "limit",
                ExecutorEnum::Sort(_) => "sort",
                ExecutorEnum::TopN(_) => "topn",
                ExecutorEnum::Sample(_) => "sample",
                ExecutorEnum::Aggregate(_) => "aggregate",
                ExecutorEnum::GroupBy(_) => "group_by",
                ExecutorEnum::Having(_) => "having",
                ExecutorEnum::Dedup(_) => "dedup",
                ExecutorEnum::Unwind(_) => "unwind",
                ExecutorEnum::Assign(_) => "assign",
                ExecutorEnum::Materialize(_) => "materialize",
                ExecutorEnum::AppendVertices(_) => "append_vertices",
                ExecutorEnum::RollUpApply(_) => "rollup_apply",
                ExecutorEnum::PatternApply(_) => "pattern_apply",
                ExecutorEnum::Remove(_) => "remove",
                ExecutorEnum::Delete(_) => "delete",
                ExecutorEnum::PipeDelete(_) => "pipe_delete",
                ExecutorEnum::Update(_) => "update",
                ExecutorEnum::InsertVertices(_) => "insert_vertices",
                ExecutorEnum::InsertEdges(_) => "insert_edges",
                ExecutorEnum::Loop(_) => "loop",
                ExecutorEnum::ForLoop(_) => "for_loop",
                ExecutorEnum::WhileLoop(_) => "while_loop",
                ExecutorEnum::Select(_) => "select",
                ExecutorEnum::ScanEdges(_) => "scan_edges",
                ExecutorEnum::ScanVertices(_) => "scan_vertices",
                ExecutorEnum::IndexScan(_) => "index_scan",
                ExecutorEnum::Argument(_) => "argument",
                ExecutorEnum::PassThrough(_) => "pass_through",
                ExecutorEnum::DataCollect(_) => "data_collect",
                ExecutorEnum::BFSShortest(_) => "bfs_shortest",
                // Management Executors (parameterized)
                ExecutorEnum::SpaceManage(e) => e.node_type_id(),
                ExecutorEnum::TagManage(e) => e.node_type_id(),
                ExecutorEnum::EdgeManage(e) => e.node_type_id(),
                ExecutorEnum::IndexManage(e) => e.node_type_id(),
                ExecutorEnum::UserManage(e) => e.node_type_id(),
                ExecutorEnum::FulltextManage(e) => e.node_type_id(),
                ExecutorEnum::VectorManage(e) => e.node_type_id(),
                // Statistics
                ExecutorEnum::ShowStats(_) => "show_stats",
                ExecutorEnum::Analyze(_) => "analyze",
                // Full-text Search Executors (data access)
                ExecutorEnum::FulltextSearch(_) => "fulltext_search",
                ExecutorEnum::FulltextLookup(_) => "fulltext_lookup",
                ExecutorEnum::MatchFulltext(_) => "match_fulltext",
                // Vector Search Executors (data access)
                ExecutorEnum::VectorSearch(_) => "vector_search",
                ExecutorEnum::VectorLookup(_) => "vector_lookup",
                ExecutorEnum::VectorMatch(_) => "vector_match",
            }
        }
    };
}
