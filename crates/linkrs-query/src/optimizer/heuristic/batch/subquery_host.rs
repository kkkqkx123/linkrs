use crate::planning::plan::core::nodes::base::plan_node_traits::{
    MultipleInputNode, SingleInputNode,
};
use crate::planning::plan::core::nodes::base::plan_node_visitor::PlanNodeVisitor;
use crate::planning::plan::core::nodes::graph_operations::graph_operations_node::AssignNode;
use crate::planning::plan::core::nodes::operation::filter_node::FilterNode;
use crate::planning::plan::core::nodes::operation::project_node::ProjectNode;
use crate::planning::plan::PlanNodeEnum;

/// Whether `node`'s subtree contains an expression-level subquery host
/// (Filter/Project/Assign carrying compiled subqueries).
///
/// The optimizer never rewrites such subtrees: every rule that rebuilds a
/// host node would drop its compiled subquery runners, so
/// subquery-bearing nodes and their surroundings are excluded from rule
/// application entirely (correctness first; surrounding plan optimization is
/// unaffected).
struct SubqueryHostFinder {
    found: bool,
}

impl SubqueryHostFinder {
    fn new() -> Self {
        Self { found: false }
    }

    fn check_host(
        &mut self,
        subqueries: &[crate::planning::statements::clauses::exists_planner::PlannedSubquery],
    ) -> bool {
        if !subqueries.is_empty() {
            self.found = true;
        }
        self.found
    }
}

macro_rules! impl_single_input_check {
    ($($method:ident => $type:ty),* $(,)?) => {
        $(
            fn $method(&mut self, node: &$type) -> bool {
                if self.found {
                    return true;
                }
                node.input().accept(self)
            }
        )*
    };
}

macro_rules! impl_binary_input_check {
    ($($method:ident => $type:ty),* $(,)?) => {
        $(
            fn $method(&mut self, node: &$type) -> bool {
                if self.found {
                    return true;
                }
                node.left_input().accept(self) || node.right_input().accept(self)
            }
        )*
    };
}

macro_rules! impl_deps_input_check {
    ($($method:ident => $type:ty),* $(,)?) => {
        $(
            fn $method(&mut self, node: &$type) -> bool {
                if self.found {
                    return true;
                }
                node.dependencies()
                    .iter()
                    .any(|dep| dep.accept(self))
            }
        )*
    };
}

macro_rules! impl_multi_input_inputs_check {
    ($($method:ident => $type:ty),* $(,)?) => {
        $(
            fn $method(&mut self, node: &$type) -> bool {
                if self.found {
                    return true;
                }
                node.inputs().iter().any(|input| input.accept(self))
            }
        )*
    };
}

macro_rules! impl_leaf_check {
    ($($method:ident => $type:ty),* $(,)?) => {
        $(
            fn $method(&mut self, _node: &$type) -> bool {
                self.found
            }
        )*
    };
}

impl PlanNodeVisitor for SubqueryHostFinder {
    type Result = bool;

    fn visit_default(&mut self) -> bool {
        self.found
    }

    fn visit_filter(&mut self, node: &FilterNode) -> bool {
        if self.check_host(node.subqueries()) {
            return true;
        }
        node.input().accept(self)
    }

    fn visit_project(&mut self, node: &ProjectNode) -> bool {
        if self.check_host(node.subqueries()) {
            return true;
        }
        node.input().accept(self)
    }

    fn visit_assign(&mut self, node: &AssignNode) -> bool {
        if self.check_host(node.subqueries()) {
            return true;
        }
        node.input().accept(self)
    }

    impl_single_input_check!(
        visit_aggregate => crate::planning::plan::core::nodes::graph_operations::aggregate_node::AggregateNode,
        visit_sort => crate::planning::plan::core::nodes::operation::sort_node::SortNode,
        visit_limit => crate::planning::plan::core::nodes::operation::sort_node::LimitNode,
        visit_topn => crate::planning::plan::core::nodes::operation::sort_node::TopNNode,
        visit_sample => crate::planning::plan::core::nodes::operation::sample_node::SampleNode,
        visit_dedup => crate::planning::plan::core::nodes::graph_operations::graph_operations_node::DedupNode,
        visit_unwind => crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnwindNode,
        visit_pattern_apply => crate::planning::plan::core::nodes::graph_operations::graph_operations_node::PatternApplyNode,
        visit_correlated_apply => crate::planning::plan::core::nodes::graph_operations::graph_operations_node::CorrelatedApplyNode,
        visit_roll_up_apply => crate::planning::plan::core::nodes::graph_operations::graph_operations_node::RollUpApplyNode,
        visit_data_collect => crate::planning::plan::core::nodes::graph_operations::graph_operations_node::DataCollectNode,
        visit_window => crate::planning::plan::core::nodes::graph_operations::window_node::WindowNode,
    );

