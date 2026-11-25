# 已编译物理计划复用：细化实施与验证方案

## 1. 结论与范围

设计文档的目标是让同一会话内重复执行的只读查询跳过 bind、逻辑规划、优化与物理构建，只保留 parse、key 构造、checked-serve 与执行。现有 `compile_or_get_cached` 对非分区查询已经有早于 bind 的 plain plan cache 命中路径；缺口在分区查询：分区指纹只能在优化后得到，因此重复查询仍要付出约 0.84 ms 的优化成本。

本次只在 `linkrs-query` 流水线新增一个只读已编译计划 memo，专门覆盖分区计划；非分区只读查询继续走现有 plain cache，DML 继续走既有 shape memo。新 memo 位于编译入口之前，命中后复用现有 `QueryPlanCache::serve_checked` 单一校验出口；失败一律重新编译，不降级 serving 过期计划。结果数据永不进入任何缓存。

## 2. 缓存身份

memo key 由以下维度组成，任意一个变化都会 miss：

- 规范化查询文本、空间名、schema 版本、index 版本、参数类型签名；
- 优化器 cost epoch、partitioning 配置哈希（复用 `PlanCacheContext`，与 plain plan cache 同源）；
- 是否存在 staged write（`in_write_context`）；
- storage 的单调 layout version；
- 统计刷新代次：每次实际/强制 `collect_statistics` 递增，覆盖统计刷新与显式 ANALYZE；
- 优化反馈指纹：对已 fold 的 feedback 序列水印做确定性哈希，覆盖反馈写入导致的优化决策变化。

key 不含参数值、不含结果数据、不含分区 range 指纹。分区 range 在 bind 后由统计和反馈决定；因此统计/反馈的代次进入 key，布局变化由 layout version 和 checked-serve 双重兜底。正常读与 staged-write 读因 `in_write_context` 分离，不允许互相复用。

## 3. 修改点

### 3.1 Optimizer

在 `OptimizerEngine` 暴露只读的反馈状态指纹：把每个 query fingerprint 的已 fold 序列号排序后哈希，确保只在反馈真正被应用时改变，而不是每次 feedback history 追加样本都让缓存失效。

### 3.2 QueryPipelineManager

新增有界 LRU memo（容量与 DML memo 对齐）、命中计数、校验失败计数和统计刷新代次。增加三个内部操作：

- 构造统一 key：从 `plan_cache_context`、统计 stamp/代次、反馈指纹、storage layout version 派生；
- 探测：线性扫描 + move-to-front，仅用于 prepare 阶段的 bind-skip 判定；
- 命中/存储：命中走 `serve_checked`，失败计数并回退；冷编译后只存储带 `PartitionSpec` 的只读计划。

`clear_plan_cache` 与 DDL 失效路径同步清空 memo。统计强制刷新递增代次；自动统计刷新同样递增代次，但因为 hot path 不触发统计刷新，命中查询不会自己把缓存打掉。

### 3.3 编译与 prepare 流程

`compile_or_get_cached` 在 DML memo 和 plain cache 之前探测新 memo；命中时 `CompileTimes` 报告 cache hit、plan/optimize/statistics 为零，仅记录 probe 时间。`prepare_request` 在 bind 之前只做非副作用的存在性探测：命中则跳过 bind，实际 serve 与计数仍由编译路径完成，避免两次计数。`ANALYZE`、`EXPLAIN`、DDL、DML 等非目标语句不进入 memo。

### 3.4 指标与 bench

新增 `compiled_plan_memo_hit` 布尔位贯穿 `CompileTimes`、`StageMetrics`、API `ExecutionMetadata`/`ResultMetadata`，使 parity bench JSON 能区分普通 plan cache 命中与新 memo 命中。流水线与 QueryApi 暴露累计命中数、校验失败数和 memo 长度；普通 `plan_cache_hits` 仍包含新 memo 命中（`serve_checked` 统一记录），两者可交叉核对。bench JSON 增加 `compiled_plan_memo_hits` 字段。

