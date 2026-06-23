# GraphDB Performance Benchmarks

Performance benchmark suite for GraphDB using Criterion.rs

## Directory Structure

```
benches/
├── common/                           # Shared utilities
│   ├── mod.rs                       # Module exports
│   ├── data_generator.rs            # Data generation utilities
│   ├── bench_utils.rs               # Benchmark helper functions
│   └── test_context.rs              # Test context setup
├── data/                            # Benchmark data (GQL files)
│   ├── generate_benchmark_data.py   # Data generation script
│   └── bench_*.gql                  # Generated data files
├── lib.rs                           # Library exports
├── storage_bench.rs                 # Storage layer benchmarks
├── transaction_bench.rs             # Transaction layer benchmarks
├── query_bench.rs                   # Query engine benchmarks
├── search_bench.rs                  # Search (fulltext + vector) benchmarks
├── api_bench.rs                     # API layer benchmarks
├── end_to_end_bench.rs             # End-to-end workflow benchmarks
└── README.md                        # This file
```

## Running Benchmarks

### Run All Benchmarks

```bash
cargo bench
```

### Run Specific Benchmark Suite

```bash
# Storage layer benchmarks
cargo bench --bench storage_bench

# Transaction layer benchmarks
cargo bench --bench transaction_bench

# Query engine benchmarks
cargo bench --bench query_bench

# Search (fulltext + vector) benchmarks
cargo bench --bench search_bench

# API layer benchmarks
cargo bench --bench api_bench

# End-to-end workflow benchmarks
cargo bench --bench end_to_end_bench
```

### Run Specific Benchmark

```bash
# Run only vertex insert benchmarks
cargo bench -- vertex_insert

# Run only query parsing benchmarks
cargo bench -- query_parse

# Use wildcard patterns
cargo bench -- 'storage*'
```

### Save and Compare Results

```bash
# Save current benchmark results as baseline
cargo bench -- --save-baseline=v1_0

# Compare against baseline
cargo bench -- --baseline=v1_0

# Save with custom profile
cargo bench --release -- --save-baseline=release_v1_0
```

### Benchmark Options

```bash
# Verbose output
cargo bench -- --verbose

# Increase sample size for more stability
cargo bench -- --sample-size 200

# Longer measurement time
cargo bench -- --measurement-time 30

# Generate plots
cargo bench --features "plots"
```

## Benchmark Data Generation

### Generate Benchmark Data

```bash
# Generate all benchmark data
python3 benches/data/generate_benchmark_data.py --type all

# Generate specific benchmark data
python3 benches/data/generate_benchmark_data.py --type storage --vertices 10000 --edges-per-vertex 5
python3 benches/data/generate_benchmark_data.py --type query --vertices 10000
python3 benches/data/generate_benchmark_data.py --type transaction --vertices 5000
python3 benches/data/generate_benchmark_data.py --type fulltext --documents 10000
python3 benches/data/generate_benchmark_data.py --type vector --vectors 10000 --dimensions 256
```

### Custom Data Generation

```bash
# Generate large-scale benchmark data
python3 benches/data/generate_benchmark_data.py --type storage \
    --vertices 100000 \
    --edges-per-vertex 10

# Generate high-dimensional vector data
python3 benches/data/generate_benchmark_data.py --type vector \
    --vectors 100000 \
    --dimensions 768
```

## Benchmark Suites Overview

### Storage Layer (`storage_bench.rs`)

Tests performance of vertex and edge storage operations:

- **Vertex Insert**: Single and batch vertex insertion (10, 100, 1000 vertices)
- **Edge Insert**: Edge creation with varying per-vertex count (1, 5, 10)
- **Data Generation**: GQL string generation performance

**Performance Targets**:
- Single vertex insert: <0.5ms
- Single edge insert: <0.5ms
- Batch insert throughput: >20k ops/s

### Transaction Layer (`transaction_bench.rs`)

Tests transaction management and MVCC:

- **Transaction Operations**: Create, commit, rollback
- **Batch Operations**: 10, 100, 1000 operations per transaction
- **MVCC Version Management**: Version chain traversal (1, 10, 100 versions)
- **Conflict Detection**: Write conflict detection overhead
- **Isolation Levels**: Performance of different isolation levels

**Performance Targets**:
- Transaction commit: <0.2ms
- 100-operation transaction: <10ms
- Concurrent reads (8 threads): >80k ops/s

### Query Engine (`query_bench.rs`)

Tests query execution performance:

- **Simple Query Parsing**: Basic vertex and edge queries
- **Data Access**: Vertex retrieval at different scales
- **Path Traversal**: 2-hop, 3-hop, 5-hop path queries
- **Aggregation**: Count, sum, average operations

**Performance Targets**:
- Simple vertex query: <1ms
- 2-hop path query: <10ms
- 3-hop path query: <100ms

### Search Layer (`search_bench.rs`)

Tests fulltext and vector search performance:

- **Fulltext Index Building**: Index creation at different scales (100, 1k, 10k documents)
- **Fulltext Queries**: Keyword search performance
- **Fulltext Scaling**: Search performance on larger datasets
- **Vector Index Building**: Vector index construction (128d, 256d, 512d)
- **Vector Distance Calculation**: Distance metric computation
- **Vector Top-K Search**: Nearest neighbor retrieval (K=10, 100, 1000)

