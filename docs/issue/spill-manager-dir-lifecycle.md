# 问题：SpillManager 目录生命周期竞争——同 query_id 实例互删 spill 目录

- 状态：新建（待修复）
- 类型：并发缺陷（查询执行 spill 路径）
- 复现：`cargo bench -p linkrs-query --bench parallel_scale_bench`（2026-10-09，warmup 阶段即失败）

## 问题描述

parallel_scale_bench.rs:288 warmup 查询失败：

```text
create run file: No such file or directory (os error 2)
```

根因链：

- `SpillManager::new_with_quota`（crates/linkrs-query/src/executor/streaming/spill.rs:972-986）
  在 `temp_dir/linkrs_spill_{query_id}` 上 `create_dir_all`——目录创建本身成功；
- 但 `impl Drop for SpillManager`（spill.rs:1116-1120）**无条件** `remove_dir_all(&self.base_dir)`；
- 执行引擎两处以 `runtime.query_id().query_id` 为 key 创建 manager
  （engine.rs:104-108、engine.rs:452-456）。当同一 query_id 存在多个 manager 实例
  （并行 pipeline / 引擎克隆 / 前序查询 manager 晚释放）时，
  先 Drop 的实例会把仍在使用的实例的整个目录连同 spill 文件删掉，
  后续 `create_run_writer`（spill.rs:1007 `File::create`）即 ENOENT。

parallel_scale_bench（10 万顶点并行流水线）在第一次 warmup 即触发，稳定复现。

## 影响

- 并行 scale 场景下任何 spill 都可能随机失败；parallel_scale_bench 完全无法运行，
  写路径并行扩展性没有基线。
- 该缺陷在生产多阶段查询（同 query_id 复用）下同样成立，属于数据面缺陷而不仅是 bench 问题。

## 修复方向

- Drop 改为“仅删除本实例创建的 run 文件；目录为空才 remove_dir”，
  或用引用计数（Arc<SpillDirGuard>）管理目录生命周期。
- `create_run_writer` 对 ENOENT 做一次 `create_dir_all(base)` 自愈重试，
  作为兜底并打 warning。
- 修复后重跑 parallel_scale_bench 补齐写并行基线（threads = [1,2,4,8]）。
