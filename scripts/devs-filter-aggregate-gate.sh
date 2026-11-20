#!/usr/bin/env bash
# Build and measure the scan-predicate and aggregate-filter paths on devs.
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo

ROOT=/data/kkkqkx/linkrs
OUT="$ROOT/filter-aggregate-results"
rm -rf "$OUT"
mkdir -p "$OUT"

echo "=== build parity bench ==="
cargo build --release --features embedded --bin linkrs-parity-bench > "$ROOT/filter-aggregate-build.log" 2>&1
echo "BUILD_EXIT=$?"
tail -n 8 "$ROOT/filter-aggregate-build.log"

echo "=== correctness gates ==="
cargo test -p linkrs-storage --lib > "$OUT/storage-all.log" 2>&1
echo "STORAGE_EXIT=$?"
tail -n 5 "$OUT/storage-all.log"
cargo test -p linkrs-query --lib -- scan_predicate > "$OUT/unit-scan-predicate.log" 2>&1
echo "SCAN_PREDICATE_EXIT=$?"
tail -n 5 "$OUT/unit-scan-predicate.log"
cargo test -p linkrs-query --lib -- push_filter_down_scan_vertices > "$OUT/unit-pushdown.log" 2>&1
echo "PUSHDOWN_EXIT=$?"
tail -n 5 "$OUT/unit-pushdown.log"
cargo test -p linkrs-query --test dql > "$OUT/dql.log" 2>&1
echo "DQL_EXIT=$?"
tail -n 5 "$OUT/dql.log"
cargo test -p linkrs-query --lib -- streaming > "$OUT/streaming.log" 2>&1
echo "STREAMING_EXIT=$?"
tail -n 5 "$OUT/streaming.log"
cargo test -p linkrs-query --test parallel_partition_execution > "$OUT/parallel.log" 2>&1
echo "PARALLEL_EXIT=$?"
tail -n 5 "$OUT/parallel.log"

# Keep the Linkrs-only shortest path case out of the parity table; the
# aggregate FILTER case is measured locally with the same runner.
SKIP="$OUT/skip-filter-gate.txt"
printf '%s\n' \
  'lsqb/operators/shortest_path Linkrs-only dialect is rejected by Ladybug and Neug' \
  > "$SKIP"

BIN=target/release/linkrs-parity-bench
DB="$ROOT/parity/lsqb-sf0.1-v4/db"
for round in 1 2; do
  echo "=== fixed gate round $round ==="
  taskset -c 16-31 "$BIN" run \
    --db "$DB" --space lsqb \
    --benchmark-dir benches/parity/queries/lsqb \
    --warmup 1 --run 5 --threads 16 --parallel-workers 16 \
    --skip-file "$SKIP" \
    --explain-dir "$OUT/explain$round" --profile-runs 1 \
    --out "$OUT/round$round" > "$OUT/round$round.log" 2>&1
  echo "ROUND${round}_EXIT=$?"
  awk '/filter_id_range|filter_or|filter_count|aggregate_filtered_count|VmHWM/' "$OUT/round$round.log"
done
echo "FILTER_AGGREGATE_GATE_DONE"
