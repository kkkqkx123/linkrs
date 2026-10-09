# 基准迁移实施记录

日期：2026-10-09 ｜ 依据：`benches_migration_analysis.md`

## 已完成

根 `benches/` 28 个 `[[bench]]` 目标全部搬离，各包新增 `benches/` 与接线：

- linkrs-storage（13 个）：storage_bench（含并入的 indexed_bulk_load）、
  csr_perf、neighbor_batch、edge_property_tables、ingest（import＋
  edge_point_write 合并）、commit_gate（edge_group_commit＋write_gate 合并）、
  edge_scan_speedup、edge_read_alloc、storage_read_baseline、wal、
  crash_recovery、rollback、storage_workflow（end_to_end 改名）。
- linkrs-transaction（2 个）：transaction（删除与 txn_conflict 重复的
  conflict_detection 组）、txn_conflict。
- linkrs-query（6 个）：query（删除名不副实的 parse 组）、query_stage、
  traversal_perf、parallel_scale、olap_e2e、executor（operator＋
  accumulation＋columnar 合并）。
- linkrs-fulltext（1 个）：fulltext（search_bench 全文部分原样搬运）。
- linkrs-core（1 个）：serde（api_bench 改名，原内容即 core 类型编解码）。
- simvec（新增 1 个）：distance（以实际 `Kernel::distance` 重写，
  替代原 L1 手写桩）。
- 根 `Cargo.toml`：28 个 `[[bench]]` 声明全部删除；dev 依赖恢复原状，
  仅去掉不再使用的 `criterion`。
- 各包统一加 `autobenches = false`（`benches/` 内有非目标辅助模块，
  不能走自动发现）与 `criterion` dev 依赖。
- `linkrs::` 根立面导入全部改写为直接包导入；plain-main 文件内的旧运行
  命令注释改为 `-p` 形式；wal 注释中的 `import` 引用改为 `ingest`。
- 根 `benches/` 仅剩 `data/`、`results/`（历史封存）、`README.md`（重写）、
  `BENCHMARK_REPORT.md`（历史快照，未动）。

## 与分析的偏差

- rollback_bench 归属由 linkrs-transaction 改为 linkrs-storage：
  该基准依赖 `GraphStorage`，而 storage 已依赖 transaction，
  放入 transaction 会新增 transaction→storage 的 dev 边，破坏单向 DAG.
- query_stage_bench 同样引用 `results_report.rs`（初版清单遗漏），
  linkrs-query 的 `benches/` 下已补齐该辅助模块。
- 修复一处编译期耦合：`linkrs-storage/src/vertex/vertex_table.rs` 的
  R6 门禁测试用 `include_str!` 直引根目录 bench 源文件，
  路径已改为包内 `benches/storage_bench.rs`；三项门禁测试全过，
  合并后的 storage_bench 满足“每个 `bench_*` 函数必须注册”不变式。
- olap_e2e_bench 按计划归入 linkrs-query，未在根目录保留回归门引用；
  回归门约定改写在根 `benches/README.md` 中。

## 后续工作（本次未做）

- linkrs-server／linkrs-api 真实基准（HTTP 路由／序列化、gRPC 编解码、
  并发请求）：当前零覆盖，需新建。
- 存储读路径四套基准（scan 加速、批量邻居、cursor 组、read_baseline）
  与 CSR／表布局决策实验的深度合并：文件已就位，内容合并待后续做。
- query 两跳遍历四个版本的数据集统一（olap 固定种子 fixture 共享）。
- 真端到端基准重写（现状 storage_workflow 实为存储级工作流，未经过
  query／transaction／api）。
- `benches/results/` 历史报告按包归档；`benches/data/` 归属确认
  （暂留根目录作共享 fixture）。
