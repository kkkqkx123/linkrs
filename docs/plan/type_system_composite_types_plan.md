# STRUCT/ARRAY 复合类型与类型元数据机制设计方案

> 状态：设计方案（2026-08-15）。
>
> 前置：`docs/plan/type_system_integrity_plan.md` §2.10（P2-J，长期）。
> 本文档把"ExtraTypeInfo 式元数据机制 + STRUCT/ARRAY + Map 键泛化"落为可实施
> 的分阶段设计。依赖已完成的类型系统整改成果：`from_u8` 严格化（Result）、
> 码位 64+ 预留（types.rs）、`Value` 全变体 serde 往返测试门禁
> （`serialization_roundtrip_test.rs` 的 `matches!` 穷尽检查）。

## 0. 结论摘要

| 项 | 决策 |
|----|------|
| 元数据载体 | 新增 `TypeInfo` 枚举（`Struct`/`Array`/`Decimal{precision,scale}`），`DataType` 新增 `Struct(Arc<StructTypeInfo>)` / `Array(Arc<ArrayTypeInfo>)` 两变体，Arc 共享避免深拷贝 |
| 类型码 | `as_u8` 分配裸码 64（Struct）/65（Array）；`from_u8` 新增 `ParameterizedTypeCode` 错误；schema 持久化格式升级为 `code + postcard(TypeInfo)`，旧格式（纯 code ≤31）兼容 |
| Value 端 | 新增 `Value::Struct(Box<StructValue>)`（保序字段）、`Value::Array(Box<ArrayValue>)`；Hash/Eq/比较/Display/内存估计/穷尽测试全链路随编译器驱动更新 |
| 存储 | 变长列（VariableWidthColumn）+ postcard 序列化整值；**必须**加入 `is_variable_length_type` 白名单（避免 0 字节定长列陷阱）；子列列式布局列为后续优化 |
| 索引 | `supports_ordered_key` 拒绝 Struct/Array（白名单不加）；Array 全标量有序编码列为后续 |
| 类型提升 | `get_common_type` 支持同构 Struct 字段联合（combineTypes 式）与 Array 元素公共类型；`can_cast` 支持 Struct↔Map 同构、Array↔List 同元素互转 |
| 表达式 | 新增 `Expression::StructField`（`addr.city` 语法）；`Subscript` 扩展支持 Struct/Array 下标 |
| Map 键泛化 | 独立阶段 M4：`Value::Map(Box<HashMap<Value, Value>>)`（`Value` 已实现 `Hash`/`Eq`，value_compare.rs:79/189） |

## 1. 背景与动机

- 属性图建模中"对象型属性"（地址 `{city, street}`、配置项、JSON 结构）与定长数组
  是高频需求，linkrs 目前仅 `List`/`Map`（键限 String）/`Set`，无 STRUCT、无 ARRAY
  （Ladybug 有 `STRUCT`/`LIST`/`ARRAY`/`MAP`/`UNION` 一等公民，可任意嵌套）。
- 根本障碍是**扁平 `DataType` 枚举无法承载嵌套子类型**：类型自身不带 schema，
  嵌套信息只能散落在 `Value` 内部，DDL/绑定/存储/索引四链路无法对齐
  （Ladybug 的 `LogicalType + ExtraTypeInfo` 指针方案正是为此设计，`types.h:290`）。
- 类型系统整改（P0/P1）已完成的前置：类型码 64+ 预留区间、`from_u8` 显式报错、
  serde 单轨序列化、全变体往返测试门禁——本方案在其上叠加元数据载体，不再
  触碰 0-31 既有码位。

## 2. 目标设计

### 2.1 元数据载体：`TypeInfo`

```rust
// crates/graphdb-core/src/core/types.rs（或新模块 core/types/type_info.rs）

/// 类型级元数据（对齐 Ladybug ExtraTypeInfo）。
/// Arc 共享：schema 内同一类型多处引用不深拷贝。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TypeInfo {
    /// STRUCT：命名字段复合类型（字段保序）。
    Struct(StructTypeInfo),
    /// ARRAY：定长数组（len = Some）或变长（len = None，等价 LIST 约束）。
    Array(ArrayTypeInfo),
    /// DECIMAL128 的精度/标度（顺带补齐，见 §5）。
    Decimal { precision: u8, scale: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StructTypeInfo {
    pub fields: Vec<(String, DataType)>, // 保序；嵌套递归经 Arc 打破
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArrayTypeInfo {
    pub element: Box<DataType>,
    pub len: Option<usize>, // Some(n) = 定长 ARRAY(n)
}
```

`DataType` 扩展（其余 30 变体不动）：

```rust
DataType::Struct(Arc<StructTypeInfo>),
DataType::Array(Arc<ArrayTypeInfo>),
```

- **`DataType` 已 derive `Hash/Eq`**（types.rs:49），`Arc<T>` 在 `T: Hash/Eq` 时
  内容比较/哈希，语义正确；递归嵌套经 `Arc`/`Box` 打破。
