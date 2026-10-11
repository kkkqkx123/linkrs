# 基线 [core / transaction] 健康微基准（可回归对比值）

本文件记录本次跑通的**健康基线**，给出可供后续回归对比的基准数值。这两包未暴露扩展性或正确性缺陷。

## linkrs-core — serde 基线

`benches/serde_bench.rs`（`bench_results/core.log:475-506`）：

| bench | 中位时间 |
|---|---:|
| `json_serialization/serialize_vertex` | [278.46, 279.90, 281.41] ns |
| `json_serialization/serialize_100_vertices` | [23.428, 23.773, 24.116] µs |
| `json_deserialization/deserialize_vertex` | [667.98, 671.61, 675.64] ns |

- 100 顶点序列化 ~23.8µs（含开销约 238ns/顶点）。
- 健康，可作 serde 回归基线。

## linkrs-transaction — 事务基元

`benches/transaction_bench.rs`（`bench_results/linkrs-transaction.log:474-573`）：

| bench | 中位时间 |
|---|---:|
| `transaction_ops/begin_read` | [2.1169, 2.1220, 2.1278] µs |
| `write_set/build/10` | [639.71, 642.27, 645.22] ns |
| `write_set/build/100` | [5.7699, 5.7917, 5.8150] µs |
| `write_set/build/1000` | [69.325, 69.565, 69.823] µs |
| `write_set/conflict_check/1000` | [9.6407, 11.299, 12.905] µs |
| `mvcc/acquire_read_ts` | [207.46, 207.90, 208.45] ns |
| `mvcc/acquire_write_ts` | [347.90, 348.56, 349.26] ns |

- `write_set/build` 10→100→1000：645ns → 5.79µs → 69.6µs，约线性偏超线性（100× 数据 → 108× 时间），无异常爆炸。
- `begin_read` 2.12µs、`mvcc` 时间戳获取 <350ns——事务基元健康。

## 注意

- 本次**未跑多线程 contention bench**。既有 `docs/issue/`（2026-10-09）中 "contention c1→c8 34µs→189µs 退化" 的声明**本次未重新测量**，留待真机（≥8 核）复核。
- 所有数值在 cgroup 4 核配额下取得，绝对时延受调度影响，但微基准量级稳定，适合做相对回归对比。

## 严重级
**信息 / 基线（健康）**

## 数据来源
`bench_results/core.log:475-506`、`bench_results/linkrs-transaction.log:474-573`
