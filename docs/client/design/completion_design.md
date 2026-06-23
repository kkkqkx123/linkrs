# 自动补全设计方案

## 1. 概述

### 1.1 目标

为 GraphDB CLI 提供多层次的智能自动补全系统，覆盖关键字、对象名、函数名、变量名等，并根据上下文提供精准的补全建议。

### 1.2 参考实现

- **psql**：基于 libedit/readline 的 Tab 补全，支持 SQL 关键字、表名、列名、函数名补全
- **usql**：上下文感知补全，`select * f<Tab>` 补全 `from`、`fetch`、`full outer join`
- **rustyline**：通过 `Completer` trait 提供补全接口，`Hinter` trait 提供行内提示

## 2. 现状分析

### 2.1 Phase 1 已实现

当前 `GraphDBCompleter`（`src/completion/completer.rs`）实现了：

- **关键字补全**：硬编码的 GQL 关键字列表（约 110 个）
- **元命令补全**：硬编码的 `\` 命令列表（约 30 个）
- **基础词法分析**：`get_last_word()` 按空白和分隔符提取当前输入词

### 2.2 不足之处

| 问题            | 说明                                       |
| --------------- | ------------------------------------------ |
| 无上下文感知    | 输入 `MATCH (p:per` 时无法补全 Tag 名      |
| 无对象名补全    | 无法补全 Space、Tag、Edge、属性名          |
| 无函数补全      | 无法补全 `count()`、`sum()` 等函数         |
| 无变量补全      | 无法补全 `:varname` 变量                   |
| 无 Hint 提示    | `Hinter` trait 返回 `None`，无行内灰色提示 |
| Schema 缓存缺失 | 每次补全都需要查询服务器，性能差           |
| 补全候选无分类  | 所有关键字混在一起，无优先级排序           |

## 3. 补全层次设计

### 3.1 补全层次总览

```
┌─────────────────────────────────────────────┐
│            Layer 4: 上下文感知补全            │
│   MATCH (p:person)-[:follow]->(f:per<Tab>   │
│   → 补全 Tag 名，排除已使用的 person          │
├─────────────────────────────────────────────┤
│            Layer 3: 对象名补全               │
│   MATCH (p:<Tab>  → 补全 Tag 名              │
│   -[:<Tab>        → 补全 Edge 名             │
│   p.<Tab>         → 补全属性名               │
├─────────────────────────────────────────────┤
│            Layer 2: 函数/变量补全             │
│   RETURN <Tab>    → 补全函数名 + 关键字       │
│   :<Tab>          → 补全变量名               │
├─────────────────────────────────────────────┤
│            Layer 1: 关键字/元命令补全          │
│   MAT<Tab>        → MATCH                    │
│   \sh<Tab>        → \show_spaces             │
└─────────────────────────────────────────────┘
```

### 3.2 Layer 1: 关键字与元命令补全（已实现，需优化）

**改进点**：

1. **关键字分组与优先级**

   ```rust
   enum CompletionPriority {
       High,    // 常用关键字: MATCH, RETURN, WHERE, CREATE, INSERT
       Medium,  // 一般关键字: AND, OR, NOT, IN, AS
       Low,     // 少用关键字: TTL_DURATION, REPLICA_FACTOR
   }

   struct KeywordEntry {
       word: &'static str,
       priority: CompletionPriority,
       category: KeywordCategory,
   }

   enum KeywordCategory {
       Dql,  // MATCH, GO, LOOKUP, FETCH
       Dml,  // INSERT, UPDATE, DELETE
       Ddl,  // CREATE, ALTER, DROP
       Dcl,  // GRANT, REVOKE
       Clause, // WHERE, ORDER, LIMIT, RETURN
       Type, // STRING, INT, FLOAT, BOOL
       Function, // count, sum, avg, min, max
   }
   ```

2. **大小写保持**：补全时保持用户输入的大小写风格
   - 用户输入 `mat<Tab>` → 补全为 `MATCH`（关键字统一大写）
   - 用户输入 `Mat<Tab>` → 补全为 `MATCH`

3. **元命令参数补全**
   - `\format <Tab>` → 补全 `table`, `csv`, `json`, `vertical`, `html`
   - `\connect <Tab>` → 补全 Space 名列表
   - `\describe <Tab>` → 补全 Tag 名列表
   - `\describe_edge <Tab>` → 补全 Edge 名列表

### 3.3 Layer 2: 函数与变量补全

#### 3.3.1 函数补全

```rust
struct FunctionEntry {
    name: &'static str,
    signature: &'static str,
    description: &'static str,
    category: FunctionCategory,
}

enum FunctionCategory {
    Aggregate,  // count, sum, avg, min, max, collect
    String,     // length, size, trim, lower, upper, substring, replace
    Numeric,    // abs, ceil, floor, round, sqrt
    List,       // head, tail, size, reverse
    Date,       // date, datetime, timestamp, duration
    Path,       // length, nodes, relationships, startNode, endNode
    Type,       // type, id, label, properties
}
```

**补全行为**：

- 输入 `RETURN cou<Tab>` → 补全 `count(`
- 输入 `RETURN str_<Tab>` → 补全 `string` 类函数
- 补全函数名后自动添加 `(` 并在 Hinter 中显示参数签名

#### 3.3.2 变量补全

```rust
impl Completer for GraphDBCompleter {
    fn complete(&self, line: &str, pos: usize, ctx: &Context) -> Result<...> {
        // 检测 :varname 模式
        if let Some(colon_pos) = find_variable_prefix(line, pos) {
            let partial = &line[colon_pos + 1..pos];
            let vars = self.variable_store.get_matching(partial);
            return Ok((colon_pos, vars_to_candidates(vars)));
        }
        // ...
    }
}
```

**补全行为**：

- 输入 `:li<Tab>` → 补全为 `:limit`（如果变量 `limit` 存在）
- 变量名补全需要与 `Session.variables` 联动

### 3.4 Layer 3: 对象名补全

#### 3.4.1 Schema 缓存

```rust
pub struct SchemaCache {
    spaces: Vec<String>,
    tags: Vec<TagInfo>,
    edges: Vec<EdgeTypeInfo>,
    functions: Vec<FunctionInfo>,
    last_updated: std::time::Instant,
    ttl: std::time::Duration,
}

impl SchemaCache {
    pub fn new() -> Self {
        Self {
            spaces: Vec::new(),
            tags: Vec::new(),
            edges: Vec::new(),
            functions: Vec::new(),
            last_updated: std::time::Instant::now(),
            ttl: std::time::Duration::from_secs(300),
        }
    }

    pub async fn refresh(&mut self, client: &GraphDBHttpClient, space: Option<&str>) -> Result<()> {
        self.spaces = client.list_spaces().await?;
        if let Some(space) = space {
            self.tags = client.list_tags(space).await?;
            self.edges = client.list_edge_types(space).await?;
        }
        self.last_updated = std::time::Instant::now();
        Ok(())
    }

    pub fn is_stale(&self) -> bool {
        self.last_updated.elapsed() > self.ttl
    }
}
```

**缓存策略**：

- 初始连接时加载一次
- 切换 Space 时刷新 Tag/Edge 列表
- 执行 DDL 语句（CREATE/DROP/ALTER）后自动刷新
- TTL 过期后下次补全时刷新
- 手动刷新命令：`\refresh_schema` 或 `\rs`

#### 3.4.2 上下文规则

| 输入模式                         | 补全内容                  | 示例                              |
| -------------------------------- | ------------------------- | --------------------------------- |
| `MATCH (x:<Tab>`                 | Tag 名列表                | `MATCH (x:person`                 |
| `MATCH (x)-[:<Tab>`              | Edge 名列表               | `MATCH (x)-[:follow`              |
| `MATCH (x)-[r:follow]->(y:<Tab>` | Tag 名列表                | `MATCH (x)-[r:follow]->(y:person` |
| `x.<Tab>`                        | 属性名列表                | `x.name`                          |
| `USE <Tab>`                      | Space 名列表              | `USE mygraph`                     |
| `CREATE TAG <Tab>`               | 已有 Tag 名（用于 ALTER） | `CREATE TAG person`               |
| `\connect <Tab>`                 | Space 名列表              | `\connect mygraph`                |
| `\describe <Tab>`                | Tag 名列表                | `\describe person`                |
| `\describe_edge <Tab>`           | Edge 名列表               | `\describe_edge follow`           |

#### 3.4.3 上下文检测器

```rust
enum CompletionContext {
    Keyword,
    TagName,
    EdgeName,
    PropertyName { tag: String },
    SpaceName,
    FunctionName,
    VariableName,
    MetaCommandArg { command: String },
}

fn detect_context(line: &str, pos: usize) -> CompletionContext {
    let before = &line[..pos];

    // 元命令参数上下文
    if before.starts_with('\\') {
        return detect_meta_context(before);
    }

    // USE 语句 → Space 名
    if before.to_uppercase().ends_with("USE ") {
        return CompletionContext::SpaceName;
    }

    // MATCH (x: → Tag 名
    if let Some(tag_ctx) = detect_tag_context(before) {
        return tag_ctx;
    }

    // -[: → Edge 名
    if let Some(edge_ctx) = detect_edge_context(before) {
        return edge_ctx;
    }

    // x. → 属性名
    if let Some(prop_ctx) = detect_property_context(before) {
        return prop_ctx;
    }

    // : → 变量名
    if before.ends_with(':') && !is_schema_context(before) {
        return CompletionContext::VariableName;
    }

    CompletionContext::Keyword
}
```

### 3.5 Layer 4: 上下文感知补全

**智能排序**：根据上下文对补全候选排序

```rust
fn rank_candidates(candidates: &mut [StringCandidate], context: &CompletionContext) {
    candidates.sort_by(|a, b| {
        let a_score = relevance_score(&a.display, context);
        let b_score = relevance_score(&b.display, context);
        b_score.cmp(&a_score)
    });
}
```

**去重**：已使用的对象名降低优先级

```rust
fn filter_used_names(candidates: &mut Vec<StringCandidate>, used: &HashSet<String>) {
    candidates.retain(|c| !used.contains(&c.display));
}
```

## 4. Hinter（行内提示）

### 4.1 功能描述

在用户输入时，以灰色文字显示可能的补全内容，按 → 或 End 接受提示。

### 4.2 实现设计

```rust
impl Hinter for GraphDBCompleter {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<String> {
        if line.is_empty() || pos != line.len() {
            return None;
        }

        let (_, candidates) = self.complete(line, pos, ...).ok()?;
        if candidates.len() == 1 {
            let hint = &candidates[0].replacement;
            if !hint.is_empty() {
                return Some(hint.clone());
            }
        }
        None
    }
}
```

**提示样式**：使用 `colored` 库将提示文字设为暗灰色

## 5. 模块结构

### 5.1 文件组织

```
src/completion/
├── mod.rs              # 模块导出
├── completer.rs        # 主补全器（已有，需重构）
├── context.rs          # 上下文检测器（新增）
├── schema_cache.rs     # Schema 缓存（新增）
├── keywords.rs         # 关键字定义（从 completer.rs 拆出）
├── functions.rs        # 函数定义（新增）
└── candidates.rs       # 候选生成与排序（新增）
```

### 5.2 核心接口变更

```rust
pub struct GraphDBCompleter {
    keywords: Vec<KeywordEntry>,
    functions: Vec<FunctionEntry>,
    meta_commands: Vec<MetaCommandEntry>,
    schema_cache: Arc<Mutex<SchemaCache>>,
    variable_store: Arc<Mutex<HashMap<String, String>>>,
}
```

**与 Session 的联动**：

- `Session.variables` 通过 `Arc<Mutex<>>` 共享给 `GraphDBCompleter`
- `SchemaCache` 同样通过 `Arc<Mutex<>>` 共享
- 补全时加锁读取，不阻塞主线程

## 6. 实现步骤

### Step 1: 拆分关键字定义（1 天）

- 将 `GQL_KEYWORDS` 和 `META_COMMANDS` 从 `completer.rs` 拆到 `keywords.rs`
- 为关键字添加分类和优先级信息
- 为元命令添加参数补全规则

### Step 2: 实现函数补全（1 天）

- 新增 `functions.rs`，定义函数列表及签名
- 在 `Completer::complete()` 中添加函数名匹配逻辑
- 补全函数名后自动添加 `(`

### Step 3: 实现 Schema 缓存（2 天）

- 新增 `schema_cache.rs`
- 实现缓存加载、刷新、过期逻辑
- 与 `SessionManager` 集成，切换 Space 时刷新缓存
- DDL 执行后标记缓存为 stale

### Step 4: 实现上下文检测（2 天）

- 新增 `context.rs`
- 实现 `detect_context()` 函数
- 覆盖 Tag、Edge、Property、Space、变量等上下文

### Step 5: 实现对象名补全（2 天）

- 基于 Schema 缓存和上下文检测，提供对象名补全
- 实现属性名补全（需要知道当前 Tag 的字段列表）
- 实现变量名补全

### Step 6: 实现 Hinter（1 天）

- 实现 `Hinter` trait，提供单候选行内提示
- 添加提示样式（暗灰色）

### Step 7: 测试与优化（1 天）

- 编写补全单元测试
- 性能测试：确保补全响应时间 < 50ms
- 边界情况处理

## 7. 测试用例

### 7.1 关键字补全

| 输入                     | 按 Tab 后      | 说明           |
| ------------------------ | -------------- | -------------- |
| `MAT`                    | `MATCH`        | 关键字补全     |
| `ret`                    | `RETURN`       | 大小写不敏感   |
| `\sh`                    | `\show_spaces` | 元命令补全     |
| `\format` + Space + `js` | `json`         | 元命令参数补全 |

### 7.2 对象名补全

| 输入              | 按 Tab 后 | 说明         |
| ----------------- | --------- | ------------ |
| `MATCH (x:per`    | `person`  | Tag 名补全   |
| `MATCH (x)-[:fol` | `follow`  | Edge 名补全  |
| `USE my`          | `mygraph` | Space 名补全 |
| `x.na`            | `name`    | 属性名补全   |

### 7.3 变量补全

| 输入        | 按 Tab 后      | 说明           |
| ----------- | -------------- | -------------- |
| `:li`       | `:limit`       | 变量名补全     |
| `LIMIT :li` | `LIMIT :limit` | 查询中变量补全 |

### 7.4 上下文感知

| 输入           | 补全候选    | 说明              |
| -------------- | ----------- | ----------------- |
| `MATCH (x:`    | Tag 名列表  | 冒号后补全 Tag    |
| `MATCH (x)-[:` | Edge 名列表 | 方括号内补全 Edge |
| `x.`           | 属性名列表  | 点号后补全属性    |