- **serde 对 `Arc` 需要 `rc` feature**：`graphdb-core` 的 `serde` 依赖需开启
  `features = ["rc"]`（实现要点 1）。
- `ValueType`（semantic.rs，`from_data_type`/`to_data_type`）需同步支持两新变体。

### 2.2 类型码与 schema 持久化

- `as_u8`：`Struct => 64`、`Array => 65`（64-95 数值/结构预留区间内），**裸码不带
  参数**（与 `FixedString(21)`/`VectorDense(26)` 折叠行为一致）。
- `from_u8`：64/65 返回新错误变体
  `TypeCodecError::ParameterizedTypeCode(u8)`（区别于 `UnknownTypeCode`，区分
  "已知但需参数"与"未知码"；保持 22/24 `Reserved` 语义不变）。
- **schema 属性序列化格式升级**（`property_table.rs:1198/1332` 现状为单字节 code）：
  ```
  [code: u8] [postcard::to_allocvec(&TypeInfo)? ...]
  ```
  - code ≤ 31：无参数块，与旧格式字节级兼容（旧文件直接可读）；
  - code ≥ 64：紧跟参数块（`postcard` 自描述，长度内嵌）；
  - 反序列化：`DataType::from_u8(code)` 后按 code 分支读参数块，
    `ParameterizedTypeCode` 即"需要参数"信号。
- 校验（DDL 创建/alter 时）：`supports_ordered_key` 之外的合法性由绑定层校验，
  schema 落盘前断言 `code + TypeInfo` 一致（防双写漂移，复用 P1-D 单一真相源思路）。

### 2.3 Value 变体

```rust
/// STRUCT 值：保序字段表。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StructValue { pub fields: Vec<(String, Value)> }

/// ARRAY 值：定长数组。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ArrayValue { pub values: Vec<Value> }

Value::Struct(Box<StructValue>),
Value::Array(Box<ArrayValue>),
```

- **编译器驱动全链路更新**（新增变体的固定检查单，参照 P1-F 的穷尽化先例）：
  1. `Value` 手写 `PartialEq/Eq/Hash`（value_compare.rs:12/79/189）——新增分支；
  2. `get_type`（value_def.rs:96）→ `DataType::Struct`/`Array`；
  3. `Display`（value_def.rs Display impl）；
  4. 内存估计（memory.rs）；
  5. `is_null`/`is_empty` 判定；
  6. `serialization_roundtrip_test.rs` 的 `matches!` 穷尽检查 + 样本构造（**门禁**，
     不改即编译失败）；
  7. `type_deduce.rs::deduce_value_type` 穷尽匹配（P1-F 已去兜底，编译器强制）；
  8. `value_convert.rs`（cast 到 Struct/Array 的分派分支）；
  9. `value_arithmetic.rs`（无算术，落入既有非数值分支）。

### 2.4 存储：变长列

- `is_variable_length_type`（column_store.rs:73）**必须**加入
  `Struct`/`Array`——否则落入 `FixedWidthColumn` 且 `element_size = 0`，
  重演 VID 的"0 字节定长列越界损坏"陷阱（类型系统整改 P0-B 刚清除的教训）；
- 值编码：复用 `VariableWidthColumn` 的 postcard 序列化整值路径（与 List/Map
  现状一致）；
- 子列列式布局（Ladybug 递归拆子列 + overflow 文件）列为后续优化（§5），
  不阻塞类型落地；
- undo/redo：`Value` 走 serde 单轨（P1-C 成果），新变体自动无损。

### 2.5 索引与 OrderedCodec

- `supports_ordered_key`（ordered_codec.rs:82）**不加** Struct/Array——DDL 建索引
  校验与编码路径行为一致（P1-D 单一真相源自动生效，无分叉）；
- 后续可选：`Array<标量>` 全元素有序编码（元素定长时字节级拼接可排序），
  单独评估后放开。

### 2.6 类型转换与提升（type_system.rs）

- `can_cast` 增补：
  - `Struct ↔ Map`（同构：字段名集合一致；Map 值类型为字段类型的公共超型）；
  - `Array ↔ List`（元素类型一致或 `can_cast` 可转换）；
  - `Struct/Array → String`（可读序列化，`to_string` 风格）。
- `get_common_type` 增补：
  - 同构 Struct：字段**并集**联合（对齐 Ladybug `combineTypes`），字段类型取
    公共超型（递归 `get_common_type`）；
  - Array/List：元素取公共超型；
  - 异构 → `Empty`（保持现状语义）。

### 2.7 表达式与 binder

- 新增 `Expression::StructField { base: Box<Expression>, field: String }`：
  - 语法 `addr.city`（点号字段访问）；无 schema 上下文时 `deduce_type` 返回
    `Empty`（与 `Property` 的 binder 补全语义一致，type_deduce.rs 既有先例）；
  - binder：schema 解析后绑定字段类型，**绑定处加调试断言**防止 `Empty` 静默
    流向下游（P1-F §2.6 的标注要求）。
- `Subscript` 扩展：`arr[0]`（Array）、`addr['city']`（Struct）——接入既有
  `deduce_subscript_type` 与新执行路径。
