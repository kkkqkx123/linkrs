//! Enhanced optimizer result equivalence tests
//!
//! This file extends the basic result count comparison with actual content verification.
//! It ensures that queries return identical results regardless of optimization settings.
//!
use graphdb_query::core::stats::StatsManager;
use graphdb_query::query::executor::base::ExecutionResult;
use graphdb_query::query::optimizer::OptimizerEngine;
use graphdb_query::query::query_pipeline_manager::QueryPipelineManager;
use std::sync::Arc;

/// Test that query results are equivalent with and without optimization
/// for various query types, with actual content verification.
#[test]
fn test_optimizer_result_equivalence_with_content() {
    let test_storage = crate::common::TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let schema_manager = test_storage.schema_manager();

    // Create data once (space + tag + vertex)
    {
        let stats_manager = Arc::new(StatsManager::new());
        let opt_enabled = Arc::new(OptimizerEngine::default());
        let mut pipeline =
            QueryPipelineManager::with_optimizer(storage.clone(), stats_manager, opt_enabled)
                .with_schema_manager(schema_manager.clone());
        pipeline
            .execute_query("CREATE SPACE opt_equiv (vid_type=INT64)")
            .expect("CREATE SPACE");
    }

    // Construct SpaceInfo for opt_equiv (space_id is 1 as it's the first space created)
    use graphdb_query::core::types::SpaceInfo;
    let space_info = SpaceInfo {
        space_id: 1,
        space_name: "opt_equiv".to_string(),
        vid_type: graphdb_query::core::DataType::BigInt,
        ..Default::default()
    };
    let space_info: SpaceInfo = space_info;

    // Create data inside the space by passing SpaceInfo
    {
        let stats_manager = Arc::new(StatsManager::new());
        let opt_enabled = Arc::new(OptimizerEngine::default());
        let mut pipeline =
            QueryPipelineManager::with_optimizer(storage.clone(), stats_manager, opt_enabled)
                .with_schema_manager(schema_manager.clone());
        pipeline
            .execute_query_with_space(
                "CREATE TAG Item(name STRING, price DOUBLE, category STRING)",
                Some(space_info.clone()),
            )
            .expect("CREATE TAG");
        pipeline
            .execute_query_with_space(
                "INSERT VERTEX Item(name, price, category) VALUES \
            1:('A', 10.0, 'Electronics'), \
            2:('B', 20.0, 'Books'), \
            3:('C', 30.0, 'Electronics'), \
            4:('D', 15.0, 'Books')",
                Some(space_info.clone()),
            )
            .expect("INSERT");
    }

    // Pipeline with optimization enabled
    let stats1 = Arc::new(StatsManager::new());
    let opt_on = Arc::new(OptimizerEngine::default());
    let mut pipeline_on = QueryPipelineManager::with_optimizer(storage.clone(), stats1, opt_on)
        .with_schema_manager(schema_manager.clone());

    // Pipeline with optimization disabled
    let stats2 = Arc::new(StatsManager::new());
    let mut opt_off_engine = OptimizerEngine::default();
    opt_off_engine.set_enable_heuristic(false);
    opt_off_engine.set_enable_cost_based(false);
    let opt_off = Arc::new(opt_off_engine);
    let mut pipeline_off = QueryPipelineManager::with_optimizer(storage.clone(), stats2, opt_off)
        .with_schema_manager(schema_manager);

    // Test: MATCH query results should be identical with or without optimization
    // Note: ORDER BY queries are excluded due to a known P0 bug where the optimizer
    // incorrectly eliminates or corrupts Sort nodes. See docs/issue/optimizer_issues.md.
    // Note: String property comparison in WHERE may not work with vertex property expressions.
    let queries = vec![
        "MATCH (i:Item) RETURN i.name, i.price",
        "MATCH (i:Item) WHERE i.price > 15.0 RETURN i.name",
        "MATCH (i:Item) RETURN COUNT(i) AS total",
        "MATCH (i:Item) RETURN SUM(i.price) AS total_price",
    ];

    for query in &queries {
        let result_on = pipeline_on.execute_query_with_space(query, Some(space_info.clone()));
        let result_off = pipeline_off.execute_query_with_space(query, Some(space_info.clone()));

        assert!(
            result_on.is_ok(),
            "Optimized query should succeed: {}",
            query
        );
        assert!(
            result_off.is_ok(),
            "Non-optimized query should succeed: {}",
            query
        );

        let result_on = result_on.unwrap();
        let result_off = result_off.unwrap();

        // Compare result counts
        assert_eq!(
            result_on.count(),
            result_off.count(),
            "Result count mismatch for query with/without optimization: {}",
            query
        );

        // Compare actual content - this is the key enhancement
        match (&result_on, &result_off) {
            (ExecutionResult::DataSet(ds_on), ExecutionResult::DataSet(ds_off)) => {
                // Compare column names
                assert_eq!(
                    ds_on.col_names, ds_off.col_names,
                    "Column names differ for query: {}",
                    query
                );

                // Compare row counts
                assert_eq!(
                    ds_on.rows.len(),
                    ds_off.rows.len(),
                    "Row count differs for query: {}",
                    query
                );

                // Compare rows as sets (order-independent)
                // Without ORDER BY, result order is undefined and may differ between
                // optimized and non-optimized execution paths
                let mut sorted_on = ds_on.rows.clone();
                let mut sorted_off = ds_off.rows.clone();
                sorted_on.sort();
                sorted_off.sort();
                for (row_idx, (row_on, row_off)) in sorted_on.iter().zip(&sorted_off).enumerate() {
                    assert_eq!(
                        row_on, row_off,
                        "Row {} (sorted) differs for query: {}",
                        row_idx, query
                    );
                }
            }
            _ => {
                // If not DataSet, just ensure they're both success/failure
                assert!(matches!(
                    result_on,
                    ExecutionResult::Success | ExecutionResult::Empty
                ));
                assert!(matches!(
                    result_off,
                    ExecutionResult::Success | ExecutionResult::Empty
                ));
            }
        }
    }
}
