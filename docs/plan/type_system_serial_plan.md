# SERIAL 自增列正式设计方案

> 状态：设计方案（2026-08-15）。
>
> 前置：`docs/plan/type_system_integrity_plan.md` §2.9（P2-I，设计草案）。
> 本文档把草案落为可实施设计，明确类型表示、序列状态、事务语义与实施边界。

## 0. 结论摘要

| 项 | 决策 |
|----|------|
| 类型表示 | SERIAL **不是** `DataType` 变体，而是 DDL 语法糖 → 绑定期展开为 `BigInt` 列 + 列属性 `serial: bool`（对齐 PostgreSQL 惯例，避免 DataType 枚举/四链路分叉） |
| 序列状态 | 每空间（可选每 tag）一个 `AtomicU64` 计数器 + 持久化 `SERIAL_NEXT` 元数据；启动恢复取 `max(持久化值, 列当前最大值 + 1)` |
| 插入语义 | 未显式提供 serial 列值 → 自动分配；显式提供 → 校验冲突并推进计数器 |
| 事务语义 | **分配即消耗**：undo 回滚不回退计数器（避免 id 重用）；WAL redo 幂等 |
| 约束 | 不可 DEFAULT、每 tag 至多一列、可参与 cast/索引 |
| 与 VID 的关系 | SERIAL 是普通属性列；VID 仍应用托管，本期不自动分配 VID |

## 1. 背景与动机

- 项目定位"轻量单节点本地部署"，但 VID 完全应用托管（`InsertVertexInfo.vertex_id`
  由调用方提供，`data_modification.rs:7-18`），与 README 定位存在张力：
  用户需自管 ID（取号、冲突、单调），而本地场景最常见的需求就是"插进去自动给号"。
- LadybugDB 将 `SERIAL` 作为一等逻辑类型（`LogicalTypeID::SERIAL=13`），
  `_nodes.id` 默认即 SERIAL，绑定器强制"SERIAL 列不可设 DEFAULT"
  （`bind_ddl.cpp:101-111`），并参与 `TO_SERIAL` cast 与 C-API。
- linkrs 全仓无 sequence/auto_increment 概念（grep 为空），类型系统整改（P0/P1）
  已为后续腾出空间：`DataType` 码位 64+ 预留（types.rs），`from_u8` 严格化。

## 2. 目标设计

### 2.1 语法与解析

```
CREATE TAG Person (
    id SERIAL,          -- 展开为 BigInt 属性列 + serial 标记
    name STRING
);
CREATE EDGE FOLLOWS (seq SERIAL) FROM Person TO Person;
```

- 词法：`SERIAL` 关键字（新 `TokenKind::Serial`）。
- DDL 解析（`helpers.rs` 类型解析处）：遇 `SERIAL` 返回
  `DataType::BigInt` + 列属性标记（见 §2.2），不产生新 `DataType` 变体。
- **绑定校验**（DDL 执行前，`schema_executor.rs` 或新 `serial` 校验点）：
  - serial 列不可同时带 `DEFAULT`（报错：`SERIAL column cannot have DEFAULT`）；
  - 每 tag / edge type 至多一列 SERIAL（报错：`only one SERIAL column is allowed`）；
  - serial 列必须 `NOT NULL`（隐式设置，Nullable 恒为 false）。

### 2.2 类型表示：列属性而非类型

- `PropertyDef`（`crates/graphdb-core/src/core/types/property.rs`）新增字段：
  `pub serial: bool`（默认 false）。`PropertyDef` 是 schema 元数据，serde 自描述，
  旧文件反序列化缺省字段需 `#[serde(default)]`（开发期可接受格式变更，与
  类型系统整改 P0-A 同款决策）。
- 存储层无需感知 `serial`：列就是普通 `BigInt`（`element_size = 8`，定长，
  可索引——`supports_ordered_key` 已含 BigInt）。唯一改动点在**插入分配**。

### 2.3 序列状态：`SERIAL_NEXT`

- 内存：每空间一个 `SerialAllocator`（内部 `AtomicU64`），持有
  `(space_id, tag_name/edge_key) → next_value` 映射，放于
  `GraphStorageContext`（与 schema_manager 并列）。
- 持久化：`SERIAL_NEXT` 元数据（schema 元数据表新增一行/每序列，或复用
  `metadata_version` 式独立表）。写路径：
  - 分配后异步/批量落盘（借用现有 checkpoint 节奏，与 `table_tracker` 的
    flush 机制对齐）；
  - **WAL redo**：`InsertVertex` redo 已携带最终属性值（writer.rs:256
    `InsertVertexRedo { properties }`），恢复重放即重写整行——计数器不参与
    redo，恢复时按 §2.4 从持久化值/列最大值重建。
- 恢复规则：`next = max(持久化 SERIAL_NEXT, 该列当前 max + 1)`。
  - `max + 1` 覆盖"分配后未落盘即崩溃"的丢失窗口（id 不重用）；
  - `持久化值`覆盖"高水位已分配但行被删除"后列 max 回落的情况（id 不重用）。
  - 两者取大即为单调性保证，无需在 WAL 单独记录分配。

