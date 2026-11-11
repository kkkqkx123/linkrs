# parity 基准套件

对等 Ladybug 公开数据集基准。方法论、口径与实施顺序见
`docs/release/benchmark/parity-benchmark.md`（权威文档）；边类型端点约束等
持久化要求见其 §4.4。

## 目录

- `schema/` — DDL + COPY 脚本（`linkrs-parity-bench copy` 消费）
- `queries/` — 与 Ladybug `.benchmark` 同构的查询文件
- `skip.txt` — 无法语义对齐、对比表标 N/A 的查询清单

## 用法

```shell
# 1. 导入（表头自动改写到 sidecar，原始 CSV 不动；version.txt 相同则跳过）
cargo run --release --features embedded --bin linkrs-parity-bench -- copy \
  --dataset lsqb-sf0.1 \
  --csv-dir graphdb-ben/data/social-network-sf0.1-projected-fk \
  --script benches/parity/schema/lsqb.cypher \
  --db /tmp/linkrs-parity/lsqb-sf0.1/db

# 2. 跑查询（warmup 1 + run 3，LSQB 口径；run 模式会先 USE <space>）
cargo run --release --features embedded --bin linkrs-parity-bench -- run \
  --db /tmp/linkrs-parity/lsqb-sf0.1/db \
  --space lsqb \
  --benchmark-dir benches/parity/queries/lsqb \
  --warmup 1 --run 3 \
  --out /tmp/linkrs-parity/results/lsqb-sf0.1
```

## 查询文件格式

```
----name
lsqb/q1
----query
MATCH (n:Person) RETURN count(n)
----expected
1700
```

`----expected` 缺省时只记录可执行与行数；存在时逐行比对，不一致记 `mismatch`。
