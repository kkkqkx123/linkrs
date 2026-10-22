# 第一阶段：死端语句与不可达语句的正式实现

上游分析：`docs/plan/query-statement-test-coverage-analysis.md`。该文档盘点了 `Stmt` 枚举全部 84 个变体的测试覆盖，得出 11 个"可解析但无 planner"的死端变体与 2 个"无 parser 生产者"的不可达变体。本文是针对这 13 个变体的**处置决策与实施任务**。

本阶段的目标不是"让语句能跑"就收工，而是让每个语句落到一个**明确且被测试锁定**的终态：

- 要么真正可执行，且语义被测试验证；
- 要么被明确拒绝，且拒绝理由对用户可读、指向正确的使用路径；
- 不存在"能解析、能计划、但结果无人验证"的中间态；
- 不存在"代码存在 ⇒ 看起来受支持"的假信号。

---

## 1. 判定原则

沿用项目既有立场（见 `docs/plan/dql-query-syntax-alignment.md` 总原则第 1、3 条）：

1. **发布文档是查询契约。** 每条语句二选一：补齐实现，或从语法与文档中同步移除。不保留"可解析但静默失效"的中间态。
2. **不新增兼容层。** 改动落在原有模块里，不产生 v2 模块。
3. **静默 fallback 必须显性化。** 吞掉的错误、被丢弃的子句、类型被悄悄改写，都要么实现，要么显式报错。
4. **优先复用既有正确实现。** 已经写好且正确的执行器不因为"路由没接上"而被删除——补路由的成本远低于重写。

对"死端"的判定以**实测**为准（本次已逐条跑通真实 pipeline），而非静态推断：

| 实际错误 | 归类 |
|---|---|
| `No planner for bound statement: X` | 死端——内部路由失败泄漏给用户，**必须消除** |
| `[parse] Unexpected token: Clear` | 不可达——语法层根本没有入口 |
| `PlannerError::UnsupportedOperation(...)` 带明确指引 | 已达成的正确终态（如 `KILL QUERY`），只需补测试锁定 |

---

## 2. 决策总表

| # | 语句变体 | 现状 | 决策 | 主要成本 |
|---|---|---|---|---|
| 1 | `Search`（全文检索） | 死端 | **实施**——执行器已完备，只缺 planner | 写 planner + 元数据解析 |
| 2 | `LookupFulltext` | 死端 | **实施**——同上 | 同上 |
| 3 | `MatchFulltext` | 死端 | **实施**——同上 | 同上 |
| 4 | `DropFulltextIndex` | 死端 | **实施**——同上 | 同上 |
| 5 | `AlterFulltextIndex` | 死端 | **实施**——同上 | 同上 |
| 6 | `ShowFulltextIndex` | 死端 | **实施**——同上 | 同上 |
| 7 | `DescribeFulltextIndex` | 死端 | **实施**——同上 | 同上 |
| 8 | `LoadFrom` | 死端 | **实施（受限形态）**——执行器对 file/glob/表函数已完备；`(query)` 源与 `RETURN` 子句无法执行，从语法中移除 | 写 planner + 收紧 parser |
| 9 | `InQueryCall`（`CALL`） | 死端 | **实施（先定契约）**——执行器有若干语义缺口，一并修 | 写 planner + 修执行器 |
| 10 | `Extension`（4 个动作） | 死端 | **显式拒绝**——能力已由嵌入式 Session 与 HTTP 端点提供，SQL 路径不重复实现；同时修复注册表归属不一致 | 2 文件 + 注册表统一 |
| 11 | `Migrate`（3 个子句） | 死端 | **显式拒绝**——能力已由 `linkrs-migration` + gRPC/HTTP/嵌入式提供；顺带清理无人产出的孤儿执行器 | 2 文件 + 死代码清理 |
| 12 | `ClearSpace` | 不可达 | **实施**——除 parser 规则外全链路完备且真的会删数据 | 补 parser 规则 + 结果行 + 权限 |
| 13 | `Query` | 不可达 | **移除**——纯遗留外壳，无任何生产者与消费者 | 删枚举变体与工厂方法 |

**决策分布**：9 项实施、2 项显式拒绝、1 项移除、1 项（`Extension`）实施与否取决于注册表归属的裁决。

---

## 3. 逐项决策与依据

### 3.1 全文检索 7 项（`Search` / `LookupFulltext` / `MatchFulltext` / `DropFulltextIndex` / `AlterFulltextIndex` / `ShowFulltextIndex` / `DescribeFulltextIndex`）

**关键事实：执行器侧 100% 完备，缺的只有 planner。**

已就位（逐一核对）：

| 环节 | 位置 |
|---|---|
| 4 个检索 plan node | `planning/plan/core/nodes/search/fulltext/data_access.rs`（`FulltextSearchNode` / `FulltextLookupNode` / `MatchFulltextNode`） |
| 4 个管理 plan node | `planning/plan/core/nodes/search/fulltext/management.rs`（`Create` / `Drop` / `Alter` / `Show` / `Describe`） |
| plan node → spec 转换 | `executor/streaming/plan/arena_builder/specs/ddl.rs`（`build_fulltext_manage_spec` / `build_fulltext_search_spec` / `build_fulltext_lookup_spec` / `build_match_fulltext_spec`） |
| spec → operator | `executor/streaming/operators/ddl_operator.rs`（`DdlOperatorKind` 4 个变体 + `from_spec` + `next` 分派） |
| operator 实现 | `executor/streaming/operators/fulltext_operator.rs`（`FulltextManage` / `FulltextSearch` / `FulltextLookup` / `MatchFulltext`） |
| 管理命令枚举 | `executor/streaming/operators/spec/fulltext.rs`（`FulltextManageCommand` 含 Create/Drop/Alter/Show/Describe） |
| 逻辑→物理映射 | `planning/physical_planner/search.rs` |

**为什么现在是死端**：

