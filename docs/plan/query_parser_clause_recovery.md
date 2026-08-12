# 解析器 Clause 级错误恢复接线方案

## 1. 现状分析

`parser/parsing/parse_context.rs` 定义了三级恢复范围：

```rust
pub enum RecoveryScope { Statement, Clause, Expression }
```

- `Statement` / `Expression` 两级已被 `parser.rs` 的 `parse_statement` 与
  `parse_expression_contextual` 调用
- `RecoveryScope::Clause` 与 `CLAUSE_SYNC_TOKENS` 定义完整，但**全库无调用点**
  —— clause 级同步是预留未接线状态

`ParseContext` 已具备：`synchronize(scope)`（跳到同步 token）、
`try_recover(error, scope)`（记录错误 + 计数 + 同步）、`RECOVERY_LIMIT = 5`、
`recovery_count` 防失控。

## 2. 方案设计

### 2.1 定义 clause 级同步 token

`CLAUSE_SYNC_TOKENS` 补充图查询子句关键 token（现状需核查并补全）：

```rust
const CLAUSE_SYNC_TOKENS: &[TokenKind] = &[
    TokenKind::Match, TokenKind::Where, TokenKind::Return,
    TokenKind::Yield, TokenKind::Order, TokenKind::Limit,
    TokenKind::Skip, TokenKind::With, TokenKind::Go,
    TokenKind::Find, TokenKind::Lookup, TokenKind::Insert,
    TokenKind::Update, TokenKind::Delete, TokenKind::With,
    TokenKind::Union, TokenKind::And, TokenKind::Or, // 布尔短路恢复点
];
```

### 2.2 子句解析入口接线

各子句解析函数（`clause_parser.rs` / `traversal_parser.rs`）在解析失败时：

```rust
// 示例：WHERE 子句解析失败后同步到下一个子句关键字
if let Err(e) = ctx.try_recover(RecoveryScope::Clause) {
    return Err(e); // 恢复次数超限
}
// 同步成功后继续解析后续子句
```

接入点选择原则：**只在子句边界包一层**（如 `parse_where`、`parse_order_by`、
`parse_match_path` 的入口），不在表达式内部使用 Clause 范围（表达式内部用
`Expression` 范围），避免恢复语义混乱。

### 2.3 错误信息增强

- `ParseErrors` 中为每条错误附带：错误位置（span）、当前 token、**预期 token
  候选**（parser 在该位置已尝试的 token 集合，若已记录）
- 同步恢复后产生的错误标记 `recovered: true`，供上层（API 层）决定是返回
  部分结果还是仅展示诊断信息

### 2.4 测试用例

`parsing/tests.rs` 补充：

- 单子句错误 → 同步到下一子句并成功解析（如 `MATCH (v) WHERE v.age > 30 RETURN v`
  中 WHERE 写错关键字）
- 多子句错误 → 全部记录且 `recovery_count <= RECOVERY_LIMIT`
- 恢复超限 → 返回错误（沿用 `test_error_recovery_stops_at_limit` 模式）

## 3. 实施步骤

| 步骤 | 内容 | 涉及文件 |
|------|------|----------|
| 1 | 核查并补全 `CLAUSE_SYNC_TOKENS` | `parsing/parse_context.rs` |
| 2 | 子句解析入口包 `try_recover(Clause)` | `parsing/clause_parser.rs`, `parsing/traversal_parser.rs` 等 |
| 3 | 错误信息增强（预期 token 候选） | `parsing/parser.rs`, `parsing/errors.rs` |
| 4 | 测试用例 | `parsing/tests.rs` |

## 4. 验证方法

- `cargo test -p graphdb-query --lib parsing` 全量通过
- 手工验证：构造带 2-3 处子句级错误的查询，确认诊断信息完整且数量受限于
  `RECOVERY_LIMIT`
- 回归：确认正常语句解析路径零行为变化（恢复路径仅在出错时触发）

## 5. 预期收益

- 单条语句报告多个子句错误，一次解析反馈多个问题（贴近 IDE 体验，
  对应文档 4.1.3 建议）
- 错误位置 + 预期 token 提升诊断质量

## 6. 风险与回退

- **风险**：同步 token 过宽导致误恢复（把合法 token 当错误跳走）。缓解：
  CLAUSE 同步只跳**子句边界关键字**；恢复路径与正常路径完全隔离
- **回退**：不接线任何新调用点即回退现状（`Clause` 保持预留状态），
  改动点集中在 `try_recover` 调用与常量表，无破坏性
