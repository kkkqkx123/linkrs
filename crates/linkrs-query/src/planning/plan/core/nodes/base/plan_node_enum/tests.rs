use super::*;
use std::collections::HashSet;

/// Documented node-name list mirrored from the macro-generated registry.
/// The registry side comes from the macro-generated `ALL_VARIANT_NAMES` above;
/// this mirror is the only hand-maintained part and is test-enforced
/// against the macro table.
///
/// When adding/removing a plan node you MUST update:
/// 1. the enum variant + the four macro tables in this file
///    (is_/as_/as_mut_ + the unified `define_all_plan_nodes!` table;
///    compiler-enforced),
/// 2. this mirror list (test-enforced against the macro table).
#[cfg(not(feature = "vector"))]
const DOCUMENTED_NAMES: &[&str] = &[
    // Access (7)
    "Start",
    "GetVertices",
    "GetEdges",
    "GetNeighbors",
    "ScanVertices",
    "ScanEdges",
    "IndexScan",
    // Operation (10)
    "Project",
    "Filter",
    "Sort",
    "Limit",
    "TopN",
    "Sample",
    "Flatten",
    "Dedup",
    "Aggregate",
    "Window",
    // Join (7)
    "InnerJoin",
    "LeftJoin",
    "RightJoin",
    "CrossJoin",
    "FullOuterJoin",
    "SemiJoin",
    "WcoIntersect",
    // Traversal (6)
    "Expand",
    "ExpandAll",
    "Traverse",
    "AppendVertices",
    "BiExpand",
    "BiTraverse",
    // ControlFlow (10)
    "Argument",
    "Loop",
    "RecursiveCte",
    "PassThrough",
    "Select",
    "BeginTransaction",
    "Commit",
    "Rollback",
    "Savepoint",
    "ReleaseSavepoint",
    // DataProcessing (12)
    "DataCollect",
    "PatternApply",
    "RollUpApply",
    "CorrelatedApply",
    "Union",
    "Minus",
    "Intersect",
    "Unwind",
    "Materialize",
    "Assign",
    "Apply",
    // Algorithm (4)
    "MultiShortestPath",
    "BFSShortest",
    "AllPaths",
    "ShortestPath",
    // Management/DDL (29)
    "SpaceManage",
    "TagManage",
    "EdgeManage",
    "IndexManage",
    "UserManage",
    "FulltextManage",
    "VectorManage",
    "MacroManage",
    "TypeManage",
    "SequenceManage",
    "CopyFrom",
    "CopyTo",
    "InsertVertices",
    "InsertEdges",
    "DeleteVertices",
    "DeleteEdges",
    "DeleteIndex",
    "PipeDeleteVertices",
    "PipeDeleteEdges",
    "Update",
    "ShowStats",
    "ShowConfigs",
    "ShowQueries",
    "ShowSessions",
    "ShowFunctions",
    "ShowGraphs",
    "ShowMacros",
    "ShowAttachedDatabases",
    "ShowExtensions",
    "LoadFrom",
    "InQueryCall",
    // DataAccess (3)
    "FulltextSearch",
    "FulltextLookup",
    "MatchFulltext",
];

/// Same as above plus the three qdrant-gated vector nodes.
#[cfg(feature = "vector")]
const DOCUMENTED_NAMES: &[&str] = &[
    "Start",
    "GetVertices",
    "GetEdges",
    "GetNeighbors",
    "ScanVertices",
    "ScanEdges",
    "IndexScan",
    "Project",
    "Filter",
    "Sort",
    "Limit",
    "TopN",
    "Sample",
    "Flatten",
    "Dedup",
    "Aggregate",
    "Window",
    "InnerJoin",
    "LeftJoin",
    "RightJoin",
    "CrossJoin",
    "FullOuterJoin",
    "SemiJoin",
    "WcoIntersect",
    "Expand",
    "ExpandAll",
    "Traverse",
    "AppendVertices",
    "BiExpand",
    "BiTraverse",
    "Argument",
    "Loop",
    "RecursiveCte",
    "PassThrough",
    "Select",
    "BeginTransaction",
    "Commit",
    "Rollback",
    "Savepoint",
    "ReleaseSavepoint",
    "DataCollect",
    "Remove",
    "PatternApply",
    "RollUpApply",
    "CorrelatedApply",
    "Union",
    "Minus",
    "Intersect",
    "Unwind",
    "Materialize",
    "Assign",
    "Apply",
    "MultiShortestPath",
    "BFSShortest",
    "AllPaths",
    "ShortestPath",
    "SpaceManage",
    "TagManage",
    "EdgeManage",
    "IndexManage",
    "UserManage",
    "FulltextManage",
    "VectorManage",
    "MacroManage",
    "TypeManage",
    "SequenceManage",
    "CopyFrom",
    "CopyTo",
    "InsertVertices",
    "InsertEdges",
    "DeleteVertices",
    "DeleteEdges",
    "DeleteIndex",
    "PipeDeleteVertices",
    "PipeDeleteEdges",
    "Update",
    "ShowStats",
    "ShowConfigs",
    "ShowQueries",
    "ShowSessions",
    "ShowFunctions",
    "ShowGraphs",
    "ShowMacros",
    "ShowAttachedDatabases",
    "ShowExtensions",
    "LoadFrom",
    "InQueryCall",
    "FulltextSearch",
    "FulltextLookup",
    "MatchFulltext",
    "VectorSearch",
    "VectorLookup",
    "VectorMatch",
];

