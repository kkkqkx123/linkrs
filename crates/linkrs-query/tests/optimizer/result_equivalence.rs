//! Enhanced optimizer result equivalence tests
//!
//! This file extends the basic result count comparison with actual content verification.
//! It ensures that queries return identical results regardless of optimization settings.
//!
use linkrs_metrics::StatsManager;
use linkrs_query::executor::base::ExecutionResult;
use linkrs_query::optimizer::OptimizerEngine;
use linkrs_query::pipeline::QueryPipelineManager;
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
    use linkrs_core::types::SpaceInfo;
    let space_info = SpaceInfo {
        space_id: 1,
        space_name: "opt_equiv".to_string(),
        vid_type: linkrs_core::DataType::BigInt,
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
                "CREATE TAG Item(id INT, name STRING, price DOUBLE, category STRING)",
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
    let opt_off = Arc::new(opt_off_engine);
    let mut pipeline_off = QueryPipelineManager::with_optimizer(storage.clone(), stats2, opt_off)
        .with_schema_manager(schema_manager);

    // Test: MATCH query results should be identical with or without optimization.
    // ORDER BY queries compare exact row order: the optimizer must preserve
    // Sort semantics, so ordered results are compared positionally while
    // unordered results are compared as sets.
    // Note: String property comparison in WHERE may not work with vertex property expressions.
    let queries = vec![
        "MATCH (i:Item) RETURN i.name, i.price",
        "MATCH (i:Item) WHERE i.price > 15.0 RETURN i.name",
        "MATCH (i:Item) RETURN COUNT(i) AS total",
        "MATCH (i:Item) RETURN SUM(i.price) AS total_price",
    ];
    let ordered_queries = vec![
        "MATCH (i:Item) RETURN i.name, i.price ORDER BY i.price ASC",
        "MATCH (i:Item) RETURN i.name ORDER BY i.name DESC LIMIT 2",
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
            (
                ExecutionResult::DataSet { data: ds_on, .. },
                ExecutionResult::DataSet { data: ds_off, .. },
            ) => {
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

    for query in &ordered_queries {
        let result_on = pipeline_on.execute_query_with_space(query, Some(space_info.clone()));
        let result_off = pipeline_off.execute_query_with_space(query, Some(space_info.clone()));
        assert!(
            result_on.is_ok(),
            "Optimized ordered query should succeed: {}",
            query
        );
        assert!(
            result_off.is_ok(),
            "Non-optimized ordered query should succeed: {}",
            query
        );
        let result_on = result_on.unwrap();
        let result_off = result_off.unwrap();
        match (&result_on, &result_off) {
            (
                ExecutionResult::DataSet { data: ds_on, .. },
                ExecutionResult::DataSet { data: ds_off, .. },
            ) => {
                assert_eq!(
                    ds_on.col_names, ds_off.col_names,
                    "Column names differ for ordered query: {}",
                    query
                );
                assert_eq!(
                    ds_on.rows, ds_off.rows,
                    "Ordered rows must match exactly for query: {}",
                    query
                );
            }
            _ => panic!("ordered query must return a dataset: {}", query),
        }
    }
}

/// Wiring: per-space cost overrides installed through the pipeline reach the
/// engine calculator for that space while other spaces keep the base config.
#[test]
fn test_space_cost_config_wiring_reaches_calculator() {
    use linkrs_query::optimizer::CostModelConfig;
    use std::collections::HashMap;

    let test_storage = crate::common::TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let schema_manager = test_storage.schema_manager();

    let stats = Arc::new(StatsManager::new());
    let engine = Arc::new(OptimizerEngine::default());
    let mut pipeline = QueryPipelineManager::with_optimizer(storage.clone(), stats, engine)
        .with_schema_manager(schema_manager.clone());

    let base_cost = pipeline
        .optimizer_engine()
        .cost_calculator_for(None)
        .config()
        .seq_page_cost;
    let mut wired = CostModelConfig::default();
    wired.seq_page_cost = base_cost + 5.0;
    let epoch_before = pipeline.optimizer_engine().cost_epoch();
    pipeline.set_space_cost_configs(
        HashMap::from([("wired_space".to_string(), wired)]),
        HashMap::from([("wired_space".to_string(), "wired".to_string())]),
    );
    assert_eq!(
        pipeline.optimizer_engine().cost_epoch(),
        epoch_before + 1,
        "installing space overrides must bump the cost epoch"
    );
    assert_eq!(
        pipeline
            .optimizer_engine()
            .cost_calculator_for(Some("wired_space"))
            .config()
            .seq_page_cost,
        wired.seq_page_cost,
        "wired space must use the override config"
    );
    assert_eq!(
        pipeline
            .optimizer_engine()
            .cost_calculator_for(Some("other_space"))
            .config()
            .seq_page_cost,
        base_cost,
        "unwired spaces must keep the base config"
    );

    pipeline
        .execute_query("CREATE SPACE wired_space (vid_type=INT64)")
        .expect("CREATE SPACE");
    let space_info = linkrs_core::types::SpaceInfo {
        space_id: 1,
        space_name: "wired_space".to_string(),
        vid_type: linkrs_core::DataType::BigInt,
        ..Default::default()
    };
    pipeline
        .execute_query_with_space(
            "CREATE TAG Item(id INT, name STRING)",
            Some(space_info.clone()),
        )
        .expect("CREATE TAG");
    pipeline
        .execute_query_with_space(
            "INSERT VERTEX Item(name) VALUES 1:('A')",
            Some(space_info.clone()),
        )
        .expect("INSERT");
    let result = pipeline
        .execute_query_with_space("MATCH (i:Item) RETURN i.name", Some(space_info))
        .expect("MATCH with space override must succeed");
    assert_eq!(result.count(), 1);
}

/// Wiring: rule selection applied at assembly reaches the physical heuristic
/// batch while the pipeline still executes filtered queries end to end.
#[test]
fn test_rule_selection_wiring_reaches_physical_heuristic() {
    use linkrs_query::optimizer::heuristic::batch::OptimizationBatch;

    const BATCHES: [OptimizationBatch; 6] = [
        OptimizationBatch::Normalize,
        OptimizationBatch::PredicatePushdown,
        OptimizationBatch::PropertyPruning,
        OptimizationBatch::ExpandPushdown,
        OptimizationBatch::Decorrelation,
        OptimizationBatch::Cleanup,
    ];
    fn total_rules(engine: &OptimizerEngine) -> usize {
        BATCHES
            .iter()
            .map(|b| {
                engine
                    .heuristic_batch()
                    .batch_rules(*b)
                    .map(|rules| rules.len())
                    .unwrap_or(0)
            })
            .sum()
    }
    fn has_rule(engine: &OptimizerEngine, name: &str) -> bool {
        BATCHES.iter().any(|b| {
            engine
                .heuristic_batch()
                .batch_rules(*b)
                .is_some_and(|rules| rules.iter().any(|rule| rule.name() == name))
        })
    }

    let mut engine = OptimizerEngine::default();
    let before = total_rules(&engine);
    assert!(before > 0, "default registry must not be empty");
    assert!(has_rule(&engine, "EliminateFilterRule"));
    engine.set_rule_selection(&["EliminateFilter".to_string()], &[]);
    assert_eq!(
        total_rules(&engine),
        before - 1,
        "disabling one rule must shrink the physical batch by exactly one rule"
    );
    assert!(
        !has_rule(&engine, "EliminateFilterRule"),
        "disabled rule must be gone from every batch"
    );

    let test_storage = crate::common::TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let schema_manager = test_storage.schema_manager();
    let stats = Arc::new(StatsManager::new());
    let mut pipeline =
        QueryPipelineManager::with_optimizer(storage.clone(), stats, Arc::new(engine))
            .with_schema_manager(schema_manager.clone());
    pipeline
        .execute_query("CREATE SPACE rule_space (vid_type=INT64)")
        .expect("CREATE SPACE");
    let space_info = linkrs_core::types::SpaceInfo {
        space_id: 1,
        space_name: "rule_space".to_string(),
        vid_type: linkrs_core::DataType::BigInt,
        ..Default::default()
    };
    pipeline
        .execute_query_with_space(
            "CREATE TAG Item(id INT, price DOUBLE)",
            Some(space_info.clone()),
        )
        .expect("CREATE TAG");
    pipeline
        .execute_query_with_space(
            "INSERT VERTEX Item(id, price) VALUES 1:(1, 10.0), 2:(2, 20.0)",
            Some(space_info.clone()),
        )
        .expect("INSERT");
    let result = pipeline
        .execute_query_with_space(
            "MATCH (i:Item) WHERE i.price > 15.0 RETURN i.id",
            Some(space_info),
        )
        .expect("filtered query must succeed with one rule disabled");
    assert_eq!(result.count(), 1);
}

/// Wiring: the feedback switch on the engine is visible through the pipeline
/// and disabling it does not break end-to-end execution.
#[test]
fn test_feedback_switch_wiring_through_pipeline() {
    let mut engine = OptimizerEngine::default();
    assert!(
        engine.feedback_enabled(),
        "feedback must be enabled by default"
    );
    engine.set_enable_feedback(false);
    assert!(
        !engine.feedback_enabled(),
        "disabling feedback must be observable on the engine"
    );

    let test_storage = crate::common::TestStorage::new().expect("Failed to create test storage");
    let storage = test_storage.storage();
    let schema_manager = test_storage.schema_manager();
    let stats = Arc::new(StatsManager::new());
    let mut pipeline =
        QueryPipelineManager::with_optimizer(storage.clone(), stats, Arc::new(engine))
            .with_schema_manager(schema_manager.clone());
    assert!(
        !pipeline.optimizer_engine().feedback_enabled(),
        "pipeline must expose the configured feedback switch"
    );

    pipeline
        .execute_query("CREATE SPACE fb_space (vid_type=INT64)")
        .expect("CREATE SPACE");
    let space_info = linkrs_core::types::SpaceInfo {
        space_id: 1,
        space_name: "fb_space".to_string(),
        vid_type: linkrs_core::DataType::BigInt,
        ..Default::default()
    };
    pipeline
        .execute_query_with_space(
            "CREATE TAG Item(id INT, name STRING)",
            Some(space_info.clone()),
        )
        .expect("CREATE TAG");
    pipeline
        .execute_query_with_space(
            "INSERT VERTEX Item(name) VALUES 1:('A')",
            Some(space_info.clone()),
        )
        .expect("INSERT");
    let result = pipeline
        .execute_query_with_space("MATCH (i:Item) RETURN i.name", Some(space_info))
        .expect("MATCH must succeed with feedback disabled");
    assert_eq!(result.count(), 1);
}
