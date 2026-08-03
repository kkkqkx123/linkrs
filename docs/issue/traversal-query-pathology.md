# 问题：图遍历查询路径病态慢（无锚 2-hop 单次 >5min）

- 状态：新建（待评估）
- 类型：性能缺陷（图遍历/Expand 路径）
- 关联：`docs/archive/benches/phase3-parallel-storage-validation.md` §3/§4

## 问题描述

100k 顶点 × 3 边（300k 边）数据上：

- 无锚 2-hop `MATCH (a:Node)-[:Link]->(b:Node)-[:Link]->(c:Node) RETURN count(c)`：单次执行 >5 分钟（终止），预期 ~90 万条路径应在秒级
- 锚定 1-hop `MATCH (a:Node)-[:Link]->(b:Node) WHERE a.value < 100 RETURN count(b)`：单次 5.15s（300k 邻居读取），端到端存储读占比 R=92%

## 分析

- 锚定 1-hop 每查询 5.15s ≈ 30 万次邻居读取，单次邻居读取（`Expand`/`get_node_edges` → 行式 `Value` 物化）约 10µs 级，且 R=92% 表明耗时集中在存储读而非查询计算
- 无锚 2-hop 的耗时远超 1-hop 的 3 倍线性外推，存在二次放大（候选：中间行物化/去重/连接实现病态，或谓词未在展开前应用）
- 属性过滤（`WHERE a.value < 100`）未明显缩小展开范围：绑定计划仍对全量 `a` 展开（过滤在展开后求值），谓词下沉（Phase 2 P2.4）与属性列块（Phase 3 A1）均能缓解

## 影响

- 图遍历型查询无锚执行不可用；作为并行评估对照基线（规划器白名单拒绝分区）耗时过长，基准不可承受

## 修复方向（暂列，未立项）

- 复查 2-hop 物理计划（EXPLAIN 展开形态），定位二次放大来源（中间行物化/连接实现）
- 谓词下沉到扫描/展开（与 Phase 2 P2.4 联动）
- Expand 路径列块化 / 属性裁剪（与 Phase 3 A1 联动，目标 R 从 92% 下降）
- 验证：锚定 1-hop 单次 ≤ 100ms；无锚 2-hop（100k×3）单次 ≤ 2s
