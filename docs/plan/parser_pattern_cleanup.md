# Pattern 解析层类型与重复代码治理方案

## 背景与目标

`dml_parser/create.rs` 的 `unused_assignments` 告警暴露出 pattern 解析层的四类结构性问题。本方案在保持语法行为（AST 语义、报错文案、span 覆盖范围）完全不变的前提下收敛这些问题。项目处于开发期、无需向后兼容，因此直接按目标形态重构，不保留过渡层。

## 现状问题

### 1. CREATE 复用胖 `EdgePattern`，占位字段永远为空

`parser/ast/pattern.rs` 的 `EdgePattern` 是 traversal（MATCH）与 CREATE DML 共用的结构，含 9 个字段。traversal 侧会填 `range` / `path_semantic` / `recursive_comprehension` / `predicates`，而 CREATE 侧（`parse_edge_pattern` in `dml_parser/create.rs`）永远写 `None` / `Vec::new()`。类型系统不表达"CREATE 模式不支持变长路径"，下游只能靠运行时约定防御。

消费方事实（决定拆分边界）：
- `binder/bind/bind_dml.rs::bind_pattern_edge` 只读 `variable / edge_types / properties / direction`；
- `binder/bind/bind_ddl.rs` 对 `CreateTarget::Path` 直接拒绝（无执行路径），因此 CREATE 侧的 pattern 只在解析与绑定报错之间流转；
- `render_edge` / `PatternUtils` / `traversal_start` 仅服务于 traversal 语义。

### 2. 方向推导逻辑逐字重复且脆弱

`traversal_parser.rs`（396-418 行）与 `dml_parser/create.rs` 各有一段相同的 if/else：先看 `<-`（BackArrow/LeftArrow）置 `In`，结尾再按 `->` / `-` / `<-` 互相覆盖成 `Out` / `Both`。方向语义分散在首尾两处 token 判断里，组合矩阵不可见；`Arrow` 与 `RightArrow` 两个分支逻辑重复。

### 3. qualified reference 报错分支重复 5 处

`identifier` 之后若跟 `Dot + identifier` 则报 `qualified_reference_message` —— 在 `create.rs`（2 处）、`traversal_parser.rs`（3 处）逐字重复。

### 4. span 追踪三行惯用法遍布全部 parser

```rust
let end_span = ctx.current_span();
let span = ctx.merge_span(start_span.start, end_span.end);
```

在 `parser/parsing/` 下出现约 60 处，是手工样板，容易在重构时漏更新或抄错起点。

## 方案

### A. 拆分 CREATE 专用边结构（问题 1）

在 `parser/ast/pattern.rs` 新增：

```rust
/// Edge hop in a CREATE data pattern. Variable-length traversal
/// constructs (`*`, path semantics, recursive comprehension) are
/// MATCH-only and cannot appear here.
pub struct CreateEdgePattern {
    pub span: Span,
    pub variable: Option<String>,
    pub edge_types: Vec<String>,
    pub properties: Option<ContextualExpression>,
    pub direction: EdgeDirection,
}
```

`PathElement` 增加 `CreateEdge(CreateEdgePattern)` 变体；`PathElement::Node` 继续共用（CREATE 节点与 MATCH 节点字段一致，无空占位问题）。

各消费方处理：
- `bind_dml.rs::bind_pattern_edge` 改收 `CreateEdgePattern`（或按变体分发）；
- `PatternUtils::find_variables_in_element` / `get_labels_in_element`：对 `CreateEdge` 提取 variable / edge_types（语义与 Edge 相同）；
- `render_edge`：为 `CreateEdge` 提供等价的文本回放（无 range/semantic 分支，输出更短）；
- `bind_match.rs` / `traversal_start.rs`：只遍历 traversal 上下文，`CreateEdge` 走不到，匹配臂直接按 traversal 不可达处理（返回 none / 跳过）。

`EdgePattern` 保留给 traversal，`predicates` / `range` / `path_semantic` / `recursive_comprehension` 成为 MATCH 专属能力，"CREATE 不支持变长"从运行时约定变为类型事实。

### B. 方向推导改为首尾 token 映射（问题 2）

新增 `parser/parsing/pattern_common.rs`，承载两个 parser 共享的 pattern 辅助逻辑：

```rust
/// Direction from the hop's boundary tokens: `<-` on the left, and
/// the closing token on the right.
pub fn resolve_edge_direction(incoming: bool, closing: &TokenKind) -> Option<EdgeDirection>
```

映射表（一目了然的组合空间）：

| 左侧 `<-` | 右侧收尾 | 方向 |
|---|---|---|
| 否 | `->`（Arrow / RightArrow） | Out |
| 否 | `-` | Both |
| 是 | `<-`（Minus） | In |
| 是 | `->` / `<-` 混用 | Both |
| 其他 | — | None（报语法错误） |

两个 parser 改为：记录左侧是否出现 `<-`，消耗完边体后取收尾 token 一次，交给该函数。

### C. 提取 qualified reference 报错（问题 3）

同一模块提供：

```rust
/// Reject `alias.table` in label/edge-type position with the
/// attached-database hint message.
pub fn reject_qualified_reference(ctx: &mut ParseContext, name: &str) -> Result<(), ParseError>
```

内部检查 `Dot`、读表名、用 `current_position()` 构造 `ParseError`。替换 create.rs（2 处）与 traversal_parser.rs（3 处）的重复块；`bind_match.rs` 里的两处是对已绑定 label 的 `split_once('.')` 检查，形态不同，不在本次范围。

### D. `ParseContext::span_since` 统一 span 追踪（问题 4）

`parse_context.rs` 增加：

```rust
/// Span from `start` (usually another span's `.start`) up to the
/// current cursor position. Replaces the three-line
/// `end_span` / `merge_span` idiom.
pub fn span_since(&self, start: Position) -> Span
```

用正则将 `parser/parsing/` 下形如

```rust
let end_span = ctx.current_span();
let span = ctx.merge_span(X.start, end_span.end);
```

的两行统一替换为 `let span = ctx.span_since(X.start);`。`pipe.rs` 等以另一条语句的 span 为终点（`end_span = right.span()`）的写法不属于该惯用法，保持原样。

## 实施顺序

1. C + B（新建 `pattern_common.rs`，替换 5 处报错块、2 处方向推导）；
2. A（新增 `CreateEdgePattern` 与 `PathElement::CreateEdge`，改 create.rs 解析与各消费方匹配臂）；
3. D（`span_since` + 全 parser 正则替换）；
4. `cargo check -p linkrs-query`、`cargo test -p linkrs-query` 验证 AST 语义与报错文案不变。

## 风险

- B 的方向映射需覆盖 `LeftArrow` 与 `BackArrow` 两种 `<-` 拼写、`Arrow` 与 `RightArrow` 两种 `->` 拼写；替换后逐分支核对映射表。
- D 的正则替换可能命中边界写法（多行、嵌套 span），以编译错误兜底逐个核对。
- A 改动 `PathElement` 枚举，凡 `match` 全枚举处编译器会强制补臂，无漏网风险。
