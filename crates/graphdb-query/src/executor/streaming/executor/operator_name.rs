use super::super::operators::apply_operator::ApplyOperatorKind;
use super::super::operators::blocking::BlockingOperatorKind;
use super::super::operators::ddl_operator::DdlOperatorKind;
use super::super::operators::fulltext_operator::FulltextOperatorKind;
use super::super::operators::gather_operator::GatherOperatorKind;
use super::super::operators::graph_operator::GraphOperatorKind;
use super::super::operators::join_operator::JoinOperatorKind;
use super::super::operators::recursive_fragment_operator::RecursiveFragmentOperatorKind;
use super::super::operators::set_operator::SetOperatorKind;
use super::super::operators::sink_operator::SinkOperatorKind;
use super::super::operators::source_operator::SourceOperatorKind;
use super::super::operators::state::ExchangeState;
use super::super::operators::txn_operator::TxnOperatorKind;
use super::super::operators::unary_operator::UnaryOperatorKind;
use super::super::operators::vector_operator::VectorOperatorKind;
use super::StreamingExecutor;

pub fn operator_name(exec: &StreamingExecutor) -> &'static str {
    use StreamingExecutor::*;
    match exec {
        Source(_, op) => match &op.kind {
            SourceOperatorKind::ScanVertices { .. }
            | SourceOperatorKind::StorageScanVertices { .. }
            | SourceOperatorKind::StandaloneValues { .. } => "ScanVertices",
            SourceOperatorKind::ScanEdges { .. } | SourceOperatorKind::StorageScanEdges { .. } => {
                "ScanEdges"
            }
            SourceOperatorKind::GetVertices { .. } => "GetVertices",
            SourceOperatorKind::GetEdges { .. } => "GetEdges",
            SourceOperatorKind::GetNeighbors { .. } => "GetNeighbors",
            SourceOperatorKind::IndexScan { .. } => "IndexScan",
            SourceOperatorKind::Argument => "Argument",
            SourceOperatorKind::GetProp { .. } => "GetProp",
            SourceOperatorKind::Start => "Start",
        },
        Unary(_, _, op) => match &op.kind {
            UnaryOperatorKind::Filter { .. } => "Filter",
            UnaryOperatorKind::Project { .. } => "Project",
            UnaryOperatorKind::Limit { .. } => "Limit",
            UnaryOperatorKind::Dedup { .. } => "Dedup",
            UnaryOperatorKind::Assign { .. } => "Assign",
            UnaryOperatorKind::Remove { .. } => "Remove",
            UnaryOperatorKind::Unwind { .. } => "Unwind",
            UnaryOperatorKind::AppendVertices { .. } => "AppendVertices",
            UnaryOperatorKind::Sample { .. } => "Sample",
            UnaryOperatorKind::Flatten { .. } => "Flatten",
        },
        Txn(_, _, op) => match &op.kind {
            TxnOperatorKind::BeginTransaction { .. } => "BeginTransaction",
            TxnOperatorKind::Commit { .. } => "Commit",
            TxnOperatorKind::Rollback { .. } => "Rollback",
            TxnOperatorKind::RollbackToSavepoint { .. } => "RollbackToSavepoint",
            TxnOperatorKind::Savepoint { .. } => "Savepoint",
            TxnOperatorKind::ReleaseSavepoint { .. } => "ReleaseSavepoint",
        },
        Join(_, _, _, op) => match &op.kind {
            JoinOperatorKind::HashJoin { .. } => "HashJoin",
            JoinOperatorKind::HashLeftJoin { .. } => "HashLeftJoin",
            JoinOperatorKind::NestedLoopJoin { .. } => "NestedLoopJoin",
            JoinOperatorKind::InnerJoin { .. } => "InnerJoin",
            JoinOperatorKind::LeftJoin { .. } => "LeftJoin",
            JoinOperatorKind::RightJoin { .. } => "RightJoin",
            JoinOperatorKind::FullOuterJoin { .. } => "FullOuterJoin",
            JoinOperatorKind::CrossJoin { .. } => "CrossJoin",
            JoinOperatorKind::SemiJoin { .. } => "SemiJoin",
        },
        Set(_, _, _, op) => match &op.kind {
            SetOperatorKind::Union { .. } => "Union",
            SetOperatorKind::UnionAll { .. } => "UnionAll",
            SetOperatorKind::Intersect { .. } => "Intersect",
            SetOperatorKind::Except { .. } => "Except",
            SetOperatorKind::Minus { .. } => "Minus",
        },
        Apply(_, _, _, op) => match &op.kind {
            ApplyOperatorKind::Apply { .. } => "Apply",
            ApplyOperatorKind::PatternApply { .. } => "PatternApply",
            ApplyOperatorKind::CorrelatedApply { .. } => "CorrelatedApply",
            ApplyOperatorKind::RollUpApply { .. } => "RollUpApply",
        },
        Blocking(_, _, op) => match &op.kind {
            BlockingOperatorKind::Sort { .. } => "Sort",
            BlockingOperatorKind::Aggregate { .. } => "Aggregate",
            BlockingOperatorKind::GroupBy { .. } => "GroupBy",
            BlockingOperatorKind::WindowFunction { .. } => "WindowFunction",
            BlockingOperatorKind::Window { .. } => "Window",
            BlockingOperatorKind::TopN { .. } => "TopN",
            BlockingOperatorKind::Distinct { .. } => "Distinct",
            BlockingOperatorKind::Materialize { .. } => "Materialize",
            BlockingOperatorKind::DataCollect { .. } => "DataCollect",
            BlockingOperatorKind::RollUpApply { .. } => "RollUpApply",
            BlockingOperatorKind::PartialAggregate { .. } => "PartialAggregate",
            BlockingOperatorKind::FinalAggregate { .. } => "FinalAggregate",
        },
        Graph(_, _, op) => match &op.kind {
            GraphOperatorKind::Expand { .. } => "Expand",
            GraphOperatorKind::ExpandAll { .. } => "ExpandAll",
            GraphOperatorKind::Traverse { .. } => "Traverse",
            GraphOperatorKind::TraverseAll { .. } => "TraverseAll",
            GraphOperatorKind::BiExpand { .. } => "BiExpand",
            GraphOperatorKind::BiTraverse { .. } => "BiTraverse",
            GraphOperatorKind::Subgraph { .. } => "Subgraph",
        },
        RecursiveFragment(_, _, op) => match &op.kind {
            RecursiveFragmentOperatorKind::ShortestPath { .. } => "RecursiveShortestPath",
            RecursiveFragmentOperatorKind::MultiShortestPath { .. } => "RecursiveMultiShortestPath",
            RecursiveFragmentOperatorKind::BFSShortest { .. } => "RecursiveBFSShortest",
            RecursiveFragmentOperatorKind::AllPaths { .. } => "RecursiveAllPaths",
        },
        Sink(_, _, op) => match &op.kind {
            SinkOperatorKind::CopyFrom { .. } => "CopyFrom",
            SinkOperatorKind::CopyTo { .. } => "CopyTo",
            SinkOperatorKind::InsertVertices { .. } => "InsertVertices",
            SinkOperatorKind::InsertEdges { .. } => "InsertEdges",
            SinkOperatorKind::UpdateVertices { .. } => "UpdateVertices",
            SinkOperatorKind::UpdateEdges { .. } => "UpdateEdges",
            SinkOperatorKind::DeleteVertices { .. } => "DeleteVertices",
            SinkOperatorKind::DeleteEdges { .. } => "DeleteEdges",
            SinkOperatorKind::PipeDeleteVertices { .. } => "PipeDeleteVertices",
            SinkOperatorKind::PipeDeleteEdges { .. } => "PipeDeleteEdges",
            SinkOperatorKind::DeleteTags { .. } => "DeleteTags",
        },
        Ddl(_, _, op) => match &op.kind {
            DdlOperatorKind::SpaceManage { .. } => "SpaceManage",
            DdlOperatorKind::TagManage { .. } => "TagManage",
            DdlOperatorKind::EdgeManage { .. } => "EdgeManage",
            DdlOperatorKind::IndexManage { .. } => "IndexManage",
            DdlOperatorKind::DeleteIndex { .. } => "DeleteIndex",
            DdlOperatorKind::UserManage { .. } => "UserManage",
            DdlOperatorKind::ShowStats { .. } => "ShowStats",
            DdlOperatorKind::ShowConfigs { .. } => "ShowConfigs",
            DdlOperatorKind::ShowQueries { .. } => "ShowQueries",
            DdlOperatorKind::ShowSessions { .. } => "ShowSessions",
            DdlOperatorKind::Analyze { .. } => "Analyze",
            DdlOperatorKind::Migrate { .. } => "Migrate",
            DdlOperatorKind::MigratePlan { .. } => "MigratePlan",
            DdlOperatorKind::MigrateRun { .. } => "MigrateRun",
            DdlOperatorKind::MigrateRollback { .. } => "MigrateRollback",
            DdlOperatorKind::SequenceManage { .. } => "SequenceManage",
        },
        Fulltext(_, _, op) => match &op.kind {
            FulltextOperatorKind::FulltextManage { .. } => "FulltextManage",
            FulltextOperatorKind::FulltextSearch { .. } => "FulltextSearch",
            FulltextOperatorKind::FulltextLookup { .. } => "FulltextLookup",
            FulltextOperatorKind::MatchFulltext { .. } => "MatchFulltext",
        },
        Vector(_, _, op) => match &op.kind {
            VectorOperatorKind::VectorManage { .. } => "VectorManage",
            VectorOperatorKind::VectorSearch { .. } => "VectorSearch",
            VectorOperatorKind::VectorLookup { .. } => "VectorLookup",
            VectorOperatorKind::VectorMatch { .. } => "VectorMatch",
        },
        Gather(_, _, op) => match &op.kind {
            GatherOperatorKind::Concatenate { .. } => "Gather(Concatenate)",
            GatherOperatorKind::MergeSort { .. } => "Gather(MergeSort)",
        },
        Exchange(_, _, op) => match &op.state {
            ExchangeState::Concatenate { .. } => "Exchange(Concatenate)",
            ExchangeState::MergeSort { .. } => "Exchange(MergeSort)",
            ExchangeState::RepartitionHash { .. } => "Exchange(RepartitionHash)",
            ExchangeState::Broadcast { .. } => "Exchange(Broadcast)",
            ExchangeState::Barrier { .. } => "Exchange(Barrier)",
            ExchangeState::Materialize { .. } => "Exchange(Materialize)",
        },
        Wco(..) => "WcoIntersect",
        HashShuffleJoin(_, _, _, op) => match op.join_kind {
            super::super::operators::shuffle_join_operator::HashJoinKind::Inner => {
                "HashShuffleJoin(Inner)"
            }
            super::super::operators::shuffle_join_operator::HashJoinKind::Left => {
                "HashShuffleJoin(Left)"
            }
        },
    }
}