- 字面量：`STRUCT{name: 1, addr: STRUCT{city: 'x'}}`（保留关键字引导，与既有
  Map 字面量 `{k: v}` 区分）；`ARRAY[1,2,3]` 与 `LIST[1,2,3]` 共存语法。

### 2.8 DDL 语法

```
CREATE TAG Person (
    id INT,
    addr STRUCT<city STRING, street STRING, geo STRUCT<lat DOUBLE, lon DOUBLE>>,
    coords ARRAY<DOUBLE>(3)          -- 定长 3
);
```

- 解析（`helpers.rs` 类型解析）：`STRUCT<...>` / `ARRAY<T>` / `ARRAY<T>(N)` 递归
  解析，嵌套深度设上限（如 16，防栈溢出，`parse_vid_type_value` 同风格）；
- 存储：`PropertyDef.data_type = DataType::Struct/Array`（TypeInfo 随 schema serde
  落盘，§2.2 格式）。

### 2.9 Map 键泛化（阶段 M4）

- `Value::Map(Box<HashMap<String, Value>>)` → `Box<HashMap<Value, Value>>`：
  - 前置已满足：`Value: Hash + Eq`（value_compare.rs:79/189 手写实现，含
    NaN/±0 归一化与集合有序哈希）；
  - 联动面：`Display`、Map 字面量解析（键类型放宽）、`value_compare`（Map 分支
    排序键比较——键从 String 排序改为 `Value` 排序）、`get_type`、`memory.rs`、
    OrderedCodec（Map 已被拒，无影响）、JSON 转换（Json↔Map 映射需处理非字符串
    键）、e2e 回归；
  - 与 STRUCT 同属"类型系统元数据化"工程，列为本方案收尾阶段。

## 3. 实施步骤与验证

| 步骤 | 内容 | 验证 |
|------|------|------|
| M1 | `TypeInfo` + `DataType::Struct/Array` + `Arc` serde `rc` feature + 码位 64/65 + `ParameterizedTypeCode` 错误 + schema 持久化格式（code+参数，旧格式兼容） | `cargo test -p graphdb-core --lib`：`as_u8/from_u8` 新码单测、`from_u8(64)` 报错用例、schema 序列化旧/新格式往返（property_table 单测） |
| M2 | `Value::Struct/Array` + 检查单 9 项（§2.3）+ 存储变长列 + `is_variable_length_type` 白名单 | 编译器驱动全量更新；`serialization_roundtrip_test.rs` 新增变体样本；列写入/读取/undo e2e（非标量回滚测试扩展） |
| M3 | DDL 语法/嵌套限制 + 字面量 + `StructField`/`Subscript` + `can_cast`/`get_common_type` 联合 + binder 绑定 | DDL 解析与嵌套深度用例；表达式 e2e（`addr.city`、`arr[0]`、`STRUCT{...}` 字面量、Struct↔Map cast）；type_deduce/type_system 单测每对规则一例 |
| M4 | Map 键泛化 `HashMap<Value, Value>` | 字面量/比较/JSON 转换单测；e2e 回归 |
| 收尾 | 全量回归 | `cargo test --workspace --lib`、`cargo test --test integration_e2e`、`cargo clippy --workspace --all-targets` |

## 4. 风险与回退

| 风险 | 缓解 |
|------|------|
| `DataType` 新增变体波及 `Hash/Eq` 与穷尽匹配面 | 编译器驱动（去兜底先例已立）；每阶段独立提交，M1 与 M2 可独立回滚 |
| serde `rc` feature 引入依赖变更 | 仅 `graphdb-core` 一个 crate 开启；`Arc` 序列化格式为标准行为，无自研格式 |
| schema 持久化格式升级兼容性 | code≤31 旧格式字节级兼容；`ParameterizedTypeCode` 显式报错不静默降级 |
| 嵌套深度/递归导致栈溢出 | DDL 深度上限 + `deduce`/`get_common_type` 递归在深度上限内 |
| 存储性能（整值序列化 vs 子列） | 本期正确性优先；子列列式（Ladybug 方案）独立立项，不阻塞 |
| `can_cast`/`get_common_type` 联合规则漂移 | 每对规则配单测（P1-E 模式）；Array 与 List 收敛到同一提升函数避免双写 |
| 回退 | M1-M4 各阶段独立；未实施 M4 时 Map 行为与现状一致 |

## 5. 后续（不在本期）

- **子列列式布局**：STRUCT 字段/ARRAY 元素递归拆子列（对齐 Ladybug
  `child column` 方案），列式过滤与向量化受益——需与存储层列式重构联动；
- **Decimal128 precision/scale**：`TypeInfo::Decimal` 已预留载体，落地时
  同步 `decimal128.rs` 的 scale 运算规则与 `can_cast` 数值转换细则；
- **UNION 可区分联合**：需新 `TypeInfo::Union` 变体 + `Value` 变体 + 存储
  变长分支，依赖 M2 的链路成熟后评估；
- **Array 有序索引**（§2.5）。