- `FulltextSearchPlanner`（`planning/fulltext_planner.rs`）只处理 `BoundStatement::CreateFulltextIndex`；其余 6 个变体在 binder 里落入兜底分支变成 `BoundStatement::Other`。
- `PlannerEnum::from_bound_statement`（`planning/planner.rs`）的 `Other` 分支没有全文检索的任何分支，落到 `_ => None`。
- 更严重的是：该 planner 里 `transform_lookup_fulltext_with_metadata` 等函数**整体带 `#[cfg(test)]`**——它们只在单元测试里存在，产物中是死代码。
- `DropFulltextIndexNode` / `AlterFulltextIndexNode` / `ShowFulltextIndexNode` / `DescribeFulltextIndexNode` **从无任何 planner 构造**（已逐一检索确认）。

**实施路径**：照搬向量 DDL 的成熟范式。`VectorDdlPlanner`（`planning/vector_ddl_planner.rs`）已经解决了完全相同的问题——`Create` 走 bound 分支，`Drop`/`Show`/`Describe` 走 AST 回退分支（`transform_bound`）。全文检索照此办理即可，不需要新设计。

**必须一并处理的语义点**：

1. **索引位置解析**。`FulltextSearchNode` 需要 `space_id` / `tag_name` / `field_name` 三个预解析字段。`MetadataContext` 已提供 `get_index_metadata` 与 `find_fulltext_index_by_field`。索引不存在时必须报"索引不存在"，而不是用空字符串继续。
2. **`ALTER FULLTEXT INDEX REBUILD`**。planner 里已有常量 `FULLTEXT_REBUILD_UNAVAILABLE` 说明该动作不能内联执行（重建需要 sync driver + 主存储扫描）。该说明目前带 `#[cfg(feature = "fulltext")]`，实施后必须**在特性关闭时也给出可读错误**，否则会退化成"无 planner"。
3. **特性门控**。`fulltext` 不在默认特性里。特性关闭时，7 个语句必须给出"该特性未启用"这类可读信息，绝不能是 `No planner for bound statement`。
4. **`SHOW` / `DESCRIBE` 的结果列**。参照向量侧 `ShowVectorIndexNode` / `DescribeVectorIndexNode` 的列定义，保持两套索引的目录输出形状一致。

### 3.2 `LOAD FROM`

**关键事实：执行器对三种可执行源已完备，另两种形态根本无法执行。**

`LoadFromStmt.source` 是四态枚举：`File` / `Glob` / `TableFunc` / `Query`。

| 源 | 执行器状态 | 位置 |
|---|---|---|
| `File` | 完备（CSV 读取、`header`/`delimiter` 选项） | `maintenance_executor.rs` 的 file 分支 |
| `Glob` | 完备（通配、多文件合并、取首个文件的表头） | 同上 glob 分支 |
| `TableFunc` | 完备（走函数注册表的 `execute_table_function`；当前只注册了 `read_csv`） | 同上 table_func 分支 |
| `Query` | **不可执行**——executor 直接报"必须在计划期展开"，而 `LoadFromNode` 是叶子节点（`ZeroInputNode`），结构上无法挂子计划 | 同上 query 分支 + `system_nodes.rs` |

另有三处执行器与 AST 的不对齐：

- **`RETURN` 子句**：parser 解析了（支持 `RETURN *` 与 `DISTINCT`），但 `LoadFromNode` 没有对应字段，arena builder 生成的片段是 terminal 且无下游输出，**无处安放投影**。
- **`WITH HEADERS` 的类型**：`LoadHeaderColumn.data_type` 被丢弃，plan node 只存列名，所有值一律按字符串返回。
- **表函数实参**：executor 把 `func_args_json` 按 `Vec<String>` 解析再强转字符串，非字面量实参（如 `read_csv($path)`）传进去的是源码文本而不是运行时值。
- **零列 schema 缺陷**：file 分支在既无 `headers` 又无表头行时产出零列 schema，但行里仍有 N 个值——`DataChunk::new` 不校验宽度，属于潜在不一致。

**决策**：实施 file/glob/table_func 三种源；`(query)` 源与 `RETURN` 子句**从语法中移除**，让它们在 parse 层就报明确的"不支持"错误，而不是静默忽略。

理由：(query) 源与 RETURN 都需要改 plan node 的输入形态（叶子→单输入）与 arena builder 的片段模型，不是补一个 planner 能解决的；而这两者当前**零使用**（唯一引用是 parser 测试与语料），移除代价极低。保留它们只会制造"能写不能跑"的语法。

**顺带修复**：表函数实参改为计划期字面量求值（与 `CALL` 的契约统一，见 3.3）；零列 schema 缺陷按首个数据行的宽度补齐列名。

### 3.3 `CALL`

**关键事实：执行器有五个内置函数分支 + 一个标量函数兜底，但存在四处语义缺口。**

executor 的分支：`db_version` / `db_current_space` / `db_property_graph` / `list_labels` / `list_relationship_types`，外加落到标量函数注册表的兜底。

缺口：

1. **实参契约未定义**。executor 期望 `args_json` 是 `Vec<serde_json::Value>`（**值**，不是表达式树）。planner 手里是 `ContextualExpression`。现有 `SerializableExpression` 序列化的是**表达式树**而非求值结果，没有任何现成工具能把表达式转成该契约要求的 JSON 值。
2. **未知函数被吞掉**。兜底分支把 `registry.execute` 的错误丢弃，返回空结果集。用户拼错函数名会得到"成功但 0 行"，而不是"函数不存在"。
3. **字符串被强制转型**。`json_to_value` 对 JSON 字符串会尝试解析成整数/浮点/布尔/null。`CALL f('123')` 会得到数字 123 而不是字符串 "123"。
4. **`YIELD` 子句被丢弃**。parser 支持 `YIELD` 的表达式、别名、`WHERE`、`ORDER BY`、`LIMIT`、`SKIP`、`SAMPLE`，但 plan node 只承载 `yield_items`，executor **只用别名当列名**，表达式与其余子句全部丢失。
5. 附带：`list_labels` / `list_relationship_types` 用自己的列名（`label` / `relationship_type`），忽略 YIELD 别名；`list.relationship_types` 这个带点的分支从 SQL 不可达（parser 只接受裸标识符）。