**Performance Targets**:
- Fulltext search: <100ms
- Vector search (K=10): <50ms
- Vector search (K=100): <100ms

### API Layer (`api_bench.rs`)

Tests HTTP and gRPC API performance:

- **HTTP Request Parsing**: JSON parsing overhead
- **HTTP Response Serialization**: Response generation
- **gRPC Encoding**: Protobuf encoding/decoding
- **Concurrent Requests**: Multi-threaded request handling
- **Request Routing**: URL routing performance
- **Authentication**: Token verification and permission checking
- **Request Validation**: Query and schema validation

**Performance Targets**:
- HTTP API request: <2ms
- gRPC request: <1ms
- Concurrent (100 reqs): P99 <100ms

### End-to-End (`end_to_end_bench.rs`)

Tests complete workflows:

- **Data Loading**: Loading 1k and 10k vertices with edges
- **Query Analysis**: Simple and path queries on loaded data
- **Search Workflow**: Fulltext and vector search on large datasets
- **Write Transactions**: Insert and update workflows
- **Concurrent Workload**: 8-thread read-write mix

## Performance Data Location

Generated benchmark results are saved to:

```
target/criterion/
├── report/
│   └── index.html          # HTML report with graphs
├── [benchmark_name]/
│   ├── base/
│   │   └── raw.json        # Raw benchmark data
│   └── comparison.json     # Comparison with baseline
```

Open the HTML report:

```bash
open target/criterion/report/index.html
```

## Best Practices

### Writing New Benchmarks

1. **Use `black_box`** to prevent compiler optimizations:
   ```rust
   b.iter(|| black_box(operation()))
   ```

2. **Separate setup from iteration**:
   ```rust
   let data = setup();
   b.iter(|| operation(&data));  // ✓ Correct
   b.iter(|| {                   // ✗ Wrong
       let data = setup();
       operation(&data);
   });
   ```

3. **Use appropriate measurement time**:
   ```rust
   group.measurement_time(Duration::from_secs(10));  // Accurate results
   ```

4. **Include warm-up**:
   ```rust
   group.warm_up_time(Duration::from_secs(1));  // Warm up CPU, caches
   ```

### Interpreting Results

- **Latency**: Mean, P50, P95, P99 times (lower is better)
- **Throughput**: Operations per second (higher is better)
- **Coefficient of Variation (CV)**: Stability metric (< 5% is good)

If results are unstable (CV > 10%):
- Close background processes
- Fix CPU frequency with `cpupower`
- Increase sample size

## Continuous Integration

Benchmarks can be integrated into CI/CD:

```bash
# Compare with main branch
git fetch origin main
cargo bench -- --save-baseline=main
cargo bench -- --baseline=main

# Fail if performance regresses >5%
# (Requires additional scripting)
```

## Performance Analysis Tips

1. **Run in Release Mode**: Always use `cargo bench --release`
2. **Monitor System**: Use `htop`, `iostat` while benchmarking
3. **Repeat Runs**: Run multiple times to check consistency
4. **Profile Hot Paths**: Use `cargo flamegraph` for detailed analysis

```bash
# Generate flame graph for a benchmark
cargo flamegraph --bench storage_bench -- --profile-time 10

# View the result
open flamegraph.svg
```

## Data Files

Pre-generated benchmark data files:

| File | Vertices/Docs | Size | Use Case |
|------|--------------|------|----------|
| `bench_storage_1000v_5e.gql` | 1k vertices, 5k edges | 429KB | Storage layer |
| `bench_query_1000v.gql` | 1k vertices | 235KB | Query engine |
| `bench_transaction_1000v.gql` | 1k vertices | 56KB | Transactions |
| `bench_fulltext_1000d.gql` | 1k documents | 287KB | Fulltext search |
| `bench_vector_1000v_128d.gql` | 1k 128d vectors | 1.2MB | Vector search |

## Common Issues

### Benchmark Results Vary Widely

**Solution**: Ensure consistent system state
- Close unnecessary applications
- Disable frequency scaling: `cpupower frequency-set -g performance`
- Increase sample size: `cargo bench -- --sample-size 200`

### Compilation Too Slow

**Solution**: Use release optimizations
- `cargo bench --release`
- Pre-compile with `cargo build --release`

### Memory Issues with Large Benchmarks

**Solution**: Reduce data size or run separately
- Run individual benchmarks: `cargo bench --bench storage_bench`
- Reduce vertex count: `--vertices 1000` instead of `100000`

## References

- [Criterion.rs Documentation](https://bheisler.github.io/criterion.rs/book/)
- [Rust Performance Book](https://nnethercote.github.io/perf-book/)
- [GitHub Benchmark Results](../docs/tests/benches/)

## Contributing

When adding new benchmarks:

1. Add benchmark to appropriate `*_bench.rs` file
2. Update data generator if needed
3. Document target performance in module doc comment
4. Run and record baseline results
5. Update this README

---

**Last Updated**: 2026-06-18  
**Maintained By**: GraphDB Team