    impl_multi_input_inputs_check!(
        visit_expand => crate::planning::plan::core::nodes::traversal::traversal_node::ExpandNode,
        visit_expand_all => crate::planning::plan::core::nodes::traversal::traversal_node::ExpandAllNode,
        visit_append_vertices => crate::planning::plan::core::nodes::traversal::traversal_node::AppendVerticesNode,
        visit_get_vertices => crate::planning::plan::core::nodes::access::graph_scan_node::GetVerticesNode,
        visit_get_neighbors => crate::planning::plan::core::nodes::access::graph_scan_node::GetNeighborsNode,
    );

    impl_binary_input_check!(
        visit_inner_join => crate::planning::plan::core::nodes::join::join_node::InnerJoinNode,
        visit_left_join => crate::planning::plan::core::nodes::join::join_node::LeftJoinNode,
        visit_cross_join => crate::planning::plan::core::nodes::join::join_node::CrossJoinNode,
        visit_full_outer_join => crate::planning::plan::core::nodes::join::join_node::FullOuterJoinNode,
        visit_right_join => crate::planning::plan::core::nodes::join::join_node::RightJoinNode,
        visit_semi_join => crate::planning::plan::core::nodes::join::join_node::SemiJoinNode,
        visit_bi_expand => crate::planning::plan::core::nodes::traversal::traversal_node::BiExpandNode,
        visit_bi_traverse => crate::planning::plan::core::nodes::traversal::traversal_node::BiTraverseNode,
        visit_multi_shortest_path => crate::planning::plan::core::nodes::traversal::MultiShortestPathNode,
        visit_bfs_shortest => crate::planning::plan::core::nodes::traversal::BFSShortestNode,
        visit_all_paths => crate::planning::plan::core::nodes::traversal::AllPathsNode,
        visit_shortest_path => crate::planning::plan::core::nodes::traversal::ShortestPathNode,
        visit_apply => crate::planning::plan::core::nodes::graph_operations::graph_operations_node::ApplyNode,
    );

