# 类型化属性裁剪（Property Pruning）方案

## 1. 现状分析

### 现有实现

- 已存在投影下推规则（`optimizer/heuristic/projection_pushdown.rs`）：
  `PushProjectDownScanVertices`、`PushProjectDownScanEdges`
- `EnrichScanSlotsWithFilterProps`（`heuristic/slot_coverage/`）：把残余谓词列注入
  scan 的 `projected_properties`，供列式求值直接服务 WHERE
- `PushProjectDown{GetVertices,GetEdges,GetNeighbors}` 三条规则**被有意排除**
  （`heuristic/rule_enum.rs:161` 注释）：因当前裁剪会把 Project 节点抹掉并将别名
  当裸属性引用，对计算表达式 / 别名 / 函数不安全，须等类型化
  required-property pruning 实现（注释中称为 "Phase 4"）后才启用

### 核心缺口

裁剪只发生在扫描（Scan）层，图算子（GetVertices / GetEdges / GetNeighbors /
AppendVertices / Expand 等）读取的整行整属性无法按需收窄；且缺少"从输出需求
逆推属性需求"的分析器，三条被排除的规则无法安全启用。

## 2. 方案设计

### 2.1 RequiredPropertyAnalyzer（需求传播分析器）

新增 `optimizer/analysis/required_properties.rs`：

- 从计划根（Sink / Project）出发，逆拓扑序传播"属性需求集合"
  `PropertyRequirement { alias, tag_name, prop_names: BTreeSet<PropName> }`
- 每类节点定义需求变换：
  - `Project`：按投影表达式收集被引用的 `var.prop`，向下游传播
  - `Filter`：谓词引用的属性并入需求，继续下传
  - `GetVertices/GetEdges/GetNeighbors/AppendVertices`：消费需求的属性名列表，
    其余属性标记为可裁剪
  - `Expand/Traverse`：透传（路径语义下属性完整保留，仅 `id_only` 标注生效）
- 输出：`RequiredPropertiesMap: HashMap<NodeId, Vec<PropertyRequirement>>`

类型化约束（本方案核心）：

- 属性引用必须能静态解析到具体 tag/edge（`metadata/` 的 schema 元数据提供
  tag/edge 属性表）
- 别名 / 计算表达式 / 函数参数中出现的属性引用视为"不透明"，整列保留
- 无法解析到 schema 的属性引用（动态标签）同样整列保留

### 2.2 图算子属性收窄

在物理计划构建层（`executor/streaming/plan/`）或启发式规则中应用：

- `GetVertices` / `GetEdges` / `GetNeighbors`：将
  `projected_properties`（或等价字段）收窄为需求集合
- `AppendVertices`：仅读取需求的 tag 属性
- 与 `EnrichScanSlotsWithFilterProps` 联动：需求传播结果作为其属性注入的输入，
  形成"需求 + 谓词列"的并集，避免重复注入

### 2.3 启用被排除的三条规则

- 启用前提：RequiredPropertyAnalyzer 与规则共享同一需求解析（类型化判定一致）
- 保留 Project 节点：不抹掉 Project，仅收窄下游算子读取的列（规避注释中所述
  别名/表达式不安全问题）；Project 自身的消除仍由既有的
  `RemoveNoopProject` / `CollapseProject` 负责
- 条件：仅当需求集合覆盖 Project 全部输出列时才允许裁剪

## 3. 实施步骤

| 步骤 | 内容 | 涉及文件 |
|------|------|----------|
| 1 | 实现 RequiredPropertyAnalyzer（需求传播 + 类型化解析） | 新增 `optimizer/analysis/required_properties.rs` |
| 2 | 图算子属性收窄（GetVertices/GetEdges/GetNeighbors/AppendVertices） | `projection_pushdown.rs`, `executor/streaming/plan/` |
| 3 | 启用 3 条被排除规则（保留 Project 语义） | `heuristic/rule_enum.rs` |
| 4 | 与 EnrichScanSlotsWithFilterProps 联动，移除重复注入 | `heuristic/slot_coverage/` |
| 5 | EXPLAIN 显示每算子 projected 列，便于验证 | `executor/explain/physical_plan_explain.rs` |

## 4. 验证方法

- 正确性：规则启用后全量执行结果必须逐行一致（与未裁剪计划对比）；
  重点覆盖：别名、计算表达式、函数参数中的属性引用
- 单元测试：每类节点需求传播测试 + 类型化解析测试（含动态标签拒绝用例）
- 回归：`cargo test -p graphdb-query` 全量
- 收益度量：benchmark 中对比裁剪前后 GetVertices/GetNeighbors 的
  `num_rows`（读取行数）与内存峰值

## 5. 预期收益

- 减少图算子读取的属性列与内存占用（文档 3.4 节 Nebula 侧 Property Pruning
  的对应能力）
- 解除 `rule_enum.rs` Phase 4 阻塞注释，3 条投影下推规则落地
- 与列式 DataChunk 结合，属性越少列式化收益越大

## 6. 风险与回退

- **风险**：类型化解析遗漏导致属性缺失（正确性 bug）。缓解：
  - 需求传播为"保守交并"：任何不透明引用即保留整列
  - 裁剪仅在 schema 完整解析成功时生效（`metadata` 缺失时跳过）
  - 测试矩阵覆盖别名/函数/嵌套属性访问
- **回退**：规则注册开关 `ENABLE_TYPED_PROPERTY_PRUNING`（默认关闭，
  验证充分后开启），回退即恢复现状