**决策**：实施，但**必须先定契约**，再写 planner：

- **实参契约**：计划期字面量求值。planner 把每个实参表达式求值成 JSON 值；非字面量实参明确拒绝（"CALL 的实参必须是字面量"）。这与内置函数的实际需要吻合（五个内置函数都不吃参数），也避免了把表达式求值器塞进 DDL operator。
- **`YIELD` 收敛为列名列表**：只接受标识符列表（可带别名）。`WHERE`/`ORDER BY`/`LIMIT`/`SKIP`/`SAMPLE` 与表达式形态的 YIELD 从 parser 拒绝。
- **未知函数必须报错**：兜底分支的错误不得丢弃。
- **字符串不得被强制转型**：`json_to_value` 的字符串分支只做字面量解析的反向（字符串保持字符串）。

### 3.4 `EXTENSION`

**关键事实：能力已完备且被正确使用，SQL 路径只是没接上；真正的问题是注册表归属不一致。**

能力分布：

| 动作 | 已实现 | 使用路径 |
|---|---|---|
| `LOAD EXTENSION '<path>'` | `UdfLoader::load`（`libloading` 动态库加载 + ABI 版本探测 + 符号解析） | 嵌入式 `Session::execute_extension`、HTTP `/v1/functions` |
| `INSTALL EXTENSION <name> FROM '<src>'` | `UdfLoader::install_from_source`（仅本地路径，远程仓库返回 `RepoDownloadUnsupported`） | 同上 |
| `UNINSTALL EXTENSION <name>` | 注册表 `unload_dynamic_udf` | 同上 |
| `UPDATE EXTENSION <name>` | 注册表 `reload_dynamic_udf`（按 mtime 判定，失败保留旧版本） | 同上 |

读取侧 `SHOW EXTENSIONS` 已可计划可执行。

**为什么仍是死端**：binder 落入兜底 → `BoundStatement::Other` → planner 的 `Other` 分支无 `Extension` → `None` → `No planner for bound statement: LOAD EXTENSION`。

**实际暴露面**：嵌入式 Session 与网络服务都在到达 pipeline **之前**就拦截了（Session 按 AST 变体分派到 `execute_extension`；网络服务直接拒绝并给出"Extension statements are not supported by the network service"）。只有两条路径会漏出内部错误：`QueryApi::execute`（不经命令拦截）与直接调用 `QueryPipelineManager::execute_query`。

**决策**：显式拒绝（照 `KILL QUERY` 范式），不实现 SQL 路径。理由：

- SQL 路径要操作函数注册表，但注册表归属存在分歧：嵌入式 Session 用**会话级**注册表（`Arc<RwLock<FunctionRegistry>>`，每会话新建），而查询引擎只有**进程级** `global_registry()`。`SHOW EXTENSIONS` 读的是后者。
- **这导致一个真实缺陷**：经嵌入式 Session `LOAD EXTENSION` 加载的扩展，在同一 Session 里 `SHOW EXTENSIONS` **看不到**（读的是全局注册表）。这是本阶段顺带必须修的一致性问题，与 SQL 路径是否实现无关。
- 统一注册表归属后，"到底要不要 SQL 路径"依然成立：扩展生命周期是会话/进程级资源管理，与 `KILL QUERY` 同类，交给会话层与服务层更合适。

**因此本项包含两件事**：(a) planner 层给明确拒绝信息；(b) 修复注册表归属，使 `SHOW EXTENSIONS` 与加载方看到同一份状态。

**附带清理**：`SessionCommand` 枚举目前**完全是装饰性的**——两个调用点（嵌入式 `execution.rs`、网络服务 `command.rs`）都丢弃了 kind，改按 AST 变体分派；`SessionCommand::Extension` 是个从未被消费的变体，造成"已接通"的假信号。要么让分派真正走这个枚举，要么删掉它。

### 3.5 `MIGRATE`

**关键事实：能力在 `linkrs-migration` + gRPC + HTTP + 嵌入式 + C API 全线完备；查询 crate 里还有一份写好了却无人产出的执行器。**

- 能力主体：`linkrs-migration` 提供 plan 生成、执行、回滚、备份、文件锁、进度事件。
- 对外接口：gRPC（`crates/linkrs-server/src/grpc/migration.rs`，含 plan/execute/rollback/dry-run/history/status/progress 流）、HTTP（`/v1/migration/*`）、嵌入式 `GraphDatabase`、C API `linkrs_migration_execute`。
- SQL 侧的 AST 字段与 gRPC 请求消息**一一对应**（`space`/`label`/`is_edge`/`from_version`/`to_version`/`plan_json`），说明当初就是照着 RPC 设计的，只是没接。
- **孤儿执行器**：`DdlSpec::Migrate*` / `DdlOperatorKind::Migrate*` / `executor/streaming/operators/ddl_operator/migration_executor.rs` 里三个函数（`execute_migrate_plan` / `execute_migrate_run` / `execute_migrate_rollback`）实现完整、调用 `linkrs-migration` 的真实 API，但**没有 plan node、没有 arena builder 分支**，任何计划都到不了它们。

**决策**：显式拒绝 + 清理孤儿执行器。

理由：

- SQL 路径用的是 `linkrs_migration::execute_migration_plan`（离线执行），而 RPC 路径用的是 `execute_online_migration`（带在线 drain 协议与进度事件）。二者语义不等价。走 SQL 会得到一个能力更弱的迁移，却让用户以为与 RPC 等价。
- 权限也不等价：gRPC/HTTP 的执行与回滚要求 **Admin**，而 SQL 侧的 guard 映射是 **Schema**。接通 SQL 会**放宽**权限。
- SQL 语法本身还带一个语义缺口：`MIGRATE EXECUTE '<plan_json>'` 依赖手写 JSON 字符串传递迁移计划，跨进程不可靠。

