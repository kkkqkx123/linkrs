# DDL 定义对齐：分阶段修改方案

对应问题清单：`docs/issue/ddl-schema-definition-audit.md` 及其 12 份分册。

## 总原则

1. **文档是发布契约**。每条不一致二选一：补齐实现，或从语法与文档中同时移除。
   不允许保留"可解析但静默失效"的中间态。
2. **不新增兼容层**。修在原始模块上，不为"修好版本"另起模块。
3. **兜底逻辑不准掩盖缺口**。空名分支、`unwrap_or_default()` 式默认落点一律改为显式错误。
4. **每个阶段以可执行证据收口**：文档中的示例语句必须真实跑通，或语法被真实拒绝。
5. **阶段间可中断**。每阶段结束时仓库必须编译通过、既有测试全绿。

## 阶段划分依据

按"用户可获得正确反馈的紧迫度"排序，而不是按代码层：

- **阶段一**（显形化）：让假能力要么能用、要么明确拒绝。这是最高优先——它决定了后续所有排查成本。
- **阶段二**（接线）：把 parser 已产出、下游丢失的字段贯通。风险集中在 planner/spec/executor，可逐条独立验证。
- **阶段三**（补强）：实现真正缺失但文档承诺的能力。
- **阶段四**（清理）：死代码与文档收尾。

---

## 阶段一：显形化——消除"可解析但静默失效"的语法

范围：`ddl-schema-auto-create-unsupported`、`ddl-ttl-dead-parse-path`、`ddl-drop-semantics-gaps`、
`ddl-fulltext-index-config-dropped`、`ddl-vector-index-scope-mismatch`。

### 任务

1. **Cypher DML Schema 自动创建**
   - 处置：从 `docs/release/03_ddl_schema_definition.md` 删除 §3 整节。
   - `docs/release/02_dml_data_manipulation.md` 增加明确声明：节点/边写入仅支持 `INSERT VERTEX` / `INSERT EDGE`。
   - `binder/bind/bind_ddl.rs:12-25` 的错误信息追加文档指引，说明应改用哪两条语句。
   - `tests/integration_cypher_create.rs` 中命名为 "schema inference" 但只测解析的测试改名，使其与断言一致。

2. **TTL**（已废弃）

   > **状态更新：** 该删除任务的前提（storage 无过期引擎、全链路死代码）已被代码推翻。TTL 现已是从
   > parser → AST → planner → spec → executor（`validate_ttl` 校验）→ 持久化（随 SchemaSnapshot 落盘）→
   > 读路径过滤 → 存储层 purge 接口 → gRPC → 文档 → 测试的完整特性。按总原则 1（文档是发布契约，
   > 文档与代码二选一对齐），正确处置是**保留 TTL**，本任务的删除动作不予执行。连带删除设施同样取消；
   > 实际清理的是 `schema_trait.rs` 中全死无调用方的 `SchemaInfo` trait（整体移除，含 tag.rs / edge.rs 的 impl）。
   > 若后续要对齐"物理删除无生产调用方、拓扑快路径不过滤"两个语义边界，应作为 TTL 专项另行立项。

   - 原处置（不再执行）：从语法层移除（`create_parser.rs` 的解析分支、`ast/stmt/ddl.rs` 的两个字段、文档 TTL 段落与汇总表 TTL 行）。

3. **DROP 语义**
   - 抽出 Space/Graph 判别函数，供 CREATE/USE/DROP 共用；补 `DROP GRAPH`。
   - `DROP SPACE IF EXISTS`：`DropSpaceNode` / `SpaceManageCommand::Drop` 加 `if_exists`，executor 按标志返回空结果。
   - `DROP TAG|EDGE INDEX ... IF EXISTS`：三个分支统一解析 `IF EXISTS` 并贯通。
   - `DROP TAG a, b, c` / `DROP EDGE a, b, c`：删除计划期的 multiple-names 拒绝，executor 逐项删除并逐项汇报。

