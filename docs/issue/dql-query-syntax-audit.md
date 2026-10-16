# DQL 查询语法一致性与语义缺口审查

对应发布文档：`docs/release/01_dql_query_syntax.md`。审查对象是解析、绑定、规划、执行四层代码；
语义裁决以发布文档为契约——每条不一致最终二选一：补齐实现，或同步从文档与语法中移除。

## 审查范围与方法

 examined:

- `crates/linkrs-query/src/parser/parsing/traversal_parser/`（MATCH/GO/FETCH/LOOKUP/FIND PATH/GET SUBGRAPH）
- `crates/linkrs-query/src/parser/parsing/fulltext_parser.rs`、`vector_parser.rs`
- `crates/linkrs-query/src/binder/bind/bind_match.rs`
- `crates/linkrs-query/src/planning/statements/match_statement_planner.rs`
- `crates/linkrs-query/src/planning/statements/pattern_planner/edge.rs`
- `crates/linkrs-query/src/planning/statements/clauses/yield_planner.rs`
- `crates/linkrs-query/tests/dql/`

## 处置建议（对照 `ref/nebula`）

| issue ID | Nebula 基线 | 处置 |
|---|---|---|
| I1 | 支持 `OPTIONAL MATCH`（限制：可选分支中无 WHERE） | **补充**：实现真正的左连接/空值填充 |
| I2 | 不支持递归模式推导 `(v \| ... \| {proj})` | **移除**：从文档和语法层删除 |
| I3 | 不支持 `MATCH ... MERGE` / `MATCH ... DELETE` 读写混合子句 | **移除**：保留独立的 DELETE/INSERT/UPSERT 语句 |
| I4 | 支持长期范围，无 `64` 步 silently 截断 | **补充**：允许合法大范围或显式报错，不再 silent degrade |
| I5 | `TO` 为必填， 默认不是全可达 | **移除**：文档改为必填，去掉真值占位 |
| I6 | 只支持 `WITH PROP`，不支持 `WITH EDGE` | **移除**：删除 `WITH EDGE` 语法 |
| I7 | 支持 `GO [M TO] N STEPS` 区间 | **补充**：接入 `Steps::Range/Variable` |
| I8 | `FETCH PROP` 不接受属性列表，投影走 YIELD | **移除**：删除属性列表语法规则 |
| I9 | `YIELD DISTINCT`/`SAMPLE` 可用 | **补充**：统一独立与后缀两种形态 |
| I10 | 变长步数必须有明确上界（无 `*` 无界 safety net） | **补充+收紧**：无上界时显式报错或取消入口明确化 |

## 问题清单

### I1 [dql-optional-match-inner-join] OPTIONAL MATCH 退化为内联匹配

**现象**  
文档 §1 将 `OPTIONAL MATCH` 列为 MATCH 关键特性。解析层接受该关键字，并将
`optional` 标记存入 `MatchStmt`，绑定层也保留该标记；但规划层 `plan_match_pattern`
没有把 `optional` 转换为左连接/空值填充，`optional` 只在 join-order 路径被用来
“拒绝走 join-order、回退到通用链”，通用链仍按内联匹配执行。执行结果只保留同时
满足所有模式的行，失败的起始点被静默丢弃。

**影响**  
`OPTIONAL MATCH (a)-[:KNOWS]->(b)` 在没有边的起始点上不会得到 `b = NULL`，
查询语义与文档/OPTIONAL 的预期不一致；用例还可能把“无边图”误判为“无结果”。

**根因**  
`optional` 从 parser 流向 AST/Bind/BoundStatement，但从未进入 plan 节点的连接方式；
没有 NULL 扩展和左连接装订路径，执行器只能执行内层匹配。

**严重级**  
P1（静默语义错误）。

**代码位置**  
`crates/linkrs-query/src/planning/statements/match_statement_planner.rs:320-340`、
`crates/linkrs-query/src/binder/bind/bind_match.rs:106`、
`crates/linkrs-query/tests/dql/match_query.rs:416-439`。

---

### I2 [dql-recursive-comprehension-rejected] 递归模式推导语法可解析但被拒绝

**现象**  
文档 §1 声明支持 `-[e*(v | WHERE ... | {node_proj}, {edge_proj})]->` 的递归模式推导。
解析层确实构造 `RecursiveComprehension` 并挂到 `EdgePattern`，但绑定层与规划层
均直接报错：“Recursive comprehension with variable binding and projection is rejected;
rewrite with plain variable-length traversal”。用户能正常通过词法、语法，却在执行前
硬失败，文档与实现不对等。