**因此**：planner 层给出明确拒绝信息并指向 `/v1/migration/*`；孤儿执行器一并删除（无产出者即死代码，符合项目"不保留不可达分支"的立场）。`MigrateAction::MigrateSpace` 是另一个不相干的东西（只调 `save_to_disk`），单独评估。

### 3.6 `CLEAR SPACE`

**关键事实：除 parser 规则外全链路完备，而且它真的会删光数据与模式。**

已就位：

- binder：`bind_clear_space` → `BoundClearSpace`
- planner：`PlannerEnum::Maintain` → `system::plan_clear_space` → `ClearSpaceNode`
- executor：`SpaceManageCommand::Clear` → `StorageSchemaOps::clear_space`
- 存储实现（`linkrs-storage/src/engine/graph_storage/schema_writer.rs`）：写 WAL redo 记录 → **丢弃该空间全部边类型存储与全部标签存储** → 级联删除索引 → 清空 schema manager 的标签/边注册表 → 重置序列分配器。

**它保留空间本身，但清空该空间内的所有数据与所有标签/边模式**——破坏性与 `DROP SPACE` 相当。

**为什么不可达**：lexer 有 `TokenKind::Clear` 关键字，但 `TokenKind::Clear` 在 parser 的关键字分派与 token 分派里**都没有分支**，于是 `CLEAR SPACE x` 在 parse 层就报 "Unexpected token: Clear"。整条 binder/planner/executor 链路是死代码。

而 tree-sitter 语法**有**这条规则（`clear_space_segment`），语法映射脚本也把它登记为 `ClearSpace` 的映射段——于是语法层宣传支持、运行时不支持。

**决策**：实施。理由：

- 实现已经存在且正确，只差一条 parser 规则。
- 存储层 `clear_space` 是合法原语，删掉 SQL 表面并不会让它消失，只是让功能无法使用。
- 与 `DROP SPACE` 的差别正是"保留空间壳、清空内容"，这是一个真实且有用的操作（重建场景）。

**实施要点**：

1. 补 parser 规则。AST 的 `space_name` 是必填 `String`，而 tree-sitter 的 `clear_space_segment` 允许省略名字。**决策**：两者对齐为都支持——`CLEAR SPACE`（当前空间）与 `CLEAR SPACE <name>`（指定空间），AST 字段改为可选。
2. **补结果行**。当前 executor 返回空操作（`Ok(())`），用户拿不到任何反馈。参照 `PURGE EXPIRED` 的做法，返回一行结果：空间名、被清空的标签/边类型数、被删除的数据量。
3. **权限对齐**。当前 guard 映射是 `Schema`，而破坏性相当的 `DROP SPACE` 要求 `Admin`。`CLEAR SPACE` 应提到 `Admin`。
4. 语法层无需改动（`clear_space_segment` 已存在且名字可选），但需要确认新增的"带名字"形态在 tree-sitter 侧同样可解析——`clear_space_segment` 用的是 `optional($.tail)`，能覆盖。

### 3.7 `QUERY`

**关键事实：纯遗留外壳。**

- `Stmt::Query(QueryStmt)` 的 `QueryStmt` 只有一个 `statements: Vec<Stmt>` 字段。
- `StmtFactory::query`（`parser/ast/utils.rs`）从未被任何调用点使用。
- 没有 parser 规则、没有 binder 分支、没有 planner 分支、没有 plan node、没有 executor。
- `StmtCategory::Query` 的文档注释写着"Subquery / compound statement wrappers (QUERY / PIPE / SET OPERATION / ASSIGNMENT)"，但 `PIPE` / `SET OPERATION` / `ASSIGNMENT` 各自都是独立变体，`QUERY` 是多出来的一个。
- tree-sitter 语法里 `Query` 被登记为允许清单项（`RULE_ALLOWLIST`），即语法层也只是名义映射。

**决策**：移除。包括 `Stmt::Query` 变体、`QueryStmt` 结构、`StmtFactory::query` 工厂方法、`define_stmt_helpers!` 里的 `Query => Query` 条目、以及 `Stmt::kind()` 中的对应分支。同时清理语法映射脚本里的允许清单项与 `StmtCategory::Query` 的文档注释（该类别仍有 `Pipe`/`Assignment`/`SetOperation` 三个成员，保留类别本身）。

---

## 4. 实施任务分解

任务按依赖排序。标注"⚠ 涉及生产代码语义变更"的任务需要单独评审。

### 4.1 任务组 A：全文检索（7 个语句）

| 任务 | 内容 | 落点 |
|---|---|---|
| **A1** | 把 `FulltextSearchPlanner` 从"只处理 Create"扩展为覆盖 7 个变体：新增 AST 回退分派（照 `VectorDdlPlanner::transform_bound` 的形态），把 `#[cfg(test)]` 的 transform 函数提升为生产代码 | `planning/fulltext_planner.rs` |
| **A2** | 在 `PlannerEnum::from_bound_statement` 的 `Other` 分支为 7 个全文检索变体加上路由（管理类 4 个 → `FulltextSearchPlanner`；检索类 3 个 → `FulltextSearchPlanner`，由 A1 内部再分派） | `planning/planner.rs` |
| **A3** | 索引位置解析：用 `MetadataContext::get_index_metadata` / `find_fulltext_index_by_field` 解析 `space_id` / `tag_name` / `field_name`；索引不存在时报"索引不存在" | `planning/fulltext_planner.rs` |
| **A4** | 特性关闭时的错误路径：`fulltext` 特性未启用时，7 个语句给出"该特性未启用"的可读信息；`ALTER FULLTEXT INDEX REBUILD` 在两种情形下都给出 `FULLTEXT_REBUILD_UNAVAILABLE` 的完整说明（当前该常量带特性门控，实施后需在关闭时也可用） | `planning/fulltext_planner.rs` |
| **A5** | ⚠ 语义测试：7 个语句的执行级测试（见第 5 节） | `tests/search/fulltext.rs` |

### 4.2 任务组 B：`LOAD FROM`

