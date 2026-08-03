# 问题：批量顶点插入 O(n²)（数据装载退化）

- 状态：新建（待修复）
- 类型：性能缺陷（数据装载路径）
- 关联：`docs/archive/benches/phase3-parallel-storage-validation.md` §4

## 问题描述

`batch_insert_vertices` 耗时随表规模近似二次增长：10k 顶点块 1.2s，170k 顶点块 358s（200k 总耗时 510s）。对比：批量边插入线性（600k 边 1.6s）。影响数据装载、基准数据准备、大库导入。

## 实测数据（release 编译，内存存储）

| 已插入顶点数 | 10k 块耗时 |
|--------------|-----------|
| 10k | 1.2 s |
| 50k | 14.3 s |
| 90k | 91.9 s |
| 130k | 220.8 s |
| 170k | 358.5 s |

每行插入成本随表规模线性增长（≈O(n²) 总量），指向逐行插入路径中存在随表规模增长的线性扫描（候选：IdIndexer 插入 / 顶点 ID 去重 / live_ids 维护 / 快照或索引更新）。

## 影响

- 100k+ 顶点批量装载耗时分钟级（基准准备 6 分钟仅 100k 顶点）
- 与写入路径基准（`storage_bench` bulk 10k ≈ 270ms）的期望量级不符

## 修复方向

- 定位 `storage/engine/graph_storage/writer.rs` `batch_insert_vertices` → `insert_vertex_at_timestamp` 插入链路的逐行线性开销（顶点表 / IdIndexer），批量阶段去重与 ID 解析
- 验收目标：100k 批量插入 ≤ 10s（当前 ~370s），600k 边保持线性
