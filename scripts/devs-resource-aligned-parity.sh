#!/usr/bin/env bash
# Collect Ladybug/Neug parity numbers with the same CPU/NUMA and buffer
# settings as the Linkrs gate on devs.
set -eu

export PATH="$HOME/.cargo/bin:$PATH"
source /data/kkkqkx/graphdb-ben/bench-env.sh
BENCH_PY=/data/kkkqkx/graphdb-ben/.venv-bench/bin/python

BIND="numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31"
export BENCH_CPU_BINDING="$BIND"
export BENCH_NUMA_POLICY="cpunodebind=1,membind=1"

ROOT=/data/kkkqkx/linkrs
OUT="$ROOT/resource-aligned-parity"
DATA=/data/kkkqkx/graphdb-ben/data/social-network-sf0.1-projected-fk
QUERIES=/data/kkkqkx/linkrs/repo/benches/parity/queries/lsqb/operators
RUNNER=/data/kkkqkx/graphdb-ben/neug_runner.py

mkdir -p "$OUT"

echo "=== cpu binding: $BIND ==="
$BIND "$BENCH_PY" "$RUNNER" --engine ladybug \
  --data "$DATA" --queries "$QUERIES" --threads 16 --warmup 1 --run 5 \
  --ladybug-buffer-mb 64 \
  --db "$OUT/lbug-ops-db" --out "$OUT/lbug-ops.json" \
  > "$OUT/lbug-ops.log" 2>&1
echo "LADYBUG_EXIT=$?"

$BIND "$BENCH_PY" "$RUNNER" --engine neug \
  --data "$DATA" --queries "$QUERIES" --threads 16 --warmup 1 --run 5 \
  --neug-buffer-strategy M_FULL \
  --db "$OUT/neug-ops-db" --out "$OUT/neug-ops.json" \
  > "$OUT/neug-ops.log" 2>&1
echo "NEUG_EXIT=$?"

echo "RESOURCE_ALIGNED_PARITY_DONE"
