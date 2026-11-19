#!/usr/bin/env bash
# Remote gate for the fused two-hop neighbor-degree read.
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo

echo "=== build parity bench ==="
cargo build --release --features embedded --bin linkrs-parity-bench > /data/kkkqkx/linkrs/readfusion-build.log 2>&1
echo "BUILD_EXIT=$?"
tail -n 6 /data/kkkqkx/linkrs/readfusion-build.log

echo "=== correctness gates ==="
cargo test -p linkrs-storage --test neighbor_degree_batch > /data/kkkqkx/linkrs/readfusion-storage-final.log 2>&1
echo "STORAGE_EXIT=$?"
tail -n 5 /data/kkkqkx/linkrs/readfusion-storage-final.log
cargo test -p linkrs-query --lib -- expand_pushdown::tests > /data/kkkqkx/linkrs/readfusion-unit.log 2>&1
echo "UNIT_EXIT=$?"
tail -n 5 /data/kkkqkx/linkrs/readfusion-unit.log
cargo test -p linkrs-query --test dql > /data/kkkqkx/linkrs/readfusion-dql-final.log 2>&1
echo "DQL_EXIT=$?"
tail -n 5 /data/kkkqkx/linkrs/readfusion-dql-final.log
cargo test -p linkrs-query --lib -- streaming > /data/kkkqkx/linkrs/readfusion-streaming.log 2>&1
echo "STREAMING_EXIT=$?"
tail -n 5 /data/kkkqkx/linkrs/readfusion-streaming.log
cargo test -p linkrs-query --test parallel_partition_execution > /data/kkkqkx/linkrs/readfusion-parallel.log 2>&1
echo "PARALLEL_EXIT=$?"
tail -n 5 /data/kkkqkx/linkrs/readfusion-parallel.log

BIN=target/release/linkrs-parity-bench
DB=/data/kkkqkx/linkrs/parity/lsqb-sf0.1-v4/db
OUT=/data/kkkqkx/linkrs/readfusion-results
rm -rf "$OUT"
for round in 1 2 3 4; do
  echo "=== fixed gate round $round ==="
  taskset -c 16-31 "$BIN" run \
    --db "$DB" --space lsqb \
    --benchmark-dir benches/parity/queries/lsqb \
    --warmup 1 --run 5 --threads 16 --parallel-workers 16 \
    --skip-file benches/parity/skip.txt \
    --out "$OUT/round$round" > "/data/kkkqkx/linkrs/readfusion-round$round.log" 2>&1
  echo "ROUND${round}_EXIT=$?"
  tail -n 3 "/data/kkkqkx/linkrs/readfusion-round$round.log"
done
echo "READFUSION_DONE"
