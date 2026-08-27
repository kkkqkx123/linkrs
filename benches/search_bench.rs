use std::time::Duration;

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

fn create_benchmark_group<'a>(
    c: &'a mut Criterion,
    name: &str,
) -> criterion::BenchmarkGroup<'a, criterion::measurement::WallTime> {
    let mut group = c.benchmark_group(name);
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(1));
    group
}

#[cfg(feature = "fulltext-search")]
fn bench_fulltext_index_build(c: &mut Criterion) {
    use graphdb_search::config::FulltextConfig;
    use graphdb_search::manager::FulltextIndexManager;
    use graphdb_search::EngineType;
    use std::sync::Arc;
    use tempfile::TempDir;

    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("fulltext_index_build");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(30);
    group.warm_up_time(Duration::from_secs(1));

    for doc_count in &[100, 500] {
        let temp_dir = TempDir::new().expect("temp dir");
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            sync: Default::default(),
            tantivy: Default::default(),
            cache_size: 100,
            max_result_cache: 1000,
            result_cache_ttl_secs: 60,
        };
        let manager = Arc::new(FulltextIndexManager::new(config).expect("manager"));
        let m = manager.clone();
        let id = *doc_count;

        group.bench_with_input(BenchmarkId::from_parameter(doc_count), doc_count, |b, _| {
            b.to_async(&rt).iter(|| {
                let mgr = m.clone();
                async move {
                    mgr.create_index(1, "Article", "content", Some(EngineType::Bm25))
                        .await
                        .expect("create");
                    for i in 0..id {
                        mgr.index_edge_property(
                            1,
                            "Article",
                            "content",
                            &format!("doc_{}", i),
                            &format!("benchmark document {}", i),
                        )
                        .await
                        .expect("index");
                    }
                    black_box(id)
                }
            });
        });
    }

    group.finish();
}

#[cfg(not(feature = "fulltext-search"))]
fn bench_fulltext_index_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("fulltext_index_build");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(1));

    group.bench_function("index_100", |b| b.iter(|| black_box(100)));
    group.bench_function("index_500", |b| b.iter(|| black_box(500)));

    group.finish();
}

#[cfg(feature = "fulltext-search")]
fn bench_fulltext_search(c: &mut Criterion) {
    use graphdb_search::config::FulltextConfig;
    use graphdb_search::manager::FulltextIndexManager;
    use graphdb_search::EngineType;
    use std::sync::Arc;
    use tempfile::TempDir;

    let rt = tokio::runtime::Runtime::new().unwrap();
    let temp_dir = TempDir::new().expect("temp dir");
    let config = FulltextConfig {
        enabled: true,
        index_path: temp_dir.path().to_path_buf(),
        default_engine: EngineType::Bm25,
        sync: Default::default(),
        tantivy: Default::default(),
        cache_size: 100,
        max_result_cache: 1000,
        result_cache_ttl_secs: 60,
    };
    let manager = Arc::new(FulltextIndexManager::new(config).expect("manager"));
    let m = manager.clone();
    rt.block_on(async {
        m.create_index(1, "Article", "content", Some(EngineType::Bm25))
            .await
            .expect("create");
        for i in 0..1000 {
            let keywords = ["rust", "database", "performance", "search", "query"];
            let kw = keywords[i % keywords.len()];
            m.index_edge_property(1, "Article", "content", &format!("doc_{}", i), kw)
                .await
                .expect("index");
        }
    });

    let mut group = c.benchmark_group("fulltext_search");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(1));

    for query in &["rust", "database performance", "search query"] {
        let m = manager.clone();
        group.bench_function(format!("search_{}", query.replace(' ', "_")), |b| {
            b.to_async(&rt).iter(|| {
                let mgr = m.clone();
                let q = query.to_string();
                async move {
                    let results = mgr
                        .search(1, "Article", "content", &q, 10)
                        .await
                        .expect("search");
                    black_box(results.len())
                }
            });
        });
    }

    group.finish();
}

#[cfg(not(feature = "fulltext-search"))]
fn bench_fulltext_search(c: &mut Criterion) {
    let mut group = c.benchmark_group("fulltext_search");
    group.measurement_time(Duration::from_secs(10));
    group.sample_size(50);
    group.warm_up_time(Duration::from_secs(1));

    group.bench_function("search_rust", |b| b.iter(|| black_box(10usize)));
    group.finish();
}

fn bench_vector_distance(c: &mut Criterion) {
    let mut group = create_benchmark_group(c, "vector_distance");

    for dim in &[128, 256, 512] {
        group.bench_with_input(BenchmarkId::from_parameter(dim), dim, |b, &d| {
            let a: Vec<f32> = (0..d).map(|i| i as f32 * 0.1).collect();
            let bv: Vec<f32> = (0..d).map(|i| (i + 1) as f32 * 0.1).collect();
            b.iter(|| {
                let sum: f32 = a.iter().zip(bv.iter()).map(|(x, y)| (x - y).abs()).sum();
                black_box(sum)
            });
        });
    }

    group.finish();
}

criterion_group!(
    benches,
    bench_fulltext_index_build,
    bench_fulltext_search,
    bench_vector_distance,
);
criterion_main!(benches);
