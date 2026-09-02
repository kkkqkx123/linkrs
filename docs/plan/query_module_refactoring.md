# Query模块重构方案（更新）

## 已完成项

### 1. 统一错误类型 ✅

- `crates/graphdb-query/src/error.rs`：`QueryPipelineError` 已实现，包含 `StatementType`、`PipelinePhase`，以及向 `DBError` 的 `From` 转换。
- 所有流水线阶段可返回带上下文的错误。

### 2. DML 形状缓存优化 ✅

- `crates/graphdb-query/src/pipeline/dml_cache.rs`：hot/cold 双层 `moka::sync::Cache`，键类型为 `DmlCacheKey` 结构体（非 String 拼接），提供 `invalidate_space`、`stats` 等方法。

### 3. 流水线阶段接口 ✅

- `crates/graphdb-query/src/pipeline/stage.rs`：`QueryStage` trait + 5 个 Stage 定义（Parse/Bind/Plan/Optimize/Execute）。
- `OptimizeStage` 和 `ExecuteStage` 为占位实现，后续填充。

### 4. 规划器迁移（已完成部分） ✅

**已真正从 `BoundStatement` 提取数据的规划器（18个）：**

| 规划器 | Bound 类型 | 迁移方式 |
|--------|-----------|---------|
| GoPlanner | `BoundGoStatement` | 完整迁移 |
| LookupPlanner | `BoundLookupStatement` | 完整迁移 |
| PathPlanner | `BoundFindPathStatement` | 完整迁移 |
| SubgraphPlanner | `BoundSubgraphStatement` | 完整迁移 |
| FetchVerticesPlanner | `BoundFetchVerticesStatement` | 完整迁移 |
| FetchEdgesPlanner | `BoundFetchEdgesStatement` | 完整迁移 |
| PipePlanner | `BoundPipeStatement` | 完整迁移 |
| SetOperationPlanner | `BoundSetOperationStatement` | 完整迁移 |
| CopyPlanner | `BoundCopy` | 本次完成 |
| DeletePlanner | `BoundDelete` | 本次完成 |
| UpdatePlanner | `BoundUpdate` | 本次完成 |
| RemovePlanner | `BoundRemove` | 本次完成 |
| SetPlanner | `BoundSet` | 本次完成 |
| **FilterPlanner** | **`BoundFilter`** | **本次完成** |
| **YieldPlanner** | **`BoundYield`** | **本次完成** |
| **CollectPlanner** | **`BoundCollect`** | **本次完成** |
| **AssignVariablePlanner** | **`BoundAssignVariable`** | **本次完成** |

**已简化（移除反向桥接）的规划器（5个）：**

| 规划器 | 改动 |
|--------|------|
| MaintainPlanner | 移除 AST 重建，直接使用 `validated` |
| UsePlanner | 同上 |
| UserManagementPlanner | 同上 |
| FulltextSearchPlanner | 同上 |
| VectorSearchPlanner | 同上 |

### 5. Bound 类型改进 ✅

- **移除 `span` 字段**：所有 17 个 Bound 结构体的 `span: Span` 字段已移除（规划阶段从未读取）。
- **新增 BoundStatement 变体**：`Filter`、`Yield`、`Collect`、`AssignVariable` 已添加，消除了 `Other` 变体对这 4 种语句的兜底。
- **新增绑定模式类型**：`BoundCreateTarget`、`BoundMergePattern`、`BoundPatternElement`、`BoundPatternVertex`、`BoundPatternEdge` 已定义，替换原始 AST 类型。
- **`BoundCreate` 和 `BoundMerge`** 已更新为使用新的绑定类型。
- **Binder 已更新**：`bind_create`、`bind_merge`、`bind_filter`、`bind_yield`、`bind_collect`、`bind_assign_variable` 均已实现或更新。

### 5. compiler.rs 简化 ✅

- 移除了 `UnsupportedOperation` fallback 分支，直接调用 `plan_bound()`。

---

## 剩余问题

### 问题 A：`BoundStatement` 缺少多种语句变体

以下语句类型在 `BoundStatement` 中没有专用变体，全部落入 `BoundStatement::Other(Box<Stmt>)`：