## 4. 失效注入点

- DDL：schema/index generation 变化且 memo 被清空；
- 显式 ANALYZE 或自动统计刷新：统计代次变化；
- 反馈实际 fold：反馈指纹变化；
- 分区/代价配置变化：cost epoch 与 planning config hash 变化；
- staged write 与普通读切换：`in_write_context` 变化；
- 参数类型变化：参数签名变化；
- layout version 变化：key 变化，且 checked-serve 再校验一次。

所有 miss 都进入完整 bind/plan/optimize/build 路径；不得通过放宽 key 或跳过校验来保持命中。

## 5. 验证方案

### 5.1 本地静态检查

修改完成后执行 `cargo fmt --check`，并用 `git diff --check` 排除空白错误。不做本地全量编译，按 `docs/dev/build/devs-sync.md` 将改动文件同步到 devs。

### 5.2 devs 编译与定向测试

使用 `scripts/devs-compiled-plan-memo.sh` 单脚本顺序执行，日志落盘 `/data/kkkqkx/linkrs/compiled-plan-memo.log`：

1. `cargo check -p linkrs-query -p linkrs-api -p linkrs`；
2. `cargo test -p linkrs-query --test compiled_plan_memo -- --nocapture`：覆盖分区热查询命中与 bind-skip、结果逐值一致、DDL/ANALYZE 失效后重编译、统计代次与反馈指纹维度；
3. `cargo test -p linkrs-query --test dql`、`--test parallel_partition_execution`、`--test dml`：分区、TopK、join、DQL 与既有 DML memo 回归；
4. `cargo test -p linkrs-query --lib -- partitioning local_topk feedback`；
5. `cargo test -p linkrs-api --lib` 与 `cargo test -p linkrs-query --lib -- cache`；
6. `cargo build --release --features embedded -p linkrs --bin linkrs-parity-bench`，确认 bench JSON 新字段可比。

### 5.3 A/B 与验收判据

在同一台 devs、同一数据集上分别以 `LINKRS_NO_COMPILED_PLAN_MEMO=1`（诊断开关，禁用新 memo）和默认配置跑 parity 目标查询。冷执行计划形状/结果与基线一致；热执行结果逐值一致。默认配置下热查询 `compile_median_ms` 与 `optimize_median_us` 应降到 probe 量级，`plan_cache_hits` 与新增 memo 命中数一致。失效注入（DDL、ANALYZE、反馈 fold、配置切换）后的下一次执行必须重新编译并获得正确结果。

## 6. 已执行验证记录

devs 上 `scripts/devs-compiled-plan-memo.sh` 全部步骤退出码为 0：workspace 相关 crate `cargo check`、新 memo 定向测试（2/2）、DQL（324）、分区并行（27）、DML（229）、优化器/cache/API lib 测试以及 release parity bench 构建均通过。日志位于 `/data/kkkqkx/linkrs/compiled-plan-memo.log`。

同一 devs、lsqb sf0.1、warmup 1 / run 5 / threads 16 的 A/B 结果（默认 vs `LINKRS_NO_COMPILED_PLAN_MEMO=1`，输出目录 `/data/kkkqkx/linkrs/compiled-memo-bench/{default,off}`）：

| 查询 | 默认 compiled_median_ms | 关闭 memo compiled_median_ms | 默认 memo 命中 | 结果一致 |
|---|---:|---:|---:|---|
| distinct_limit | 0.017 | 0.706 | 5/5 | 是 |
| group_by_count | 0.060 | 2.154 | 5/5 | 是 |
| filter_count_pushed | 0.017 | 0.016 | 0/5 | 是 |
| join_two_hop | 0.025 | 0.020 | 0/5 | 是 |

非分区查询继续走原有 plain cache（两模式 `plan_cache_hits` 均为 5/5，memo 命中为 0），分区查询热执行从优化成本降到 probe 量级，且与关闭 memo 的结果逐值一致。
