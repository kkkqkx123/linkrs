# P1 [query] parallel_scale_bench 在 collector.rs:458 panic "writer must exist"

## 现象（实测）

`parallel_scale_bench` 在 E2 edge scan 阶段 panic，整包 `cargo bench` 失败：

```
thread 'main' (1268493) panicked at crates/linkrs-query/src/executor/streaming/chunk/collector.rs:458:50:
writer must exist
note: run with RUST_BACKTRACE=1 environment variable to display a backtrace
error: bench failed, to rerun pass `-p linkrs-query --bench parallel_scale_bench`
```

数据来源：`bench_results/linkrs-query.log:3406-3409`；整包 `bench_results/linkrs-query.exit` = `EXIT=101`。

- 前序 Q1/Q2/Q3/E1 已正常产出并行效率表（见 `query-parallel-efficiency.md`），**E2 触发 panic 导致后续中断**。
- 因此并行扩展类覆盖在 E2 之后全部缺失。

## 影响

**P1 —— 正确性 / 覆盖缺口**：并行扩展 bench 直接 panic 失败，留下并行执行路径的覆盖缺口；同时暴露 **collector/writer 生命周期缺陷**（writer 在并行收集尚未完成时即被访问）。

## 疑似根因

并行 chunk collector 在 E2（edge scan）路径下，writer 句柄未就绪 / 已被消费即被 collector 访问。日志中前序查询的 `fallback="partitioning is disabled"` 提示该路径走了非分区 fallback，writer 初始化顺序可能与分区路径不同，是疑点。

## 建议

1. 在 `collector.rs:458` 附近加固 writer 存在性检查 / 生命周期管理——即使异常也应返回明确错误而非 `panic!`，避免整包 bench 崩溃。
2. 用 `RUST_BACKTRACE=1` 取栈，定位 E2 路径 writer 初始化顺序为何晚于 collector 访问。
3. 修复后让 `parallel_scale_bench` 全量跑通（Q1–E2+），并纳入 CI 门禁，避免回归。

## 严重级
**P1（正确性 / bench 基础设施破裂）**

## 数据来源
`bench_results/linkrs-query.log:3370-3409`、`bench_results/linkrs-query.exit` = `EXIT=101`