| 语句类型 | 对应规划器 | 当前状态 |
|----------|-----------|---------|
| SHOW / SHOW CREATE | MaintainPlanner | `Other` 路径 |
| CREATE USER / DROP USER / ALTER USER | UserManagementPlanner | `Other` 路径 |
| CREATE FULLTEXT INDEX | FulltextSearchPlanner | `Other` 路径 |
| CREATE VECTOR INDEX | VectorSearchPlanner | `Other` 路径 |
| EXPLAIN / PROFILE | ExplainPlanner | `Other` 路径 |

### 问题 B：规划器忽略 `BoundStatement`

以下规划器的 `plan_bound()` 完全忽略 `bound` 参数，委托给 `self.transform(validated, qctx)`：

| 规划器 | 原因 |
|--------|------|
| CreatePlanner | `BoundCreate.target` 已迁移为 `BoundCreateTarget`，但 `plan_bound` 未更新 |
| MergePlanner | `BoundMerge.pattern` 已迁移为 `BoundMergePattern`，但 `plan_bound` 未更新 |
| InsertPlanner | 无专用 BoundStatement 变体 |
| ExplainPlanner | 需要内部语句重新规划 |
| MatchStatementPlanner | 需要 `validated.expr_context()` |
| DDL 规划器（Maintain/Use/UserManagement/Fulltext/Vector） | 无专用 BoundStatement 变体 |

### 问题 D：`build_validated_fallback` 无法删除

`compiler.rs` 仍调用 `build_validated_fallback(ast)` 并传入 `plan_bound()`，因为：
1. Create/Merge/Assignment/Explain/Match 规划器仍需通过 `validated` 访问 AST
2. DDL 规划器（Maintain/Use/UserManagement/Fulltext/Vector）的 `transform()` 读取 `validated.stmt()`

### 问题 E：`transform_with_metadata` 残留

以下规划器仍实现 `transform_with_metadata`：

| 规划器 | 是否仍需要 |
|--------|-----------|
| ExplainPlanner | 需要 — 内部调用 `inner_planner.transform_with_metadata(...)` |
| LookupPlanner | 需要 — 用于元数据驱动的索引选择 |
| MatchStatementPlanner | 需要 — 用于存储 metadata_context |
| FulltextSearchPlanner | 需要 — 用于全文索引元数据 |
| VectorSearchPlanner | 需要 — 用于向量索引元数据 |

这些保留，因为 `ExplainPlanner` 的内部规划流程依赖此方法。可考虑后续将 `ExplainPlanner` 重构为使用 `plan_bound` 内部调用。

### 问题 F：`span` 字段普遍未使用

所有 17 个 Bound 结构体的 `span: Span` 字段在规划阶段从未被读取。详见"附录：Bound 类型字段使用分析"。

### 问题 G：部分 Bound 类型完全被规划器忽略

`BoundMatchStatement`（9字段）和 `BoundInsert`（3字段）的 `plan_bound()` 实现完全忽略 bound 参数，从 AST 读取所有数据。详见"附录"。

### 问题 H：部分 Bound 类型字段冗余

`BoundFindPathStatement`（5/10 未使用）、`BoundSubgraphStatement`（3/6 未使用）、`BoundFetchEdgesStatement`（1/6 未使用）存在未接入规划器的字段。详见"附录"。

---

## 修改方案

### 方案 1：补充 `BoundStatement` 变体（推荐）

**目标**：为所有尚未有专用变体的语句类型创建 Bound 结构体。

**步骤**：

#### 1.1 在 `crates/graphdb-query/src/binder/bound.rs` 中新增结构体

```rust
// ── Clause-level DQL ────────────────────────────────────────────────
pub struct BoundAssignVariable {
    pub span: Span,
    pub name: String,
    pub expression: BoundExpression,
}

pub struct BoundFilter {
    pub span: Span,
    pub condition: BoundExpression,
}

pub struct BoundCollect {
    pub span: Span,
    pub items: Vec<BoundYieldItem>,
}

pub struct BoundYield {
    pub span: Span,
    pub items: Vec<BoundYieldItem>,
    pub where_clause: Option<BoundExpression>,
    pub distinct: bool,
    pub order_by: Option<Vec<BoundOrderByItem>>,
    pub skip: Option<BoundSkipClause>,
    pub limit: Option<BoundLimitClause>,
}

// ── DDL ─────────────────────────────────────────────────────────────
pub struct BoundShow {
    pub span: Span,
    pub target: ShowTarget,
}

pub struct BoundShowCreate {
    pub span: Span,
    pub target: ShowCreateTarget,
}

pub struct BoundCreateUser {
    pub span: Span,
    pub username: String,
    pub password: String,
    pub role: Option<String>,
    pub if_not_exists: bool,
}

pub struct BoundDropUser {
    pub span: Span,
    pub username: String,
    pub if_exists: bool,
}

pub struct BoundAlterUser {
    pub span: Span,
    pub username: String,
    pub alter_type: AlterUserType,
}

pub struct BoundCreateFulltextIndex {
    pub span: Span,
    pub index_name: String,
    pub target: FulltextIndexTarget,
    pub if_not_exists: bool,
}

pub struct BoundCreateVectorIndex {
    pub span: Span,
    pub index_name: String,
    pub target: VectorIndexTarget,
    pub if_not_exists: bool,
}

// ── EXPLAIN / PROFILE ──────────────────────────────────────────────
pub struct BoundExplain {
    pub span: Span,
    pub statement: Box<BoundStatement>,
    pub format: Option<ExplainFormat>,
}

pub struct BoundProfile {
    pub span: Span,
    pub statement: Box<BoundStatement>,
}
```