**影响**  
宣言能力不可用；唯一可运行的替代写法是普通变长遍历， loses 了节点/边投影语义。

**根因**  
递归投影的执行器不存在；特性只完成了 parser/AST 表面。

**严重级**  
P1（ documented 能力不可用）。

**代码位置**  
`crates/linkrs-query/src/parser/parsing/traversal_parser.rs:292-318`、
`crates/linkrs-query/src/binder/bind/bind_match.rs:320`、
`crates/linkrs-query/src/binder/bind/bind_match.rs:410`、
`crates/linkrs-query/src/planning/statements/pattern_planner/edge.rs:79`。

---

### I3 [dql-match-merge-clause-dropped] MATCH 后的 MERGE 子句被消费后丢弃

**现象**  
文档 §1 说“MATCH 模式之后还支持 … 与 MERGE 子句”。解析器在每个模式子句后检查
`Merge` 并调用 `DmlParser::parse_merge_statement`，但其结果完全未挂入 AST；
MERGE 子串虽然被成功消费（无报错），其语义却不进入 `MatchStmt`，也不会执行。

**影响**  
`MATCH ... MERGE ...` 可以被接受为查询，却执行不出合并效果，形成静默半成功。

**根因**  
`parse_match_clause` 无 `merge_clause` 字段；局部 `DmlParser::parse_merge_statement(ctx)?`
的结果被丢弃，属于 parser/AST 接线缺口。

**严重级**  
P1（静默丢失语义）。

**代码位置**  
`crates/linkrs-query/src/parser/parsing/traversal_parser/match.rs:124-129`、
`crates/linkrs-query/src/parser/ast/stmt/dql.rs:29-36`。

---

### I4 [dql-varlen-oversize-silent-degrade] 可变长范围过大时静默退化为单跳

**现象**  
变长范围 planning 中，当 `max-min > 64` 或 `max > 64` 时，`apply_edge_range`
把 `step_limit` 设为 1 并返回，查询继续执行，但实际 hop 数被 silently 截断。
`*1..100` 与 `*1..1` 的表现可能相同，没有警告或错误。

**影响**  
长程查询的观测结果不可信；性能回退和正确性回退混在一起，使用者无从获知截断。

**根因**  
把资源保护 threshold 直接内化为范围语义，默认值/错误通道没有向上层传递。

**严重级**  
P2。

**代码位置**  
`crates/linkrs-query/src/planning/statements/pattern_planner/edge.rs:46-60`。

---

### I5 [dql-find-path-to-optional-default] FIND PATH 的可选 TO 默认成真值表达式

**现象**  
文档 §5 说 `TO` 子句可省略。解析层在 `TO` 缺失时把 `to` 设为
`create_true_expression`（布尔字面量 `true`），这个表达式并不是合法顶点，
后续规划/执行在解释成顶点时会出现语义不匹配或运行时错误；“可省略”并未真实支持。

**影响**  
文档示例中省略 `TO` 的写法不能稳定执行，会用布尔表达式 masquerade 为顶点。

**根因**  
未定义“无目标”的 FIND PATH 结果集规则（全可达域 vs 单源最短树）；
实现 merely 用默认真值填补 AST。

**严重级**  
P2（ documented 入口不可用）。

**代码位置**  
`crates/linkrs-query/src/parser/parsing/traversal_parser/find_path.rs:52-60`。

---

### I6 [dql-get-subgraph-flags-dropped] GET SUBGRAPH 的 WITH PROP / WITH EDGE 仅做词法消费

**现象**  
文档 §8 也 candidly 承认该点：`WITH PROP` / `WITH EDGE` “目前仅完成语法解析，
绑定与执行阶段尚未接入”。AST 中 `SubgraphStmt` 没有这两个标志字段，`_with_edge`
被直接丢弃；返回内容不受它们影响。虽然文档自注，但这仍是语法可达但语义无效
的接口，与“没有静默无效语义”的契约相冲突。

**影响**  
用户写 `WITH PROP` / `WITH EDGE` 得不到声明的属性/边信息差异。

**根因**  
AST 不表达这两个开关，无执行分支。

**严重级**  
P2。

**代码位置**  
`crates/linkrs-query/src/parser/parsing/traversal_parser/get_subgraph.rs:16-21`、
`crates/linkrs-query/src/parser/ast/stmt/dql.rs:104-112`。

---

### I7 [dql-go-steps-range-variable-unimplemented] GO 步数区间与变量形式未接入