| 任务 | 内容 | 落点 |
|---|---|---|
| **B1** | 新建 `LOAD FROM` planner：把 `ScanSource` 映射成执行器约定的源种类字符串（`file` / `glob` / `table_func`），选项与表头列 1:1 透传，用 `SubPlan::from_single_node` 产出物理-only 叶子（与 `MaintainPlanner` 同形态） | 新增于 `planning/statements/dql/` |
| **B2** | 在 `PlannerEnum::from_bound_statement` 路由 `Stmt::LoadFrom` | `planning/planner.rs` |
| **B3** | ⚠ 从 parser 移除 `(query)` 源：删掉 `ScanSource::Query` 变体、`ScanSource` 的手写 `PartialEq` 里对应的比较分支、parser 里消费 `(` 的分支；parser 遇到该形态时报明确的"不支持"错误 | `parser/ast/stmt/dql.rs`、`parser/parsing/stmt_parser/admin.rs` |
| **B4** | ⚠ 从 parser 移除 `RETURN` 子句：`LoadFromStmt.return_clause` 字段、`ReturnClause` 的相关构造、parser 里消费 `RETURN` 的分支一并删除 | `parser/ast/stmt/dql.rs`、`parser/parsing/stmt_parser/admin.rs` |
| **B5** | ⚠ 修正表函数实参：planner 侧做字面量求值并序列化成 JSON 值数组；executor 侧改为按 `Vec<serde_json::Value>` 解析并保留值的原始类型（不再把字符串强转数值） | `planning/statements/dql/`（新增 planner）、`executor/streaming/operators/ddl_operator/maintenance_executor.rs` |
| **B6** | ⚠ 修正零列 schema 缺陷：file/glob 分支在既无声明表头又无文件表头时，按首个数据行的宽度生成 `col0..colN` 列名 | 同上 executor 文件 |
| **B7** | ⚠ 决定 `WITH HEADERS` 的类型声明去留：要么 plan node 与 spec 承载类型并由 executor 按类型产出值，要么从 parser 移除类型语法（当前 `data_type` 被静默丢弃）。**倾向移除**：执行器所有列一律按字符串，承载类型会产生"声明了类型但值仍是字符串"的假承诺 | `parser/ast/stmt/dql.rs`、`parser/parsing/stmt_parser/admin.rs`、`planning/plan/core/nodes/management/system_nodes.rs` |
| **B8** | 语义测试：三种源各自的成功路径 + 错误路径（文件不存在、glob 无匹配、表函数不存在、表头数不匹配） | `tests/dql/load_from.rs` |

### 4.3 任务组 C：`CALL`

| 任务 | 内容 | 落点 |
|---|---|---|
| **C1** | ⚠ 定义并实现实参契约：planner 对每个实参做计划期字面量求值，序列化为 JSON 值数组；非字面量实参明确拒绝（"CALL 的实参必须是字面量"）。executor 侧保留值类型 | 新增 planner、`executor/streaming/operators/ddl_operator/maintenance_executor.rs` |
| **C2** | ⚠ 兜底分支不得吞错：`registry.execute` 失败时把错误原样抛出（未知函数 → "函数不存在"），不得返回空结果集 | 同上 executor 文件 |
| **C3** | ⚠ 修正字符串强制转型：`json_to_value` 的字符串分支保持字符串语义，去掉整数/浮点/布尔的反向解析 | 同上 executor 文件 |
| **C4** | ⚠ 收敛 `YIELD`：parser 只接受标识符列表（可带别名）；`YIELD` 的表达式形态与 `WHERE`/`ORDER BY`/`LIMIT`/`SKIP`/`SAMPLE` 从 parser 拒绝 | `parser/parsing/stmt_parser/database.rs`、`parser/ast/stmt/dql.rs` |
| **C5** | ⚠ 内置函数尊重 YIELD 别名：`list_labels` / `list_relationship_types` 的列名改用 YIELD 别名；删除从 SQL 不可达的 `list.relationship_types` 带点分支 | 同上 executor 文件 |
| **C6** | 新建 `CALL` planner + 路由（`SubPlan::from_single_node` 形态） | 新增于 `planning/statements/dql/`、`planning/planner.rs` |
| **C7** | 语义测试：五个内置函数各自的结果内容、YIELD 别名、未知函数的错误、实参类型保真 | `tests/dql/call.rs` |

### 4.4 任务组 D：`EXTENSION`

| 任务 | 内容 | 落点 |
|---|---|---|
| **D1** | planner 层显式拒绝：在 `PlannerEnum::from_bound_statement` 的 `Other` 分支为 `Stmt::Extension` 加上路由到 `MaintainPlanner`，并在 `MaintainPlanner` 里返回 `UnsupportedOperation`，文案指向嵌入式 Session API 与 HTTP `/v1/functions` 端点（照 `KILL QUERY` 范式） | `planning/planner.rs`、`planning/statements/ddl/maintain_planner.rs` |
| **D2** | ⚠ 统一函数注册表归属：使扩展加载方与 `SHOW EXTENSIONS` 看到同一份注册表。**须先裁决方向**——会话级还是进程级。当前会话用会话级注册表、查询引擎用进程级注册表，导致"加载后 SHOW 看不到"。两种方向各有代价（会话级需把注册表注入执行上下文；进程级需收敛会话的隔离语义），**这是一个需要显式决策的架构点，不是纯实现细节** | `crates/linkrs-api/src/embedded/session.rs`、`crates/linkrs-api/src/embedded/session/extension.rs`、`crates/linkrs-query/src/executor/expression/functions/registry.rs` |
| **D3** | ⚠ 清理 `SessionCommand` 装饰性枚举：让分派真正走该枚举，或删除它（含 `Extension` 变体）。当前两个调用点都丢弃 kind | `crates/linkrs-api/src/api_core/session_command.rs`、`crates/linkrs-api/src/embedded/session/execution.rs`、`crates/linkrs-server/src/graph_service/command.rs` |
| **D4** | 测试：嵌入式 Session 加载扩展 → 同会话 `SHOW EXTENSIONS` 可见 → 卸载后不可见（这条正是 D2 的验收） | `crates/linkrs-api/src/embedded/session/` 内联测试或 `tests/` 集成测试 |