#### 1.2 在 `BoundStatement` 枚举中添加变体

```rust
pub enum BoundStatement {
    // ... 现有变体 ...
    AssignVariable(BoundAssignVariable),
    Filter(BoundFilter),
    Collect(BoundCollect),
    Yield(BoundYield),
    Show(BoundShow),
    ShowCreate(BoundShowCreate),
    CreateUser(BoundCreateUser),
    DropUser(BoundDropUser),
    AlterUser(BoundAlterUser),
    CreateFulltextIndex(BoundCreateFulltextIndex),
    CreateVectorIndex(BoundCreateVectorIndex),
    Explain(BoundExplain),
    Profile(BoundProfile),
}
```

#### 1.3 在 `crates/graphdb-query/src/binder/` 中实现绑定逻辑

为每个新 Bound 类型实现 `bind_xxx` 函数，将 AST 节点转换为 Bound 结构体。

#### 1.4 更新 `PlannerEnum::from_bound_statement`

为每个新变体注册对应的规划器。

#### 1.5 逐个迁移规划器

每个规划器的迁移步骤：
1. 在 `plan_bound()` 中从 `BoundStatement` 提取数据
2. 调用 `bound_expr_to_contextual()` 转换表达式
3. 生成 `SubPlan`
4. 删除对 `validated` 的依赖（或保留为 fallback）

**优先级**：
1. FilterPlanner / CollectPlanner / YieldPlanner（简单，一个表达式或少量字段）
2. AssignVariablePlanner（简单，一个名称 + 一个表达式）
3. ExplainPlanner（需要递归绑定内部语句）
4. DDL 规划器（Show/CreateUser/DropUser/AlterUser/CreateFulltextIndex/CreateVectorIndex）

### 方案 2：处理 Bound 类型使用原始 AST 的问题

**目标**：为 `BoundCreate.target` 和 `BoundMerge.pattern` 创建完全绑定的表示。

**步骤**：

#### 2.1 定义 Bound 模式类型

```rust
// crates/graphdb-query/src/binder/bound.rs

pub enum BoundCreateTarget {
    Node {
        labels: Vec<String>,
        properties: Vec<(String, BoundExpression)>,
    },
    Edge {
        edge_type: String,
        src: BoundPatternVertex,
        dst: BoundPatternVertex,
        properties: Vec<(String, BoundExpression)>,
    },
    Path {
        patterns: Vec<BoundPatternElement>,
    },
}

pub enum BoundPatternElement {
    Vertex(BoundPatternVertex),
    Edge(BoundPatternEdge),
}

pub struct BoundPatternVertex {
    pub variable: Option<String>,
    pub labels: Vec<String>,
    pub properties: Option<Vec<(String, BoundExpression)>>,
}

pub struct BoundPatternEdge {
    pub variable: Option<String>,
    pub edge_types: Vec<String>,
    pub direction: EdgeDirection,
    pub properties: Option<Vec<(String, BoundExpression)>>,
    pub min_steps: Option<u32>,
    pub max_steps: Option<u32>,
}

pub enum BoundMergePattern {
    Node(BoundPatternVertex),
    Edge {
        src: BoundPatternVertex,
        edge: BoundPatternEdge,
        dst: BoundPatternVertex,
    },
}
```

#### 2.2 更新 `BoundCreate` 和 `BoundMerge`

