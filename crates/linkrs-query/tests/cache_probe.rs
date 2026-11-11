//! TEMPORARY probe: does a repeated point query hit the plan cache?
//! Deleted after diagnosis.
use std::sync::Arc;

use parking_lot::RwLock;

use linkrs_metrics::StatsManager;
use linkrs_query::optimizer::OptimizerEngine;
use linkrs_query::pipeline::QueryPipelineManager;
use linkrs_query::storage::GraphStorage;
use linkrs_storage::StorageReader;

#[test]
fn probe_plan_cache_hits() {
    let storage = Arc::new(RwLock::new(
        GraphStorage::new_with_config(linkrs_storage::PropertyGraphConfig::test())
            .expect("storage"),
    ));
    let mut pipeline = QueryPipelineManager::with_optimizer(
        storage.clone(),
        Arc::new(StatsManager::new()),
        Arc::new(OptimizerEngine::default()),
    );
    pipeline.execute_query("CREATE SPACE probe").expect("space ok");
    let space = storage
        .read()
        .get_space("probe")
        .expect("get space")
        .expect("space exists");
    let run = |pipeline: &mut QueryPipelineManager<GraphStorage>, q: &str| {
        pipeline.execute_query_with_space(q, Some(space.clone()))
    };
    run(
        &mut pipeline,
        "CREATE TAG Node (name: STRING, value: DOUBLE)",
    )
    .expect("ddl ok");
    for i in 0..10 {
        run(
            &mut pipeline,
            &format!(
                "INSERT VERTEX Node(name, value) VALUES {i}:(\"{i}\", {})",
                i as f64
            ),
        )
        .expect("insert ok");
    }
    let q = "MATCH (n:Node) WHERE id(n) == 4 RETURN n.name, n.value";
    let mut warm = Vec::new();
    for _ in 0..10 {
        let start = std::time::Instant::now();
        let r = run(&mut pipeline, q).expect("query ok");
        warm.push(start.elapsed());
        assert!(matches!(
            r,
            linkrs_query::executor::base::ExecutionResult::DataSet { .. }
        ));
    }
    warm.sort();
    let snap = pipeline.plan_cache_metrics().snapshot();
    println!(
        "WARMED cache_len={} hits={} misses={} median={:?}",
        pipeline.plan_cache().len(),
        snap.hits,
        snap.misses,
        warm[warm.len() / 2],
    );
    pipeline.clear_plan_cache();
    let start = std::time::Instant::now();
    run(&mut pipeline, q).expect("cold query ok");
    println!("COLD after clear elapsed={:?}", start.elapsed());
    // Profiled entry splits parse vs execute-folded-compile vs the rest.
    for _ in 0..3 {
        run(&mut pipeline, q).expect("warmup ok");
    }
    for i in 0..3 {
        let (result, _metrics, profile) = pipeline
            .execute_query_with_profile(q, i as i64, Some(space.clone()))
            .expect("profile ok");
        println!(
            "profiled total={}us parse={}us execute={}us stats={}us rows={}",
            profile.total_duration_us,
            profile.stages.parse_us,
            profile.stages.execute_us,
            profile.stages.statistics_us,
            result.count(),
        );
    }
    {
        let start = std::time::Instant::now();
        for _ in 0..100 {
            let _ = linkrs_query::parser::Parser::new(q).parse().expect("parse");
        }
        println!("parse-only 100x avg={:?}", start.elapsed() / 100);
    }
    for round in 0..4 {
        let start = std::time::Instant::now();
        let r = run(&mut pipeline, q).expect("query ok");
        let dt = start.elapsed();
        let m = pipeline.plan_cache_metrics();
        let snap = m.snapshot();
        println!(
            "round={round} elapsed={dt:?} cache_len={} hits={} misses={} result={:?}",
            pipeline.plan_cache().len(),
            snap.hits,
            snap.misses,
            match &r {
                linkrs_query::executor::base::ExecutionResult::DataSet { data } =>
                    data.row_count(),
                _ => 9999,
            },
        );
    }
}
