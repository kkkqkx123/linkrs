# Space Session 持久化架构分析

## 问题现象

`TestDb::execute_query()` 每次调用都创建新 context，`USE` 语句切换的 space 不持久化到下一次调用，导致后续 DML/DQL 报错：

```
Semantic error: No graph space selected, please execute USE <space> first
```

## 架构全景

### 三层架构

```
┌──────────────────────────────────────────────────────────────────┐
│  Session 层（有状态）                                              │
│  Embedded: Session<S>        Server: ClientSession              │
│     space_id: Arc<RwLock<…>>     space_context: SpaceContext    │
│     space_name: Arc<RwLock<…>>    session_context.space_name    │
└────────────────┬─────────────────────────────────────────────────┘
                 │ 注入 space_id/space_name 到 QueryRequest
                 ▼
┌──────────────────────────────────────────────────────────────────┐
│  QueryApi 层（无状态）                                              │
│  execute(&mut self, query, ctx: QueryRequest) -> QueryResult     │
│    将 SpaceSwitched → QueryResult { columns: [space_name,…] }    │
└────────────────┬─────────────────────────────────────────────────┘
                 │ 调用 PipelineManager
                 ▼
┌──────────────────────────────────────────────────────────────────┐
│  QueryPipelineManager 层（无状态）                                  │
│  每次新建 QueryContext，执行后丢弃                                  │
└──────────────────────────────────────────────────────────────────┘
```

### 数据流

```
Session.execute(query)
  │
  ├─ 1. 读取 space_id, space_name（状态字段）
  ├─ 2. 构造 QueryRequest { space_id, space_name }
  ├─ 3. QueryApi.execute(query, ctx)
  │     │
  │     ├─ PipelineManager 使用 ctx.space_info 设置 QueryContext
  │     ├─ 执行 USE → SpaceSwitched(SpaceSummary)
  │     └─ QueryApi 将 SpaceSwitched 降级为 QueryResult（扁平化）
  │
  ├─ 4. Session.update_space_from_result(&result)
  │     │
  │     └─ 从 result.columns/rows 解析 space_name/space_id
  │        更新 space_id, space_name 字段
  │
  └─ 5. 返回 QueryResult
```

## 生产代码缺陷

### 缺陷 1：语义降级（Semantic Downgrade）

**位置**：`crates/graphdb-api/src/api/core/query_api.rs:230-254`

`ExecutionResult::SpaceSwitched(SpaceSummary)` 是一个语义丰富的变体，包含 `{id, name, vid_type, status}`。但 `QueryApi::convert_to_query_result()` 将其降级为扁平的 `QueryResult { columns, rows }`：

```rust
ExecutionResult::SpaceSwitched(summary) => {
    // 丢失了 SpaceSummary 的类型信息
    Ok(QueryResult {
        columns: vec!["space_name", "space_id", "vid_type"],
        rows: vec![row],
        ...
    })
}
```

上层（`Session`、`GraphService`）不得不重新解析扁平数据来回退提取信息，形成"有损往返"反模式。

**影响**：
- `Session.update_space_from_result()` 通过 `columns.contains("space_name")` 探测 — 脆弱，任何查询只要列名叫 `space_name` 就会触发
- `GraphService.extract_space_summary_from_result()` 先尝试从 DataSet 解析，再 fallback 到 `result.space_summary()` — 但 QueryApi 已经将 SpaceSwitched 转换了，fallback 分支实际不可达

### 缺陷 2：不一致的检测策略

| 路径 | 检测方式 | 文件 |
|------|----------|------|
| Embedded Session | 扫描所有结果的 `space_name` 列 | `session.rs:158` |
| Server GraphService | 检查语句是否以 `USE ` 开头 | `graph_service.rs:293` |

**问题**：
- Embedded：如果普通查询的列名恰好叫 `space_name`，会误触发 space 切换
- Server：如果 `USE` 语句通过某种途径（存储过程、复合语句）不以 `USE ` 开头，会漏检测

### 缺陷 3：ClientSession 双重空间状态

**位置**：`crates/graphdb-api/src/api/server/client/client_session.rs`

`ClientSession` 中有**两个地方**存储 space 信息：

| 位置 | 类型 | 更新方式 |
|------|------|----------|
| `session_context.session.space_name` | `Option<String>` | `update_space_name()` |
| `space_context.space` | `Option<SpaceSummary>` | `set_space()` |

两个方法互不同步：
- `set_space()` 更新 `SpaceContext` 但不更新 `SessionContext`
- `update_space_name()` 更新 `SessionContext` 但不更新 `SpaceContext`