### 4.5 任务组 E：`MIGRATE`

| 任务 | 内容 | 落点 |
|---|---|---|
| **E1** | planner 层显式拒绝：路由 `Stmt::Migrate` 到 `MaintainPlanner`，返回 `UnsupportedOperation`，文案指向 `/v1/migration/plan`、`/execute`、`/rollback` 三个端点与 gRPC 对应方法 | `planning/planner.rs`、`planning/statements/ddl/maintain_planner.rs` |
| **E2** | ⚠ 删除孤儿执行器：`DdlSpec::Migrate*`、`DdlOperatorKind::Migrate*`、`executor/streaming/operators/ddl_operator/migration_executor.rs` 三个函数、相关状态映射与名称映射。这些代码无任何 plan node 可达 | `executor/streaming/operators/spec/ddl.rs`、`executor/streaming/operators/ddl_operator.rs`、`executor/streaming/operators/ddl_operator/migration_executor.rs`、`executor/streaming/operators/state.rs`、`executor/streaming/plan/arena_builder/metadata.rs` |
| **E3** | ⚠ 单独评估 `MigrateAction::MigrateSpace`（只调 `save_to_disk`，与迁移无关）：保留并接线，或删除 | 同 E2 相关文件 |
| **E4** | 测试：三个 MIGRATE 子句形态都返回明确拒绝信息，且信息里含可用的端点指引 | `tests/admin/maintenance.rs` |

### 4.6 任务组 F：`CLEAR SPACE`

| 任务 | 内容 | 落点 |
|---|---|---|
| **F1** | 补 parser 规则：`TokenKind::Clear` 的分派分支 + `parse_clear_space_statement`（支持 `CLEAR SPACE` 与 `CLEAR SPACE <name>` 两种形态） | `parser/parsing/stmt_parser/dispatch.rs`、`parser/parsing/stmt_parser/admin.rs` |
| **F2** | ⚠ AST 字段改为可选空间名（`String` → `Option<String>`），binder 与 plan node 同步；缺省时解析为当前空间 | `parser/ast/stmt/ddl.rs`、`binder/bind/bind_ddl.rs`、`binder/bound/stmt_ddl.rs`、`planning/plan/core/nodes/management/space_nodes.rs` |
| **F3** | ⚠ 补结果行：executor 从"空操作"改为返回一行（空间名、被清空的标签数、被清空的边类型数、被删除的数据量），参照 `PURGE EXPIRED` 的形态 | `executor/streaming/operators/ddl_operator/schema_executor/space.rs` |
| **F4** | ⚠ 权限对齐：`CLEAR SPACE` 的 guard 权限从 `Schema` 提到 `Admin`，与破坏性相当的 `DROP SPACE` 一致 | `crates/linkrs-server/src/graph_service/guard.rs` |
| **F5** | 测试：清空后数据为空、标签/边模式为空、空间仍在；两种语法形态；空间不存在时报错 | `tests/ddl/space_basic.rs` |

### 4.7 任务组 G：`QUERY` 移除

| 任务 | 内容 | 落点 |
|---|---|---|
| **G1** | 移除 `Stmt::Query` 变体、`QueryStmt` 结构及其 `PartialEq`/`new`、`StmtFactory::query`、`define_stmt_helpers!` 的 `Query => Query` 条目、`Stmt::kind()` 的 `Query` 分支 | `parser/ast/stmt.rs`、`parser/ast/stmt/dql.rs`、`parser/ast/utils.rs` |
| **G2** | 清理语法映射脚本里的 `Query` 允许清单项；`StmtCategory::Query` 的文档注释改为只列 `Pipe` / `SetOperation` / `Assignment` | `scripts/check-grammar-structure.py`、`parser/ast/stmt.rs` |
| **G3** | 确认 tree-sitter 语法没有 `QUERY` 关键字规则（若有，一并移除，避免语法层继续宣传） | `crates/linkrs-grammar/grammar.js`、`crates/linkrs-grammar/data/keywords.json` |

### 4.8 任务组 H：可达性契约测试（收口）

| 任务 | 内容 | 落点 |
|---|---|---|
| **H1** | 建立语句清单常量（SQL 文本 + 期望 `kind()` + 期望 `StmtCategory` + 期望状态），每个 `Stmt` 变体一条 | 新增 `tests/common/statement_inventory.rs` |
| **H2** | 可达性契约测试：清单里每条语句按期望状态断言——`Executable` 必须成功且有内容级断言；`Rejected` 必须失败且错误信息落在用户可理解的类别内（**`No planner for bound statement` 不合格**）；`Unreachable` 必须在 parse 层被拒 | 新增 `tests/statement_reachability.rs` |
| **H3** | 清单与枚举的一致性断言：条目数必须等于 `Stmt` 变体数，新增变体不入清单即失败 | 同 H2 |
| **H4** | 语义测试补齐：新增实现的 9 个语句（全文检索 7 + `LOAD FROM` + `CALL` + `CLEAR SPACE`）各自的成功路径、错误路径、边界路径 | `tests/search/fulltext.rs`、`tests/dql/load_from.rs`、`tests/dql/call.rs`、`tests/ddl/space_basic.rs` |

### 4.9 任务组 I：语料测试（见第 5 节）

| 任务 | 内容 | 落点 |
|---|---|---|
| **I1** | 语料按"结构化"与"tail 覆盖"两类拆分，对 tail 覆盖的语句显式断言树中出现 `tail` 节点，让"grammar 只是不报错、并未建模"这件事显性化 | `tests/corpus_parity.rs` |
| **I2** | 语料去重（当前 111 条里约 30 条重复），并把 `covered > 50` 的松散下界改成精确值 | 同上 |
| **I3** | 语料从 Rust 源码内联迁到 `tests/corpus/*.gql`，便于语义 runner 复用同一份清单 | `tests/corpus/`、新增 runner |
| **I4** | ⚠ 修复失效的快照门禁：`scripts/verify-grammar.sh` 里对 `crates/linkrs-grammar/outputs/` 的 `git diff --exit-code` 永远看不到未跟踪文件（该目录被 gitignore 且不存在），导致 token 分类漂移无人拦截。改为把快照纳入版本控制或改用不依赖 git 的比对方式 | `scripts/verify-grammar.sh`、`.gitignore` |

