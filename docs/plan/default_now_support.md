# DEFAULT now() 和 INSERT now() 支持分析

## 背景

项目之前不支持 `DEFAULT now()` 语法。DDL 解析器遇到 `now()` 会报错，INSERT 值中的 `now()` 会被求值为 Null。

## 已完成

### 1. DDL 解析器支持 `DEFAULT now()`

**文件**: `crates/graphdb-query/src/query/parser/parsing/ddl_parser.rs`

修改了 `parse_value_literal` 方法，当 `DEFAULT` 后面跟着 `identifier(` 时，调用新增的 `parse_and_eval_function_call` 方法：

```rust
// 检测函数调用模式：identifier + '('
if matches!(token_kind, TokenKind::Identifier(_)) && ctx.peek_token().kind == TokenKind::LParen {
    return self.parse_and_eval_function_call(ctx);
}
```

`parse_and_eval_function_call` 使用 `ExprParser` 解析函数调用表达式，然后使用 `ExpressionEvaluator` + `DefaultExpressionContext` 立即求值。DDL 阶段求值是可接受的，因为：
1. DDL 不是热路径
2. `now()` 在 DDL 时求值意味着"schema 创建时间"，语义上合理

### 2. Geography 类型存储修复

**文件**: `crates/graphdb-storage/src/storage/vertex/column_store.rs`

修复了 `write_variable_value` 和 `VariableWidthColumn::get` 不支持 `Value::Geography` 的问题：

- `VariableWidthColumn` 新增 `data_type` 字段
- `write_variable_value` 对 `Geography` 使用 JSON 序列化
- `VariableWidthColumn::get` 对 `Geography` 使用 JSON 反序列化

## 待实现：INSERT now() 支持

### 问题分析

INSERT 值中的 `now()` 经过以下阶段：

| 阶段 | 文件 | 当前行为 |
|------|------|----------|
| 解析 | `dml_parser.rs:609` | ✅ 正确解析为 `Expression::Function` |
| 验证 | `insert_vertices_validator.rs:512` | ❌ 返回 `Value::Null` |
| 规划 | `insert_planner.rs:72` | ✅ 保留表达式 |
| 构建 | `data_modification_builder.rs:74` | ❌ `evaluate_literal` 返回 None → Null |
| 执行 | `insert.rs:170` | ❌ 只写预计算的 Value |

### UPDATE 的正确模式（参考）

UPDATE 执行器正确处理了非字面量表达式：

1. **构建阶段** (`data_modification_builder.rs:462-498`)：
   ```rust
   if let Some(value) = Self::evaluate_literal(&expr) {
       properties.insert(key.clone(), value);
   } else {
       property_expressions.insert(key.clone(), value_expr.clone());
       has_non_literal_expr = true;
   }
   ```

2. **执行阶段** (`update.rs:340-349`)：
   ```rust
   let mut context = DefaultExpressionContext::new();
   ExpressionEvaluator::evaluate(expr, &mut context)
   ```

### 实现方案

采用与 UPDATE 相同的模式：

#### 方案 A：修改构建器 + 执行器（推荐）

**Step 1**: 修改 `InsertVerticesNode` 或 `VertexInsertInfo` 以保留非字面量表达式

在 `insert_nodes.rs` 中，`values` 字段目前是 `Vec<(ContextualExpression, Vec<Vec<ContextualExpression>>)>`。
需要新增一个字段来存储非字面量表达式，或者修改现有结构。

**Step 2**: 修改 `data_modification_builder.rs` 的 `build_insert_vertices`

```rust
fn evaluate_property_value(expr: &Expression) -> (Option<Value>, Option<ContextualExpression>) {
    match expr {
        Expression::Literal(value) => (Some(value.clone()), None),
        _ => {
            // 尝试求值（支持 now() 等无参数函数）
            let mut eval_ctx = DefaultExpressionContext::new();
            match ExpressionEvaluator::evaluate(expr, &mut eval_ctx) {
                Ok(value) => (Some(value), None),
                Err(_) => (None, Some(expr.clone())),
            }
        }
    }
}
```

**Step 3**: 修改 `InsertExecutor` 在执行阶段求值

#### 方案 B：在构建器中直接求值（更简单）

直接在 `build_insert_vertices` 中使用 `ExpressionEvaluator` 对所有表达式求值：

```rust
let value = prop_value
    .get_expression()
    .and_then(|e| {
        if let Expression::Literal(value) = e {
            Some(value.clone())
        } else {
            let mut eval_ctx = DefaultExpressionContext::new();
            ExpressionEvaluator::evaluate(e, &mut eval_ctx).ok()
        }
    })
    .unwrap_or(Value::Null(crate::core::NullType::Null));
```

**优点**：
- 最小改动
- 与 DDL `DEFAULT now()` 实现一致
- `DefaultExpressionContext` 已经支持函数注册

**缺点**：
- 构建器阶段需要表达式求值上下文
- 如果表达式包含变量引用，求值会失败（但 INSERT 值通常不含变量）

### 推荐方案

**方案 B** 更简单且足够。原因：
1. INSERT 值中的函数调用（如 `now()`）不依赖运行时上下文
2. `DefaultExpressionContext` 已提供函数注册支持
3. 与 DDL `DEFAULT now()` 的实现一致
4. 如果求值失败，回退到 `Value::Null` 是合理的

### 需要修改的文件

1. `crates/graphdb-query/src/query/executor/factory/builders/data_modification_builder.rs`
   - 修改 `evaluate_literal` 或 `build_insert_vertices` 中的求值逻辑

2. `tests/e2e/data/social_network_data.gql`（可选）
   - 添加 `DEFAULT now()` 测试数据

### 测试计划

1. 添加 GQL 测试数据：`created_at: TIMESTAMP DEFAULT now()`
2. 验证 INSERT 时 `created_at` 自动填充
3. 验证 `RETURN now()` 返回当前时间戳
4. 运行完整 e2e 测试套件确认无退化
