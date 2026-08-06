# 问题：并行库声明但零使用（rayon/crossbeam 0 命中）

- 状态：新建（已验证，待修复）
- 类型：架构缺陷（依赖清单与实现能力不符）
- 来源：`docs/analysis/linkrs-vs-ladybug-存储并行对比分析.md` 缺陷 D（与"结论先行"一致）
- 关联：`docs/issue/defect-C-flush-under-catalog-write-lock.md`（并行落盘前提）、`docs/plan/parallel-extension-and-storage-rework-design.md`（既有并行分区计划）

## 问题描述

`crates/graphdb-storage/Cargo.toml:31-32` 声明 `crossbeam-utils` 与 `rayon`，但对整个 crate（src + tests）grep 结果为 **0 命中**：

```
$ grep -rn "rayon\|crossbeam" --include="*.rs" crates/graphdb-storage/ | wc -l
0
```

并行库被声明、被编译、被链接，但从未被调用。存储层**没有任何一行数据并行代码**。

## 根因分析（佐证：工作线程无池化）

- 存储层共 **15 处 `thread::spawn`**（其中生产代码 6 处）：`index_gc_manager.rs:299`、`shard_runtime.rs:1116`、`graph_storage/index_manager.rs:1156,1162`、`context/freeze.rs:47`、`vertex/gc_manager.rs:139`——每次新建 OS 线程，无池化、无背压；
- `freeze.rs:35-61`：门闩限定全局只允许 **1 个**后台维护线程，compaction/freeze 吞吐上限 = 单核，且线程句柄被丢弃，无法 join / 优雅关闭；
- `engine/background_freeze.rs:170` 的 `BackgroundFreezeManager` 名为 Manager，实际只维护 `Arc<Mutex<FreezeStats>>` 统计，**不启动任何线程**（注释与实现不符的典型，同 `data_store.rs:644-648` 注释所描述的锁粒度在代码中已兑现，但迭代仍是串行 `.map()`）。
- 并行机会点：scan / compaction / flush / `for_all_edge_partitions_mut` 等均为串行 `.map()`。

## 影响

- README 性能宣称缺少实现支撑，误导审查者与新贡献者；
- 存储层全部重操作单核执行：并行 flush、并行 compaction、并行扫描均不可用。

## 修复方向

1. **引入统一线程池**：替换 6 处生产 `thread::spawn`（冻结/GC/索引重建），支持 join 与优雅关闭；
2. **启用 rayon**：`for_all_edge_partitions_mut`、scan、flush、compaction 的串行 `.map()` 改 `par_iter()`；
3. **删除或使用 `crossbeam-utils`**：若接入 bounded MPSC / 信号量则保留，否则从依赖移除；
4. 后台维护门闩从"全局唯一"放宽为"按表粒度"或按资源预算并发。

详细方案见 `docs/plan/storage-concurrency-correctness-rework-design.md` P1-D。

## 验收

- 生产代码中不再出现裸 `thread::spawn`（grep 校验），全部走统一线程池；
- 至少 1 个重操作路径（flush 或 scan）启用 rayon 并测得并行加速；
- 依赖清单与实际使用一致（`cargo machete` 或等价检查通过）；
- 全量 `cargo test --test '*'` + clippy 全绿。