**现象**  
文档 §2 已自注：GO 目前仅支持单个整数字面量；`GO 2 TO 4 STEPS` 区间和变量步数
AST 已预留，解析层未接入，非整数字面量静默按 1 步处理。`parse_steps` 对所有
非整数 token 返回 `Steps::Fixed(1)`，执行层也按固定步执行；`Steps::Range`与 `Steps::Variable` 除 MOVE 之外没有构造入口。

**影响**  
用户不能表达“2 到 4 步”的遍历，必须拆写多条查询。

**根因**  
解析层把 token 集收束到整数，未使用 `TO`/标识符分支；规划层 correspondingly
仅处理 `Fixed`。

**严重级**  
P2。

**代码位置**  
`crates/linkrs-query/src/parser/parsing/traversal_parser/go.rs:14`、
`crates/linkrs-query/src/parser/parsing/traversal_parser.rs:393-401`、
`crates/linkrs-query/src/parser/ast/stmt/dql.rs:58-62`。

---

### I8 [dql-fetch-property-list-parsed-and-discarded] FETCH 指定属性列表语法被丢弃

**现象**  
文档 §4 明确指出：`FETCH PROP ON <tag> <id> <prop_list>` 不支持，语法保留但解析后
被丢弃。代码在所有 FETCH 目标分支里把 `properties` 置为 `None`，无论用户是否
写了属性列表。 parser 面接受写法，执行面始终取全部属性。

**影响**  
对属性列表的误用无反馈，查询无法限制传输列。

**根因**  
在“不支持”与“静默接受”之间选了最差的中间态：语法入口保留、语义输出恒等。

**严重级**  
P3。

**代码位置**  
`crates/linkrs-query/src/parser/parsing/util_stmt_parser.rs:145-195`、
`crates/linkrs-query/src/parser/ast/stmt/dql.rs:66-78`。

---

### I9 [dql-yield-standalone-distinct-sample] 独立 YIELD 丢失 DISTINCT/SAMPLE

**现象**  
文档 §9.2 已自注：独立 `YIELD` 语句当前实现丢弃 `DISTINCT` 与 `SAMPLE`，仅在后缀
子句形态完整生效。`YieldStmt` AST 也有这些字段，但执行路径（至少 pipe/独立
路径）在提取时把它们标为 `_sample` / 未平原样传递。后缀子句形态可工作，
同一语句独立使用却不等价。

**影响**  
同一语法在两种使用点语义不一致，调试成本高。

**根因**  
独立与后缀两种通道没有共享同一 lowering。

**严重级**  
P3。

**代码位置**  
`crates/linkrs-query/src/planning/statements/clauses/yield_planner.rs:345,392`、
`crates/linkrs-query/src/parser/ast/stmt/dql.rs:126-136`。

---

### I10 [dql-unbounded-enumeration-safety] 无界变长枚举的安全边界不具手柄

**现象**  
文档 §1 的路径语义说明 admit：纯 `*`、只写语义不写范围都是无界枚举，路径数
随图稠密度阶乘增长；建议加 `*1..5` 或取消查询。执行层确实把 bare `*` 改为 trail，
但 `*TRAIL` themselves 仍可能在稠密图中产生指数路径；取消检查与资源 threshold
是 importer 级的，query 阶段没有明确的 timeout/permission guard 合同。

**影响**  
在生产-like 数据上，合法查询可能长时间占用执行资源；“取消”依赖上层，而文档未
定义取消入口。

**根因**  
无界语义与资源治理 contract 没有在同一层闭合。

**严重级**  
P2。

**代码位置**  
`crates/linkrs-query/src/planning/statements/pattern_planner/edge.rs:60-66`、
`crates/linkrs-query/src/executor/traversal/runtime.rs:250-260`。

---

## 文档层不一致小结

- §1/PARAGRAPH claiming 支持递归模式推导：实现拒绝（I2）。
- §1 声明 `MATCH` 可接 MERGE：实现消费后丢失（I3）。
- §2/§5/§8 中的“区间/可选语法”不少只是语法层真值占位（I5/I6/I7）。
- §4 中 `FETCH ... <prop_list>` 已被文档自 declared unsupported，但 parser 不拒绝
  （I8）。
- §9.2 中独立 YIELD 的 DISTINCT/SAMPLE 限制有文档但无完整实现（I9）。

## 总体结论

本批问题以“ silently 无效语法”和“声明能力过早进入文档”为主，不涉及存储层 corruption。
优先级顺序是：先让所有可达语法 either 真正执行或显式失败，再接通已有 AST 字段，
最后扩展 RANGE/recursive 等真正缺失能力。