4. **全文索引**
   - 短期：从文档 §6 删除 `ANALYZER` / `BOOST` / `OPTIONS(...)` 表格与字段级分词器描述，
     parser 中对应子句（`fulltext_parser.rs:96-116,139-196`）改为**显式报错**而非解析后丢弃。
     `ENGINE = BM25` 保留（它已被真正校验）。
   - 索引命名：文档写明向量/全文索引按 (space, schema, field) 定位，或在下阶段接通 index_name。

5. **向量索引适用范围**
   - 文档 §7「标签或边类型」改为「仅顶点标签」；`ON <tag_or_edge_name>` 改为 `ON <tag_name>`。
   - `compression` 在 quantization 非 PRODUCT 时显式报错。
   - `hnsw.validate()` / `quant_cfg.validate()` 的返回值转为错误传播。

### 收口标准

- 文档 §3 删除后，全文检索 `auto creat` / `自动创建` 无残留引用。
- `docs/release/03_ddl_schema_definition.md` 中每个语法块要么有对应的执行层测试，
  要么能用一个统一的"语法收敛"说明解释为何不可用。
- 新增测试：`DROP GRAPH x`、`DROP SPACE IF EXISTS missing`、`DROP TAG INDEX IF EXISTS i`、
  `DROP TAG a, b, c` 四条从 parse→execute 全通。
- 全文 OPTIONS 等不受支持的子句返回明确错误而非解析成功。
- TTL 作为保留特性：文档对读路径过滤范围、purge 需显式调用的描述与代码一致。

---

## 阶段二：接线——贯通 parser 已产出、下游丢失的字段

范围：`ddl-ast-fields-silently-dropped`、`ddl-alter-change-loses-constraints`、
`ddl-show-create-output-incomplete`、`ddl-drop-semantics-gaps` 剩余部分。

### 任务

1. **CREATE SPACE 的 comment**
   - `SpaceManageInfo` / `SpaceManageCommand::Create` 增加 `comment: Option<String>`，
     贯通到 `SpaceInfo::with_comment`。
   - `parse_space_with_params` 遇到未知键显式报错并列出已支持键。
   - `SpaceManageInfo::new()` 的默认 vid_type 与 parser 默认值统一为 `INT64`。

2. **vid_type 归一化**
   - parser 只产出规范字符串（`FIXED_STRING(n)` 形式），修掉 `helpers.rs:246-250` 的裸标识符直通。
   - executor `parse_vid_type_str` 删除 `DataType::String` 兜底，无法识别时报错。

3. **ALTER CHANGE 一致性**
   - `EdgeAlterInfo` 增加 `with_changes`，`EdgeManageCommand::Alter` 增加 `changes` 字段，
     executor 实现 `rename_edge_property`，与 TAG 路径对齐。
   - 文档明确 CHANGE 不保留默认值/注释/可空性（语义冻结为"改名"）。

4. **SHOW CREATE 统一**
   - 四个目标统一输出单列 `create_statement`。
   - TAG 渲染补全约束子句；EDGE 渲染含 `FROM ... TO ...`；SPACE 渲染含 `vid_type` 与 `comment`；
     INDEX 渲染含 `ON <target>(<props>)`。
   - 类型拼写用 `DataType` 的 `Display` 而非 `{:?}`；默认值渲染为 SQL 字面量。
   - 补四个目标的输出断言测试。

### 收口标准

- `CREATE SPACE s(vid_type=FIXEDSTRING32, comment='x')` 后 `DESC SPACE s` 的 vid_type 为
  `FixedString(32)`、comment 为 `x`。
- `CREATE SPACE s WITH partition_num=10` 报错。
- `SHOW CREATE TAG/EDGE/SPACE/INDEX` 四者列名一致，且 TAG 输出可被 parser 重新解析。

---

## 阶段三：补强——实现文档承诺但真实缺失的能力

范围：`ddl-sequence-unsupported`、`ddl-fulltext-index-config-dropped`（真实接线版）。