---

## 5. 关于 `corpus_parity.rs` 的处置

**结论：不删除，但必须加固。** 用户的判断方向是"如果无实际意义可以删"，核查后的事实是：它有意义，但当前形态下**大部分断言是空的**。

### 5.1 它不是文本提取的残留

"tree-sitter 生成时依赖文本提取"指的是**词表抽取**这条链路——`crates/linkrs-query/examples/export_keyword_inventory.rs` 把 Rust lexer 的关键字导出成 `crates/linkrs-grammar/data/keywords.json`，语法文件再由它生成 `kw_*` 规则。这是**另一条链路**，由 `scripts/verify-grammar.sh` 的前几步守护，与 `corpus_parity.rs` 无关。

`corpus_parity.rs` 做的是**跨解析器行为比对**：断言"手写 parser 能接受的语句，tree-sitter 语法也能接受"。

### 5.2 语法是活能力，不是死代码

| 消费方 | 用途 |
|---|---|
| 前端 Monaco 编辑器 | `frontend/vite/grammarWasm.ts` 编译 wasm，`frontend/src/lib/utils/treeSitterCypher.ts` 注册 `SemanticTokensProvider`，用 `queries/highlights.scm` 做语法高亮 |
| CLI | 只用 `inventory_json()` 做补全候选，**没有**接入语法解析；CLI 高亮仍是手写状态机 |
| 服务端二进制 | **完全不用** |

所以"手写 parser 接受、语法不接受"是一类**用户可见缺陷**：编辑器会把一条合法的查询标红。这正是该测试守护的东西。

### 5.3 但它现在是个空门禁

语法里有一条兜底规则 `tail`（匹配除换行外的任意文本），在 59 个语句段里被 37 个引用。后果是：相当一部分语料语句整条被吞成一个不透明 token，语法只是"没报错"，并没有建模。

**本次实测**：对语料的 81 条去重语句逐条解析并检查语法树，**34 条（42%）出现了 `tail` 节点**，即被兜底规则整条吸收。被吸收的清单如下：

```
LOOKUP ON TAG player YIELD id(vertex) AS vid      SEARCH INDEX 'term'
LOOKUP ON player YIELD id(vertex) AS vid          SEARCH 'term'
SHOW CREATE TAG person                            SEARCH VECTOR 'a'
BEGIN TRANSACTION READ ONLY                       MATCH FULLTEXT 'term'
CREATE USER alice WITH PASSWORD 'p'               CALL foo()
CREATE FULLTEXT INDEX idx ON tag(field)           LET x = 1
CREATE VECTOR INDEX idx ON tag(field)             $x = MATCH (a) RETURN a
CREATE MACRO f(x) AS x                            ROLLBACK TRANSACTION TO SAVEPOINT sp1
CREATE TYPE uid AS INT                            IMPORT DATABASE 'a'
CREATE GRAPH g                                    DROP MACRO f
KILL QUERY 1                                      DROP TYPE uid
SET x = 1                                         DROP FULLTEXT INDEX idx
ATTACH DATABASE 'a'                               SHOW FULLTEXT INDEX
EXPORT DATABASE 'a'                               DESCRIBE FULLTEXT INDEX idx
LOAD EXTENSION 'a'                                DROP VECTOR INDEX idx
MIGRATE PLAN                                      SHOW VECTOR INDEX
PURGE EXPIRED                                     DESCRIBE VECTOR INDEX idx
```

这份清单与本阶段要处理的死端/不可达语句**高度重合**——全文检索的索引管理、向量索引的目录语句、`LOAD EXTENSION`、`MIGRATE`、`CALL`、独立 `SET`、库管理语句，几乎全部落在 tail 覆盖区。**语法层的"支持"与运行时的"不支持"是同一批语句**。这说明当初为了让语料通过而放松语法的做法，恰好掩盖了功能缺口。

更说明问题的是允许清单的演变：git 历史显示语料扩充与语法放宽是**同步**发生的——为了让新语句通过，先给语法加 `tail`、再把语句加进语料，最后清空允许清单。允许清单"只减不增"的健康度指标因此失去意义：它是靠放松语法清空的，不是靠语法建模能力清空的。

对照之下，`crates/linkrs-grammar/tests/smoke.rs` 与 `fixtures/*.gql` 是**另一层**：它们只测语法自身（不碰 `linkrs-query`），且 `fixtures/09_error_cases.gql` 明确用畸形语句验证语法的严格性。两者互补，不是替代关系——`smoke.rs` 察觉不到"后端接受了语法拒绝的东西"。

### 5.4 删除会破坏什么

- `scripts/verify-grammar.sh` 直接调用这个测试目标，删掉会让该脚本**硬失败**。
- `crates/linkrs-query/Cargo.toml` 的 `linkrs-grammar` dev-dependency 变成未使用（Cargo 不报错，但 `cargo machete` 会报）。
- 每次 `cargo test -p linkrs-query` 会少付约 1 分钟的 tree-sitter 运行时 + 9 万行 C 编译成本。
- 设计与实现文档（`docs/plan/tree_sitter_syntax_layer*.md`、`docs/grammer/tree-sitter-generation.md`）把"差分对等测试转入 CI 门禁"列为阶段二目标，删除等于放弃该目标。

### 5.5 处置

按任务 I1–I4 加固：拆分类别、显式断言 tail 节点、去重、精确化计数、迁出内联语料、修复失效的快照门禁。加固后它守护的是**结构化语句**的严格性（那部分是真有价值的），tail 覆盖的语句退化为"语法显式声明为不建模"，不再伪装成"语法支持"。

若仍决定删除，则必须同步：移除 `verify-grammar.sh` 里的调用、移除 dev-dependency、更新四份设计文档的阶段二条目、并在 `scripts/check-grammar-structure.py` 里补上等价的结构覆盖检查。**只删测试文件不补这些，会让语法层失去唯一的跨解析器门禁。**