### 2.4 插入语义（`writer.rs` 的 `insert_vertex_at_timestamp` 前）

```
对每 tag 的 props：
  若 serial 列存在：
    若 props 含该列 → 用显式值；校验与当前列 max 无冲突（重复 → 错误）；
       并推进计数器到 max(计数器, 显式值)
    若 props 不含该列 → next = allocator.next(space, tag)；追加 (col, BigInt(next))
```

- 分配发生在事务写路径内、写 WAL 之前（与普通属性一致），undo 无特殊处理：
  **分配即消耗**，回滚不回收 id（文档化语义，避免"回滚后复用已暴露的 id"）。
- 显式值冲突校验：查询该列是否已有该值（走列 max 判断即可：`value <= current_max`
  且已被占用时冲突；实现上做一次存在性检查），冲突返回明确错误。

### 2.5 约束与联动面

| 面 | 行为 |
|----|------|
| cast | `can_cast` 无感知（列类型就是 BigInt，天然参与既有转换矩阵） |
| 索引 | `supports_ordered_key(BigInt) = true`，可建索引；**不**自动建唯一索引（本期文档化，用户按需 `CREATE TAG INDEX ... ON (id)`） |
| 唯一性 | 本期不隐式唯一；显式插入冲突检测仅对"已分配区间"负责 |
| DML | `UPDATE`/`DELETE` 不改计数器；`INSERT` 显式提供走 §2.4 |
| 导入 | `import`/`bulk` 路径复用同一写入入口，逐行分配（性能后续优化） |
| 序列化 | `DataType` 无新变体，`as_u8`/`from_u8`/`OrderedCodec`/`element_size` 零改动 |
| undo/redo | 分配值随属性值进 undo 旧值（`Value::BigInt`），回滚恢复原值；计数器不回退 |

### 2.6 与 VID 的关系（明确边界）

- 本期 SERIAL 是**属性列**，不是 VID 生成器：`INSERT VERTEX Person(id, name)
  VALUES ...` 中 id 是属性；VID 仍由应用提供。
- 后续（不在本期）：`CREATE SPACE s (vid_type=SERIAL)` 或
  `CREATE TAG ... (id SERIAL PRIMARY VID)` 式扩展，把分配器接到
  `VertexId::from_int64` 构造点——需另立文档，涉及 VertexId 生成、
  空间级唯一性与 fetch_add 路径。

## 3. 实施步骤与验证

| 步骤 | 内容 | 验证 |
|------|------|------|
| 1 | `PropertyDef.serial` 字段 + `#[serde(default)]`；DDL 解析 `SERIAL` 关键字 → BigInt + 标记；绑定校验（DEFAULT 冲突、每表一列、隐式 NOT NULL） | `cargo test -p graphdb-query`（DDL 解析/校验单测；`CREATE TAG ... (id SERIAL DEFAULT 1)` 报错用例） |
| 2 | `SerialAllocator`（AtomicU64，`(space, table) → next`）挂入 `GraphStorageContext`；插入缺省时分配并追加属性；显式值冲突检测 | `cargo test -p graphdb-storage --lib`（分配单调性、显式值推进、冲突报错单测） |
| 3 | `SERIAL_NEXT` 持久化（元数据行 + checkpoint 落盘）+ 启动恢复 `max(持久化, 列max+1)` | 重启恢复单测：分配后崩溃未落盘 → id 不重用；删行后重启 → id 不重用 |
| 4 | e2e：`CREATE TAG ... (id SERIAL)` + 两条 `INSERT VERTEX`（不带 id）→ 自动 1、2；显式插入 → 冲突报错；`FETCH PROP` 断言 id 值 | `cargo test --test integration_e2e` 新增用例 |
| 5 | 文档化语义（分配即消耗、不自动唯一索引） | 更新本计划状态；README 索引更新 |

## 4. 风险与回退

| 风险 | 缓解 |
|------|------|
| 计数器与 WAL 重放时序不一致导致 id 重复 | 恢复规则取 max 双源，重放天然幂等（redo 携带最终值）；分配只发生在写路径，不依赖 WAL 顺序 |
| 显式插入 + 自动分配混用产生冲突语义混乱 | 文档明确：显式值校验"已分配区间"冲突；用户混用需自行保证（与 PostgreSQL `OVERRIDING` 语义对齐留作后续） |
| `PropertyDef` 格式变更 | 开发期无向后兼容要求（同 P0-A 决策），`#[serde(default)]` 兜底 |
| 批量导入性能 | 本期逐行分配；性能不达标时改批量预取区间（`fetch_range(n)`） |
| 回退 | 单步可回滚；`serial` 字段为纯增量，删除后行为与现状一致 |

## 5. 不做（本期范围外）

- VID 自动生成（§2.6）。
- 隐式唯一索引 / 主键语义。
- 序列的 `CURRVAL/NEXTVAL` 函数与显式 sequence 对象（Ladybug 也无 SQL 级 sequence API，按需再加）。
- 跨事务预分配区间缓存（`fetch_range` 批分配）——先以单值分配验证正确性。
