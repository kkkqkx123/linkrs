# 问题：5 个 bench 的数据生成违反存储层校验契约（主键镜像 / 列名）

- 状态：新建（待修复）
- 类型：bench 基建缺陷（系统性）
- 复现：`cargo bench -p linkrs-storage` / `cargo bench -p linkrs-query`（2026-10-09）

## 问题描述

存储层要求 tag 的主键列值必须镜像顶点 id（vertex id）。BENCHMARK_REPORT.md 曾记录 olap_e2e 基准
因此修过一次数据生成（"name 是 tag 主键列，必须镜像顶点 id，原代码写入 p{i} 被主键镜像校验拒绝"），
但同一模式在其余 bench 中仍存在，导致以下 bench 在当前 HEAD **全部无法运行**：

| bench | 位置 | panic 信息（节选） |
|---|---|---|
| storage_bench | storage_bench.rs:706 | `name` got `"node_0"`, expected `"0"` |
| storage_workflow_bench | storage_workflow_bench.rs:27 | `name` got `"vertex_0"`, expected `"v0"` |
| query_bench | query_bench.rs:70 | `name` got `"node_0"`, expected `"n0"` |
| edge_scan_speedup_bench | edge_scan_speedup_bench.rs:88 | `value` got `BigInt(0)`, expected `BigInt(1000000)`（多分区时 `value` 写的是分区内行号 `i`，未加 `p * TOTAL_EDGES` 偏移） |
| storage_read_baseline | storage_read_baseline.rs:93 | `ColumnNotFound("v0")`——批列名与 schema 不符 |

且 `edge_scan_speedup_bench` 的 panic 会中断 `cargo bench -p linkrs-storage` 的默认执行顺序，
使其后的 7 个 storage bench 被连带跳过（本次已改为逐 bench 补跑）。

## 影响

- 34 个声明 bench 目标中 9 个无法产出数据（含本组 5 个），存储/查询层多条关键路径
  （bulk 写入基线、读基线、工作流、边扫描加速比）没有当前版本的数字。
- 失败模式是 panic 中断整包 bench，掩盖后续 bench 的结果。

## 修复方向

- 引入统一的 bench 数据生成 helper（如 `benches/common` 下 `make_vertices(tag, n)`，
  保证 PK 列 == vid 的字符串/整型镜像），替换 5 处各自手写的数据构造。
- 评估“主键必须镜像 vid”的契约本身：若业务需要自然键（如用户名），该约束过强，
  建议至少把校验错误信息改为同时给出 vid 与期望值规则（当前信息已足够定位，但语义怪异）。
- CI 中 `cargo bench --no-run --workspace` + 各包 bench smoke（最小 scale）作为门禁，
  避免 bench 与存储校验契约再次脱节。
