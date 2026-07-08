# DEFAULT 表达式支持方案

## 问题描述

`CREATE TAG` 的字段定义中，`DEFAULT now()` 语法不被当前解析器支持，报错：

```
Parse error: Unsupported default value type: Identifier("now")
```

## 现状

### 支持的 DEFAULT 值类型

当前 `parse_value_literal()` 仅支持 6 种字面量：

| 类型 | 示例 | Value 变体 |
|------|------|------------|
| 字符串 | `DEFAULT "hello"` | `Value::String` |
| 整数 | `DEFAULT 42` | `Value::BigInt` |
| 浮点 | `DEFAULT 3.14` | `Value::Double` |
| 布尔 | `DEFAULT true` | `Value::Bool` |
| NULL | `DEFAULT NULL` | `Value::Null` |
| 负数 | `DEFAULT -42` | `Value::BigInt`/`Double` |

### 现有基础设施

- `Expression` 枚举已有 `Function { name, args }` 变体
- `now()` 函数已在函数注册表中注册（`DateTimeFunction::Now`，返回 `Value::BigInt` 毫秒时间戳）
- 表达式求值器（`ExpressionEvaluator`）可以递归求值函数调用
- INSERT VERTEX 在 `data_modification_builder.rs:80-93` 中通过 `tag_props` 填充默认值

### 架构局限

`PropertyDef.default` 字段类型为 `Option<Value>`，只存字面量，无法表达函数调用。

```
PropertyDef {
    name: String,
    data_type: DataType,
    nullable: bool,
    default: Option<Value>,       // ← 只能存字面量
    comment: Option<String>,
}
```

## 方案

### 核心改动

将 `PropertyDef.default` 从 `Option<Value>` 改为 `Option<DefaultValue>`：

```rust
enum DefaultValue {
    /// 字面量默认值（已有功能）
    Literal(Value),
    /// 函数调用默认值（新增）：DEFAULT now(), DEFAULT uuid() 等
    Function { name: String, args: Vec<Expression> },
}
```

### 影响范围（按模块）

#### 1. Parser — `crates/graphdb-query/src/query/parser/parsing/ddl_parser.rs`

- `parse_value_literal()` 重命名为 `parse_default_value()`，新增 `Identifier("now")` → `DefaultValue::Function` 分支
- 当前 `_ => Err(...)` 改为尝试解析函数调用：匹配 `Identifier(name) + LParen + [args] + RParen`
- 新增 `parse_function_call()` 处理 `now()` 等零参函数

#### 2. Core types — `crates/graphdb-core/src/core/types/property.rs`

- 新增 `DefaultValue` 枚举
- `PropertyDef.default` 类型变更
- `DataType` 可能需要新增方法来判断某个 `DefaultValue` 是否类型兼容

#### 3. Storage — `crates/graphdb-storage/`

- `StoragePropertyDef.default_value` 类型同步变更
- 序列化/反序列化（Bincode/Protobuf）新增 `DefaultValue` 变体
- 向后兼容：读取旧数据时，`Function` 变体不存在，需要 fallback

#### 4. Query Metadata — `crates/graphdb-query/src/query/metadata/types.rs`

- `PropertyDefinition.default_value` 类型同步变更

#### 5. INSERT VERTEX 默认值求值 — `data_modification_builder.rs`

- `fill_default_values()` 遇到 `DefaultValue::Function { name: "now", .. }` 时调用 `ExpressionEvaluator` 求值
- 需要 `ExecutionContext` 或 `FunctionRegistry` 引用传递到 builder

#### 6. SHOW CREATE TAG — `show_create_tag.rs`

- `Display` 实现新增 `DefaultValue` 渲染：
  - `Literal(v)` → `DEFAULT <v>`
  - `Function { name, args }` → `DEFAULT <name>(<args>)`

#### 7. DESC TAG — `desc_tag.rs`

- 当前 "Default" 列硬编码为空字符串，改为显示 `DefaultValue`

#### 8. SchemaManager — `crates/graphdb-core/src/core/metadata/schema_manager.rs`

- `TagData` 内部存储 `PropertyDef`，类型变更后自动同步

### 涉及文件清单

| 文件 | 改动类型 |
|------|----------|
| `crates/graphdb-core/src/core/types/property.rs` | 新增 `DefaultValue` enum，改 `default` 类型 |
| `crates/graphdb-core/src/core/types/property_trait.rs` | `default_value()` 返回类型变更 |
| `crates/graphdb-core/src/core/types/tag.rs` | 无改动（`TagInfo` 引用 `PropertyDef`） |
| `crates/graphdb-query/src/query/parser/parsing/ddl_parser.rs` | 新增 `parse_default_value()`，支持函数调用 |
| `crates/graphdb-query/src/query/parser/ast/stmt.rs` | 无改动（`CreateTarget::Tag` 引用 `PropertyDef`） |
| `crates/graphdb-query/src/query/planning/plan/core/nodes/management/tag_nodes.rs` | `TagManageInfo.properties` 类型自动同步 |
| `crates/graphdb-query/src/query/executor/factory/builders/admin_builder.rs` | 无改动（透传 `PropertyDef`） |
| `crates/graphdb-query/src/query/executor/factory/builders/data_modification_builder.rs` | 新增 `DefaultValue::Function` 求值逻辑 |
| `crates/graphdb-query/src/query/executor/admin/tag/create_tag.rs` | 无改动（透传 `PropertyDef`） |
| `crates/graphdb-query/src/query/executor/admin/tag/show_create_tag.rs` | `DefaultValue` Display 渲染 |
| `crates/graphdb-query/src/query/executor/admin/tag/desc_tag.rs` | 显示默认值而非空字符串 |
| `crates/graphdb-query/src/query/validator/helpers/schema_validator.rs` | `get_default_value()` 返回类型变更 |
| `crates/graphdb-query/src/query/metadata/types.rs` | `PropertyDefinition.default_value` 类型变更 |
| `crates/graphdb-storage/src/storage/types.rs` | `StoragePropertyDef.default_value` 类型变更 |
| `crates/graphdb-storage/src/storage/client.rs` | 如果有序列化逻辑，同步变更 |

## 优先级与建议

**不推荐现在实现。** 理由：

1. **无实际需求**：当前所有 `.gql` 和数据集的 INSERT 都显式提供了 `created_at` 值，没有依赖 `DEFAULT now()`
2. **改动面大**：跨越 8+ 个文件、3 个 crate，涉及序列化兼容，风险与收益不成正比
3. **合理的替代方案**：在应用层由客户端插入时提供时间戳，或后续用触发器（trigger）机制实现

**建议**：保留 `DEFAULT now()` 作为未来功能（v0.2+），当前从测试 GQL 文件中移除该语法即可。