```rust
pub struct BoundCreate {
    pub span: Span,
    pub target: BoundCreateTarget,  // 替换为绑定类型
    pub if_not_exists: bool,
}

pub struct BoundMerge {
    pub span: Span,
    pub pattern: BoundMergePattern,  // 替换为绑定类型
    pub on_create: Vec<BoundAssignment>,
    pub on_match: Vec<BoundAssignment>,
}
```

#### 2.3 迁移 CreatePlanner 和 MergePlanner

从 `BoundCreate.target` / `BoundMerge.pattern` 提取数据，不再依赖 AST。

### 方案 3：消除 `build_validated_fallback`

**前置条件**：方案 1 和方案 2 完成后，所有规划器均可从 `BoundStatement` 获取所需数据。

**步骤**：

1. 确认所有规划器的 `plan_bound()` 不再读取 `validated.stmt()` 或 `validated.expr_context()`
2. 从 `compiler.rs` 中移除 `build_validated_fallback` 调用
3. 将 `plan_bound` 签名中的 `validated` 参数改为 `Option<&ValidatedStatement>`（仅为 MATCH 等复杂规划器保留过渡期支持）
4. 最终移除 `validated` 参数

### 方案 4：重构 ExplainPlanner 内部规划

**目标**：消除 `ExplainPlanner` 对 `transform_with_metadata` 的依赖。

**步骤**：

1. 在 `BoundStatement` 中添加 `Explain(BoundExplain)` / `Profile(BoundProfile)` 变体
2. `BoundExplain.statement` 存储已绑定的内部语句
3. `ExplainPlanner::plan_bound()` 递归调用 `PlannerEnum::from_bound_statement(&inner_bound)` → `planner.plan_bound()`
4. 移除 `ExplainPlanner` 的 `transform_with_metadata` 实现

### 方案 5：清理 `transform_with_metadata`

**目标**：将 `transform_with_metadata` 降级为仅在 `Planner` trait 中保留默认实现，移除规划器级 override。

**步骤**：

1. `ExplainPlanner` 重构后（方案 4），不再有外部调用者
2. 检查 `LookupPlanner`、`MatchStatementPlanner`、`FulltextSearchPlanner`、`VectorSearchPlanner` 的 override 是否可以合并到 `plan_bound` 中
3. 如果 `plan_bound` 接收 `metadata: Option<&MetadataContext>`，则这些 override 均可移除
4. 从 `Planner` trait 中移除 `transform_with_metadata` 方法
5. 从 `PlannerEnum` 中移除 `transform_with_metadata` 分发

---

## 执行顺序

```
阶段 0（立即执行，低风险）：
  └── 问题 F：移除或标记所有 Bound 类型中的 span 字段

阶段 1（可并行）：
  ├── 方案 1.1-1.3：补充 BoundStatement 变体 + 绑定逻辑
  ├── 方案 2.1-2.2：定义 Bound 模式类型
  └── 问题 G/H：处理完全忽略和冗余字段

阶段 2（依赖阶段 1）：
  ├── 方案 1.4-1.5：迁移 Filter/Yield/Collect/AssignVariable 规划器
  ├── 方案 2.3：迁移 Create/Merge 规划器
  └── 方案 4：重构 ExplainPlanner

阶段 3（依赖阶段 2）：
  ├── 方案 3：消除 build_validated_fallback
  └── 方案 5：清理 transform_with_metadata
```

## 验证

每个阶段完成后运行：

```bash
cargo check --lib
cargo test -p graphdb-query --lib -- --nocapture
```

全部完成后运行完整测试：

```bash
cargo test --lib -- --nocapture
```

## 最终目标

- `BoundStatement` 覆盖所有语句类型，消除 `Other` 变体
- 所有规划器从 `BoundStatement` 提取数据，不依赖 `ValidatedStatement`
- `build_validated_fallback` 和 `ValidatedStatement` 类型可完全移除
- `transform_with_metadata` 方法从 trait 中移除
- `compiler.rs` 调用路径简化为：`from_bound_statement` → `plan_bound`

---

## 附录：Bound 类型字段使用分析

### 问题 F：`span` 字段普遍未使用

所有 17 个 Bound 结构体均携带 `span: Span` 字段，但在规划阶段**无一被读取**。该字段仅在 binder 阶段用于错误报告，规划器从未访问。