/// Default build: 89 variants. With `vector`: 92 variants.
/// (`vector-qdrant` implies `vector`, so no separate count is needed.)
#[cfg(not(feature = "vector"))]
const EXPECTED_VARIANT_COUNT: usize = 89;
#[cfg(feature = "vector")]
const EXPECTED_VARIANT_COUNT: usize = 92;

#[test]
fn variant_count_matches_documented_number() {
    assert_eq!(
        PlanNodeEnum::ALL_VARIANT_NAMES.len(),
        EXPECTED_VARIANT_COUNT,
        "PlanNodeEnum variant count drifted from the documented {EXPECTED_VARIANT_COUNT}. \
             Update the macro tables / enum in this file and the documented list."
    );
}

#[test]
fn variant_names_are_unique() {
    let mut seen = HashSet::new();
    for name in PlanNodeEnum::ALL_VARIANT_NAMES {
        assert!(seen.insert(*name), "duplicate variant name: {name}");
    }
}

#[test]
fn macro_registry_matches_documented_names() {
    let from_macro: HashSet<&str> = PlanNodeEnum::ALL_VARIANT_NAMES.iter().copied().collect();
    let documented: HashSet<&str> = DOCUMENTED_NAMES.iter().copied().collect();
    assert_eq!(
        from_macro,
        documented,
        "macro registry and documented list drifted. \
             Missing in macro table: {:?}. Extraneous in macro table: {:?}.",
        documented.difference(&from_macro).collect::<Vec<_>>(),
        from_macro.difference(&documented).collect::<Vec<_>>(),
    );
}

#[test]
fn removed_variants_are_absent() {
    for name in ["HashInnerJoin", "HashLeftJoin", "FulltextIndexScan"] {
        assert!(
            !PlanNodeEnum::ALL_VARIANT_NAMES.contains(&name),
            "removed variant {name} is still registered"
        );
    }
}

#[test]
fn simvec_nodes_are_feature_gated() {
    let names = PlanNodeEnum::ALL_VARIANT_NAMES;
    // VectorManage is an always-present management node; the three vector
    // SEARCH nodes are the qdrant-gated ones.
    let simvec: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| matches!(*n, "VectorSearch" | "VectorLookup" | "VectorMatch"))
        .collect();
    #[cfg(feature = "vector")]
    {
        assert_eq!(
            simvec.len(),
            3,
            "expected 3 vector search nodes under qdrant"
        );
        for n in ["VectorSearch", "VectorLookup", "VectorMatch"] {
            assert!(simvec.contains(&n), "missing {n} under qdrant");
        }
    }
    #[cfg(not(feature = "vector"))]
    {
        assert!(
            simvec.is_empty(),
            "vector search nodes leaked into the default build: {simvec:?}"
        );
    }
}

#[test]
fn type_name_consistent_with_registry() {
    // `type_name()` is generated from the same macro table as
    // `ALL_VARIANT_NAMES`; spot-check the default instance to keep the
    // wiring honest.
    let node = PlanNodeEnum::default();
    assert!(PlanNodeEnum::ALL_VARIANT_NAMES.contains(&node.type_name()));
}

/// Stack-footprint guard: plan trees are held by value in recursive
/// planning/optimization passes, so every inline byte multiplies across
/// frames (worse in debug builds). Snapshot-serializable expression
/// payloads must stay behind `Box`; a new inline 200+ byte field will
/// trip these caps.
#[test]
fn plan_type_sizes_stay_stack_friendly() {
    assert!(
        std::mem::size_of::<PlanNodeEnum>() <= 384,
        "PlanNodeEnum grew past 384 bytes; box the new large variant/field"
    );
    assert!(
        std::mem::size_of::<crate::planning::plan::logical::LogicalNodeEnum>() <= 448,
        "LogicalNodeEnum grew past 448 bytes; box the new large variant/field"
    );
    assert!(
        std::mem::size_of::<crate::planning::plan::SubPlan>() <= 1216,
        "SubPlan grew past 1216 bytes; pass by &mut instead of by value"
    );
    assert!(
        std::mem::size_of::<crate::planning::plan::ExecutionPlan>() <= 1216,
        "ExecutionPlan grew past 1216 bytes; pass by &mut instead of by value"
    );
}
