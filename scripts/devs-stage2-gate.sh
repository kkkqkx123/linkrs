#!/usr/bin/env bash
# Remote gate for the two-hop and variable-path count fast paths.
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo

echo "=== build parity bench ==="
cargo build --release --features embedded --bin linkrs-parity-bench > /data/kkkqkx/linkrs/stage2-build.log 2>&1
echo "BUILD_EXIT=$?"
tail -n 6 /data/kkkqkx/linkrs/stage2-build.log

echo "=== correctness gates ==="
cargo test -p linkrs-query --lib -- expand_pushdown::tests > /data/kkkqkx/linkrs/stage2-unit.log 2>&1
echo "UNIT_EXIT=$?"
tail -n 5 /data/kkkqkx/linkrs/stage2-unit.log
cargo test -p linkrs-query --test dql > /data/kkkqkx/linkrs/stage2-dql.log 2>&1
echo "DQL_EXIT=$?"
tail -n 5 /data/kkkqkx/linkrs/stage2-dql.log
cargo test -p linkrs-query --lib -- streaming > /data/kkkqkx/linkrs/stage2-streaming.log 2>&1
echo "STREAMING_EXIT=$?"
tail -n 5 /data/kkkqkx/linkrs/stage2-streaming.log
cargo test -p linkrs-query --test parallel_partition_execution > /data/kkkqkx/linkrs/stage2-parallel.log 2>&1
echo "PARALLEL_EXIT=$?"
tail -n 5 /data/kkkqkx/linkrs/stage2-parallel.log

BIN=target/release/linkrs-parity-bench
DB=/data/kkkqkx/linkrs/parity/lsqb-sf0.1-v4/db
OUT=/data/kkkqkx/linkrs/stage2-results
rm -rf "$OUT"
for round in 1 2; do
  echo "=== fixed gate round $round ==="
  taskset -c 16-31 "$BIN" run \
    --db "$DB" --space lsqb \
    --benchmark-dir benches/parity/queries/lsqb \
    --warmup 1 --run 5 --threads 16 --parallel-workers 16 \
    --skip-file benches/parity/skip.txt \
    --out "$OUT/round$round" > "/data/kkkqkx/linkrs/stage2-round$round.log" 2>&1
  echo "ROUND${round}_EXIT=$?"
  tail -n 5 "/data/kkkqkx/linkrs/stage2-round$round.log"
done
echo "STAGE2_DONE"