| Bound 类型 | 总字段数 | 被规划器读取 | `span` 未使用 |
|-----------|---------|------------|-------------|
| `BoundGoStatement` | 7 | 6 | ✅ |
| `BoundLookupStatement` | 4 | 3 | ✅ |
| `BoundFindPathStatement` | 10 | 5 | ✅ |
| `BoundSubgraphStatement` | 6 | 3 | ✅ |
| `BoundFetchVerticesStatement` | 4 | 3 | ✅ |
| `BoundFetchEdgesStatement` | 6 | 4 | ✅ |
| `BoundMatchStatement` | 9 | **0** | ✅ |
| `BoundInsert` | 3 | **0** | ✅ |
| `BoundDelete` | 4 | 3 | ✅ |
| `BoundUpdate` | 5 | 4 | ✅ |
| `BoundSet` | 2 | 1 | ✅ |
| `BoundRemove` | 2 | 1 | ✅ |
| `BoundCopy` | 7 | 6 | ✅ |
| `BoundPipeStatement` | 2 | 1 | ✅ |
| `BoundReturnStatement` | 6 | 5 | ✅ |
| `BoundWithStatement` | 3 | 2 | ✅ |
| `BoundGroupByStatement` | 3 | 2 | ✅ |

**建议**：将 `span` 改为 `#[allow(dead_code)]` 或移至 `#[cfg(test)]` 仅在测试中保留。或从所有 Bound 结构体中移除 `span`，错误定位改由 AST 或 parser span 独立提供。

### 问题 G：部分 Bound 类型完全被规划器忽略

| Bound 类型 | 总字段 | 被读取 | 说明 |
|-----------|-------|-------|------|
| `BoundMatchStatement` | 9 | **0** | 规划器 `plan_bound` 仅做类型校验，全部从 `validated.stmt()`（AST）读取。`query_graph`、`where_clause`、`return_clause`、`order_by`、`limit`、`skip`、`optional`、`delete_clause` 全部未使用。 |
| `BoundInsert` | 3 | **0** | 规划器声明 `_bound`（显式忽略），全部从 AST 读取。`target`、`if_not_exists` 全部未使用。 |

**建议**：这两个类型是 binder 产出与 planner 消费之间的完全断裂。要么完成迁移（方案 1/2），要么在迁移完成前标记为 `#[allow(dead_code)]` 以避免误导。

### 问题 H：部分 Bound 类型字段冗余

| Bound 类型 | 未使用字段 | 说明 |
|-----------|-----------|------|
| `BoundFindPathStatement` | `where_clause`, `limit`, `skip`, `yield_clause` | 规划器未实现过滤/分页/投影，仅用 `from`, `to`, `over`, `shortest`, `max_steps` |
| `BoundSubgraphStatement` | `from`, `yield_clause` | `from` 未使用（规划器始终从空 `ArgumentNode` 开始），`yield_clause` 未使用 |
| `BoundFetchEdgesStatement` | `properties` | 未用于控制返回列 |

**建议**：
- 如果字段属于"规划器尚未实现的功能"，保留但添加 `// TODO(plan_bound): wire into planner` 注释
- 如果字段属于"设计时过度设计"，移除以简化类型

### 问题 I：`BoundAssignment` 与 AST `Assignment` 结构差异

| 字段 | AST `Assignment` | `BoundAssignment` |
|------|-----------------|-------------------|
| `target` | `Option<ContextualExpression>` | `Option<BoundExpression>` |
| `property` | `String` | `String` |
| `value` | `ContextualExpression` | `BoundExpression` |
| `object` | 无 | `Option<BoundExpression>` |

`BoundAssignment` 多出 `object` 字段，实际在 `SetPlanner` 中用于区分"直接属性更新"和"变量赋值"。该字段在 bind 阶段从 `target` 的 `Property { object, .. }` 模式中提取并扁平化。

**建议**：保留当前设计。`object` 字段的扁平化是合理的——它让规划器无需再次解析 `Property` 表达式结构。

### 类型改造优先级

| 优先级 | 改动 | 影响范围 |
|--------|------|---------|
| P0 | 所有 Bound 类型移除或标记 `span` | 17 个结构体 |
| P1 | `BoundMatchStatement` / `BoundInsert` 完成迁移或标记为 dead_code | 2 个结构体 |
| P2 | `BoundFindPathStatement` 移除未使用字段或添加 TODO 注释 | 1 个结构体 |
| P2 | `BoundSubgraphStatement` 移除 `from` / `yield_clause` 或添加 TODO | 1 个结构体 |
| P3 | `BoundFetchEdgesStatement` 移除 `properties` 或添加 TODO | 1 个结构体 |