`space_name()` 读的是 `SessionContext`，`space()` 读的是 `SpaceContext`，两者可能不一致。

### 缺陷 4：QueryApi `&mut self` 误导性接口

`QueryApi::execute(&mut self, ...)` 声明为 `&mut self`，暗示有状态变更，但实际上不管理任何 session 状态。`&mut self` 仅用于内部的 `pipeline_manager` 可变引用。

后果：每个调用者都必须手动实现：
1. 读取自己的 space 状态
2. 注入到 `QueryRequest`
3. 调用 `execute()`
4. 解析结果中的 space 信息
5. 更新自己的状态

`TestDb` 就是第4个重复实现这套模式的消费者。

## 测试套件层面的问题

### TestDb 的正确做法

当前的 `TestDb` 实现是**正确的**：

```rust
pub struct TestDb {
    current_space_id: Option<u64>,
    current_space_name: Option<String>,
}

pub fn execute_query(&mut self, query: &str) -> CoreResult<QueryResult> {
    let ctx = QueryRequest {
        space_id: self.current_space_id,
        space_name: self.current_space_name.clone(),
        ...
    };
    let result = self.query_api.execute(query, ctx)?;
    // 从结果解析 space 信息并更新状态
    if result.columns.iter().any(|c| c == "space_name") {
        // 更新 self.current_space_id / self.current_space_name
    }
    Ok(result)
}
```

### 数据驱动测试的问题

`test_social_network.rs` 的正确工作方式：
1. `setup_test_space()` 调用 `USE e2e_social_network` → TestDb 记录 `current_space_name = "e2e_social_network"`
2. 后续 `db.execute_query("INSERT ...")` → TestDb 注入 `space_name = "e2e_social_network"` 到 QueryRequest

`data_driven.rs` 中 `load_gql_file()` 间接调用的 `create_test_db()` → `TestDb::new()`，每次获得全新的 `current_space_id: None`。这是正常的，因为 GQL 文件内包含 `CREATE SPACE IF NOT EXISTS` + `USE`。修复方案就是在 TestDb 中正确追踪 space 状态。

## 修复方案

### 短期修复（已实施）

在 `TestDb` 中增加 `current_space_id`/`current_space_name` 字段，每次 `execute_query()` 时：
1. 注入当前 space 到 `QueryRequest`
2. 从 `QueryResult` 的 `space_name`/`space_id` 列反解析 space 变更

### 中长期修复建议

#### 方案 A：消除语义降级（推荐）

在 `QueryApi::execute()` 返回值中直接保留 `SpaceSwitched` 信息：

```rust
// QueryApi 新增方法
pub fn execute_raw(&mut self, query: &str, ctx: QueryRequest)
    -> CoreResult<ExecutionResult>;
```

让上层直接拿到 `ExecutionResult::SpaceSwitched`，而非解析扁平 `QueryResult`。

这样：
- Embedded Session 直接匹配 `SpaceSwitched`，无需探测列名
- Server GraphService 直接调用 `result.space_summary()`，无需 `starts_with("USE ")` 
- TestDb 直接匹配变体，不需反解析

#### 方案 B：统一 ClientSession 空间状态

消除 `SessionContext.space_name` 与 `SpaceContext` 的重复：

```rust
// 统一从 SpaceContext 获取 space_name
pub fn space_name(&self) -> Option<String> {
    self.space_context.space().map(|s| s.name)
}
```

删除 `session_context.session.space_name` 和 `update_space_name()` 方法。

#### 方案 C：消除 TestDb 重复代码

将 embedded `Session` 直接暴露给测试用：
- `GraphDatabase::session()` 返回有状态的 `Session`
- 测试直接使用 `Session::execute()`，而不是手动维护 `TestDb`

## 优先级

| 缺陷 | 严重程度 | 修复成本 | 建议 |
|------|----------|----------|------|
| 缺陷 1：语义降级 | 中 | 高（改 QueryApi 接口） | 长期 |
| 缺陷 2：检测不一致 | 低 | 低 | 结合方案 A 修复 |
| 缺陷 3：双重状态 | 中 | 低 | 短期 |
| 缺陷 4：误导接口 | 低 | 低（加注释） | 短期 |
| TestDb 重复代码 | 低 | 中 | 长期 |

**短期（当前 Sprint）**：修复缺陷 3、4，保持 TestDb 当前修复方案
**长期**：方案 A 消除语义降级，连带解决缺陷 1、2，同时简化 TestDb