---

## 6. 实施顺序与依赖

| 序 | 任务组 | 依赖 | 风险 |
|---|---|---|---|
| 1 | A（全文检索） | 无 | 低——执行器完备，纯补路由；主要风险在特性开关下的错误路径 |
| 2 | B（`LOAD FROM`） | A | 中——涉及 parser 收紧与 executor 缺陷修复 |
| 3 | C（`CALL`） | B | 中——实参契约需先定，且要改 executor 的错误传播 |
| 4 | F（`CLEAR SPACE`） | 无 | 低——但含权限提升与结果行两个语义变更 |
| 5 | D（`EXTENSION`） | 无 | **高——D2 是架构裁决，不是实现细节，需要独立决策** |
| 6 | E（`MIGRATE`） | 无 | 低——拒绝 + 删死代码；但 E2 删除的孤儿执行器需先确认无外部引用 |
| 7 | G（`QUERY` 移除） | 无 | 低——纯删除，但需确认 tree-sitter 侧同步 |
| 8 | H（契约测试） | A–G | 低——但必须在前面几组落地后才有稳定断言 |
| 9 | I（语料加固） | 与 A–G 无耦合 | 低 |

**建议**：A、F、G、E 可以并行推进；D 因涉及架构裁决单独走；B 与 C 有契约共性（实参求值），宜连续推进。

---

## 7. 验收标准

1. **死端清零**：`Stmt` 枚举中不存在"能解析但 planner 返回 `None`"的变体。全部 84 个变体的终态为：可执行、明确拒绝、或已移除。
2. **无内部错误泄漏**：全仓测试与手工验证中，`No planner for bound statement` 不再出现在任何面向用户的错误里。
3. **无静默失效**：`LOAD FROM` 的 `(query)` 源与 `RETURN` 子句、`CALL` 的 `YIELD` 过滤/排序/限制、`WITH HEADERS` 的类型声明，要么被实现，要么在 parse 层被明确拒绝——不接受"能写但被忽略"。
4. **错误类型保真**：`CALL` 的字符串实参不得被转成数值；未知函数必须报错而非返回空结果。
5. **权限一致**：`CLEAR SPACE` 与 `DROP SPACE` 同为 Admin；`MIGRATE` 的 SQL 拒绝信息不得暗示 SQL 路径可用。
6. **注册表一致**：同一会话内加载扩展后 `SHOW EXTENSIONS` 必须可见。
7. **契约测试守护**：语句清单覆盖全部变体且条目数与变体数一致；`Rejected` 类断言到具体错误文案。
8. **语义测试到位**：本阶段新增实现的 9 个语句，每个都至少有成功路径 + 错误路径 + 边界路径的内容级断言。
9. **语料门禁有效**：tail 覆盖的语句在语料中被显式标注，结构化语句的严格性由快照断言守护。

---

## 附录 A：关键证据索引

| 主题 | 位置 |
|---|---|
| 语句枚举与分类 | `crates/linkrs-query/src/parser/ast/stmt.rs` |
| 绑定兜底分支（死端根因之一） | `crates/linkrs-query/src/binder/bind.rs` |
| 计划路由（`_ => None` 即死端） | `crates/linkrs-query/src/planning/planner.rs` |
| 无 planner 的错误出口 | `crates/linkrs-query/src/pipeline/compiler.rs` |
| 全文检索 planner（`#[cfg(test)]` 实现） | `crates/linkrs-query/src/planning/fulltext_planner.rs` |
| 全文检索 plan node 与 spec 转换 | `crates/linkrs-query/src/planning/plan/core/nodes/search/fulltext/`、`crates/linkrs-query/src/executor/streaming/plan/arena_builder/specs/ddl.rs` |
| 全文检索 operator 实现 | `crates/linkrs-query/src/executor/streaming/operators/fulltext_operator.rs` |
| 向量 DDL 的 AST 回退范式（全文检索照此办理） | `crates/linkrs-query/src/planning/vector_ddl_planner.rs` |
| `KILL QUERY` 显式拒绝范式 | `crates/linkrs-query/src/planning/statements/ddl/maintain_planner.rs` |
| `LOAD FROM` / `CALL` 执行器 | `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/maintenance_executor.rs` |
| `LOAD FROM` / `CALL` plan node（叶子，无逻辑对应） | `crates/linkrs-query/src/planning/plan/core/nodes/management/system_nodes.rs` |
| `CLEAR SPACE` 存储实现（真删数据与模式） | `crates/linkrs-storage/src/engine/graph_storage/schema_writer.rs` |
| `CLEAR SPACE` parser 缺口（lexer 有 token、无分派） | `crates/linkrs-query/src/parser/lexing/spec.rs`、`crates/linkrs-query/src/parser/parsing/stmt_parser/dispatch.rs` |
| 扩展加载能力 | `crates/linkrs-query/src/executor/expression/functions/udf/` |
| 会话级 vs 进程级注册表分歧 | `crates/linkrs-api/src/embedded/session.rs`、`crates/linkrs-query/src/executor/expression/functions/registry.rs` |
| 迁移能力主体 | `crates/linkrs-migration/`、`crates/linkrs-server/src/grpc/migration.rs`、`crates/linkrs-server/src/http/handlers/schema/migration.rs` |
| `MIGRATE` 孤儿执行器 | `crates/linkrs-query/src/executor/streaming/operators/ddl_operator/migration_executor.rs` |
| 权限映射 | `crates/linkrs-server/src/graph_service/guard.rs` |
| 语料测试与语法兜底规则 | `crates/linkrs-query/tests/corpus_parity.rs`、`crates/linkrs-grammar/grammar.js` |
| 语法门禁脚本 | `scripts/verify-grammar.sh`、`scripts/check-grammar-structure.py` |
| 语法活消费方（前端高亮） | `frontend/src/lib/utils/treeSitterCypher.ts`、`frontend/vite/grammarWasm.ts` |