    impl_leaf_check!(
        visit_get_edges => crate::planning::plan::core::nodes::access::graph_scan_node::GetEdgesNode,
        visit_scan_vertices => crate::planning::plan::core::nodes::access::graph_scan_node::ScanVerticesNode,
        visit_scan_edges => crate::planning::plan::core::nodes::access::graph_scan_node::ScanEdgesNode,
        visit_argument => crate::planning::plan::core::nodes::control_flow::control_flow_node::ArgumentNode,
        visit_pass_through => crate::planning::plan::core::nodes::control_flow::control_flow_node::PassThroughNode,
        visit_start => crate::planning::plan::core::nodes::control_flow::start_node::StartNode,
        visit_index_scan => crate::planning::plan::core::nodes::access::IndexScanNode,
        visit_insert_vertices => crate::planning::plan::core::nodes::data_modification::InsertVerticesNode,
        visit_insert_edges => crate::planning::plan::core::nodes::data_modification::InsertEdgesNode,
        visit_update => crate::planning::plan::core::nodes::data_modification::UpdateNode,
        visit_delete_vertices => crate::planning::plan::core::nodes::data_modification::DeleteVerticesNode,
        visit_delete_edges => crate::planning::plan::core::nodes::data_modification::DeleteEdgesNode,
        visit_delete_index => crate::planning::plan::core::nodes::data_modification::DeleteIndexNode,
        visit_pipe_delete_vertices => crate::planning::plan::core::nodes::data_modification::PipeDeleteVerticesNode,
        visit_pipe_delete_edges => crate::planning::plan::core::nodes::data_modification::PipeDeleteEdgesNode,
        visit_show_stats => crate::planning::plan::core::nodes::management::stats_nodes::ShowStatsNode,
        visit_begin_transaction => crate::planning::plan::core::nodes::control_flow::control_flow_node::BeginTransactionNode,
        visit_commit => crate::planning::plan::core::nodes::control_flow::control_flow_node::CommitNode,
        visit_rollback => crate::planning::plan::core::nodes::control_flow::control_flow_node::RollbackNode,
        visit_savepoint => crate::planning::plan::core::nodes::control_flow::control_flow_node::SavepointNode,
        visit_release_savepoint => crate::planning::plan::core::nodes::control_flow::control_flow_node::ReleaseSavepointNode,
        visit_fulltext_search => crate::planning::plan::core::nodes::search::fulltext::data_access::FulltextSearchNode,
        visit_fulltext_lookup => crate::planning::plan::core::nodes::search::fulltext::data_access::FulltextLookupNode,
        visit_match_fulltext => crate::planning::plan::core::nodes::search::fulltext::data_access::MatchFulltextNode,
        visit_show_configs => crate::planning::plan::core::nodes::management::system_nodes::ShowConfigsNode,
        visit_show_queries => crate::planning::plan::core::nodes::management::system_nodes::ShowQueriesNode,
        visit_show_sessions => crate::planning::plan::core::nodes::management::system_nodes::ShowSessionsNode,
        visit_space_manage => crate::planning::plan::core::nodes::management::manage_node_enums::SpaceManageNode,
        visit_tag_manage => crate::planning::plan::core::nodes::management::manage_node_enums::TagManageNode,
        visit_edge_manage => crate::planning::plan::core::nodes::management::manage_node_enums::EdgeManageNode,
        visit_index_manage => crate::planning::plan::core::nodes::management::manage_node_enums::IndexManageNode,
        visit_user_manage => crate::planning::plan::core::nodes::management::manage_node_enums::UserManageNode,
        visit_fulltext_manage => crate::planning::plan::core::nodes::management::manage_node_enums::FulltextManageNode,
        visit_vector_manage => crate::planning::plan::core::nodes::management::manage_node_enums::VectorManageNode,
    );

    impl_deps_input_check!(
        visit_materialize => crate::planning::plan::core::nodes::graph_operations::graph_operations_node::MaterializeNode,
        visit_union => crate::planning::plan::core::nodes::graph_operations::graph_operations_node::UnionNode,
        visit_minus => crate::planning::plan::core::nodes::graph_operations::set_operations_node::MinusNode,
        visit_intersect => crate::planning::plan::core::nodes::graph_operations::set_operations_node::IntersectNode,
        visit_traverse => crate::planning::plan::core::nodes::traversal::traversal_node::TraverseNode,
        visit_wco_intersect => crate::planning::plan::core::nodes::join::wco_intersect_node::WcoIntersectNode,
    );

    fn visit_loop(
        &mut self,
        node: &crate::planning::plan::core::nodes::control_flow::control_flow_node::LoopNode,
    ) -> bool {
        if self.found {
            return true;
        }
        node.body()
            .as_ref()
            .map(|body| body.accept(self))
            .unwrap_or(false)
    }

    fn visit_select(
        &mut self,
        node: &crate::planning::plan::core::nodes::control_flow::control_flow_node::SelectNode,
    ) -> bool {
        if self.found {
            return true;
        }
        let if_hit = node
            .if_branch()
            .as_ref()
            .map(|branch| branch.accept(self))
            .unwrap_or(false);
        if if_hit {
            return true;
        }
        node.else_branch()
            .as_ref()
            .map(|branch| branch.accept(self))
            .unwrap_or(false)
    }

    #[cfg(feature = "vector")]
    impl_leaf_check!(
        visit_simvec => crate::planning::plan::core::nodes::search::vector::data_access::VectorSearchNode,
        visit_vector_lookup => crate::planning::plan::core::nodes::search::vector::data_access::VectorLookupNode,
        visit_vector_match => crate::planning::plan::core::nodes::search::vector::data_access::VectorMatchNode,
    );
}

pub(crate) fn subtree_contains_expression_subqueries(node: &PlanNodeEnum) -> bool {
    let mut finder = SubqueryHostFinder::new();
    node.accept(&mut finder)
}