### 任务

1. **序列**
   - `PlanNodeEnum` 增加 `SequenceManage(SequenceManageNode)`，`SequenceManageInfo` 携带五个参数。
   - spec builder 产出 `DdlSpec::SequenceManage`，参数全量传递（删除 `ddl_operator.rs:580-599` 的 `let _ = (...)`）。
   - executor 接 `rt.sequence_manager()` 的 create/update/drop，`nextval` 类函数取数。

2. **全文索引参数**
   - `FulltextManageCommand::Create` 扩充为携带 `engine_type`、`Vec<{field_name, analyzer, boost}>`、`options`。
   - `linkrs-fulltext` 侧确认 `create_index` 可接收 BM25 参数与 per-field 分词器；若引擎暂不支持，
     则维持阶段一的"显式报错"，不降级为静默。
   - 索引命名语义与 Drop/Show 对齐。

3. **SHOW 能力补齐**
   - `SHOW HOSTS` / `SHOW PARTS` 二选一：定义独立 `ShowTarget` 与真实语义，或从文档删除。

### 收口标准

- `CREATE SEQUENCE s START=1 INCREMENT=2` → `nextval(s)` 返回符合步进的序列值。
- `CREATE FULLTEXT INDEX i ON T(f) ENGINE = BM25 OPTIONS (k1=1.2)` 后索引元数据 k1 == 1.2（或语法被拒绝）。

---

## 阶段四：清理——死代码、兜底分支与文档收尾

范围：`ddl-dead-code-and-unreachable-branches`、`ddl-doc-vs-code-drift`。

### 任务

1. 删除 `create_parser.rs:44-191` 死函数与不可达的重复 SPACE/TAG INDEX/EDGE INDEX 分支。
2. 删除 `describe_parser.rs:70-98` 死函数。
3. 删除 `maintenance.rs:442-459` 的空名兜底分支（parser 保证非空）。
4. `AlterTarget::Space`：补齐 parser 并修 executor 字段读取错误，或整体删除该变体与 `AlterSpaceNode`。
5. 文档收尾：整数类型表补实际位宽、`TIMESTAMP` 标注为 `DATETIME` 别名、补 `TIME` 行、
   删除"省略类型"特性描述、类型别名环检测改为"创建与解析时"、
   SERIAL 补齐三项限制、`DEFAULT` 常量函数明确"解析时求值一次并冻结"。

### 收口标准

- 对 `ddl-doc-vs-code-drift.md` 的九条逐条给出"文档已改"或"代码已改"的结论，无悬空项。
- `cargo clippy` 对 DDL 相关模块无新增告警。

---

## 验证方式（全阶段共用）

1. `cargo test -p linkrs-query` —— DDL 测试集（`tests/ddl/`）。
2. `cargo test -p linkrs-query --test integration_cypher_create` —— 阶段一的拒绝语义。
3. `cargo test -p linkrs-storage -p linkrs-core` —— 确认 TTL 字段与读路径过滤、死 trait 移除未破坏 storage 契约。
4. `cargo test -p linkrs-server` —— gRPC schema 服务（TTL 为保留特性，无需移除验证）。

## 风险与回退

| 阶段 | 主要风险 | 缓解 |
|---|---|---|
| 一 | 全文 OPTIONS 等旧语法被移除后，依赖方继续使用会报错 | 项目处于开发期、无兼容义务；用显式错误替代静默成功 |
| 二 | SHOW CREATE 输出形态变化可能影响 CLI/前端解析 | 同步检查 `crates/linkrs-cli` 与 `frontend` 对输出列的引用 |
| 三 | 序列/全文接线涉及 WAL 与元数据持久化 | 仅在现有序列/全文模块之上接线，不新建持久格式 |
| 四 | 删死代码可能连带删掉被 IDE 搜索遗漏的调用点 | 删除前以 `cargo check --workspace` 全量确认 |
