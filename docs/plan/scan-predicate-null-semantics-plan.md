# 扫描谓词 NULL 语义修复方案

**日期**：2026-11-20  
**依据**：`docs/issue/linkrs-remaining-parity-issues.md` 第 22 节与本轮实测撤回记录。本文件只描述问题成因与修改方式。

## 1. 结论摘要

当前过滤计数掩码路径在其窄前置内语义等价，保留。扫描谓词链路仍有一个既有缺陷：`ScanPredicate` 一律拒绝 NULL，而查询 evaluator 中 NULL 排在标量之前，`<`、`<=` 以及等价的 `literal > property`、`literal >= property` 会把 NULL 行判为符合条件。扫描预过滤提前丢弃这些行，上层 `Filter` 无法挽回。

修复方式是**让存储谓词遵循与 evaluator 相同的 NULL 排序**，而不是删除谓词或删除上层 `Filter`：

- `ScanPredicate` 对上界区间的 NULL 返回 true，等价于归一化后的 `<` / `<=`；
- zone-map 剪枝对上界区间保持保守，不因非 NULL 边界丢弃可能含 NULL 的块；
- 边表按列过滤把缺失列与 NULL 单元交给同一个 NULL 判定，索引候选路径遇到上界区间退回段扫描；
- 完整谓词替换继续关闭，直到存储侧证明谓词在所有扫描形态下都是权威过滤。

曾尝试只做“逻辑统计 / 物理过滤”拆分，但物理谓词是否执行会改变扫描算子的实际行数，进而改变反馈学习结果；范围查询因此再次退回串行。修存储语义可以保持物理行数和计划形状不变。

## 2. 两次失败尝试的成因

### 2.1 完整谓词替换

把残余 `Filter` 删除、让扫描谓词单独决定行可见性，在三类形状上出现数值回归：

| 形状 | 结果 | 成因 |
|---|---|---|
| identity/flat 投影扫描 | 期望 14 行，得到 20 行 | 该扫描路径没有把扫描谓词作为精确行过滤执行，上层 `Filter` 仍在做最终裁决 |
| TTL 表 | 期望 1 行，得到 0 行 | TTL/可见性判定与谓词列解码、版本读取交织，预过滤误删存活行 |
| Int 属性与浮点字面量比较 | 优化后 3 行，未优化 2 行 | 存储侧比较与表达式 evaluator 的类型提升/边界语义不一致，残余 `Filter` 原本会清掉假阳性 |

这些结果说明 `ScanPredicate` 当前只是预过滤，不具备替代上层 `Filter` 的资格。问题不在优化器删除动作的写法，而在存储侧谓词语义尚未统一。要放行完整替换，必须先逐路径审计 zone-map 剪枝、列式扫描、identity/flat、TTL/MVCC、混合数值类型和缺失属性，并补 pushed/unpushed 差分测试。

### 2.2 缩小 `<` / `<=` 下推白名单

把 `<`、`<=` 从 `pushable_conjuncts` 删除在 NULL 语义上是正确的，但带来计划回归：范围查询 `WHERE n.value < 800` 失去扫描节点上的谓词统计，估计行数从满足经济门的值降到 758，`feedback_does_not_learn_partition_bias_from_range_slices` 看到 `below the partition threshold`，计划退回串行。

因此该失败不是语义修正被证伪，而是统计/反馈与物理下推耦合在一起。简单停止物理下推仍会改变扫描算子的实际行数，反馈样本随之变化。修复必须保留物理下推，并让被下推的谓词与 evaluator 同语义。

## 3. 语义真值表

`Value` 的排序中 NULL 低于数值、字符串等标量类型；`ScanPredicate` 对 NULL 一律返回 false。以属性在左为例：

| 比较 | evaluator 下 NULL 行 | 原扫描谓词 | 修复后判定 |
|---|---|---|---|
| `prop < literal` | true | false | true |
| `prop <= literal` | true | false | true |
| `prop > literal` | false | false | false |
| `prop >= literal` | false | false | false |
| `prop = literal` | false | false | false |

字面量在左时按交换后的运算符判断：`literal < prop`、`literal <= prop` 可下推，`literal > prop`、`literal >= prop` 不可下推。`match_comparison` 已把形状归一化为“属性在左”，因此归一化后为 `<`、`<=` 的 conjunct 不进入物理谓词。

## 4. 修改落点

修改集中在存储层，查询层下推规则与 `Filter` 均不改：

- `crates/linkrs-storage/src/cursor/predicates.rs`：`ScanPredicate` 增加 NULL 判定，上界区间匹配 NULL；`matches`、`matches_column`、`matches_value` 统一走该判定；`PredicateRange::overlaps` 对上界区间不再按非 NULL 边界剪枝。
- `crates/linkrs-storage/src/edge/csr_with_properties/read.rs`：列式边过滤把缺失列、缺失单元视为 NULL 并交给谓词判定；剪枝范围缺列时保持块存活。
- `crates/linkrs-storage/src/edge/edge_table/core/index.rs`：上界区间不走有序索引候选，改由段扫描按解码单元裁决。
- 查询层 `extract_scan_predicates` 与 `pushable_conjuncts` 保持原样，避免改变计划形状与反馈样本。

## 5. 护栏

- 不删除残余 `Filter`，不把扫描谓词提升为权威过滤。
- 不修改并行经济门、反馈学习或分区配置；物理过滤行为在非 NULL 数据上保持不变。
- NULL 语义只在 `ScanPredicate` 一处定义，列式匹配、行式匹配、zone-map 与索引回退都复用该判定。
- 不扩大到缺失属性以外的复杂类型；上界区间统一保守处理，避免遗漏 NULL。
- 任何谓词形状的数值必须在 pushed 与 unpushed 语义下一致。

## 6. 验证

- 单测：`ScanPredicate` 的 NULL 判定、行式与列式匹配、zone-map 对上界区间不剪枝；查询层下推单测保持原样。
- DQL：NULL 键列上 `FILTER` 与 `WHERE` 在 `<`、`<=`、`literal > prop`、`>`、`=` 上逐值相等；空输入继续返回单行零。
- 回归：`parallel_partition_execution` 的范围反馈用例必须继续放行；DQL、streaming、parity 固定门数值不变。
- devs：release 编译、上述测试、过滤门与新指标复测；结果与日志按既有命名回填问题清单。

## 7. 完整替换的解禁条件

只有同时满足以下条件，才重新评估删除残余 `Filter`：

1. 存储谓词在 identity/flat、TTL/MVCC、混合数值类型和 zone-map 剪枝路径上均与 evaluator 逐行等价；
2. 有 pushed/unpushed 差分测试覆盖上述形状，且数值完全一致；
3. 扫描模式、布局版本或 schema 信息能作为窄放行前置，未证明的形状继续保留双层结构；
4. 性能收益有安静窗口的重复测量支撑。

在此之前，完整替换保持关闭。
