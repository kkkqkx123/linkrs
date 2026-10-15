# P1 [query] FIND PATH 省略 OVER 时被解析器报错拦截，执行器 bare-id 语义不可达

## 现象

`test_find_path_bare_id_without_edge_types_errors`
（`crates/linkrs-query/tests/dql/find_path.rs:590`）失败：

```text
expected the bare-id endpoint error, got:
DBError { kind: Query, message: "[parse] Parse error at line 1, column 30: Expected Over, found Eof ..." }
```

查询 `FIND SHORTEST PATH FROM 1 TO 3`（省略 `OVER`）期望在执行期得到
"bare id" 语义错误（"Without OVER the edge schema cannot determine the endpoint
label of a literal id"），实际在解析期就被报 `Expected Over` 拦下，
执行器里对应的 bare-id 分支从未被执行。

测试 `assert_error()` 本身通过（确实报错了），失败的是错误**内容**断言
（期望 `error.contains("bare id")`）。

## 影响

两份实现意图互相矛盾且外显行为误导：

- 语法层把 `OVER` 写成可选（`crates/linkrs-query/src/parser/parsing/traversal_parser/find_path.rs:71-95`
  用 `over_present` 分支构造空的 `OverClause`）；
- 执行器专门实现了 bare-id 报错
  （`crates/linkrs-query/src/executor/streaming/operators/algorithms.rs:22-47`，
  文案 "Path endpoint is a bare id: OVER must list edge types ..."）。

结果是：无 `OVER` 的合法输入既不是"按空 OverClause 执行"，也不是"执行期 bare-id
报错"，而是解析期一个与设计意图无关的 `Expected Over` syntax error。用户看到
的错误指向了不存在的语法要求，排查会被误导；同时执行器的 bare-id 分支成为死路径。

## 根因

`find_path.rs:71-77` 用 `recover_clause(|| Ok(false), |c| { c.expect_token(Over); Ok(true) })`
实现"可选 `OVER`"。当 `OVER` 确实缺席时，`recover_clause` 的失败分支虽返回 `false`，
但仍把 `expect_token` 产生的 `Expected Over` 记入 recovered parse errors，
最终由 pipeline 作为解析错误抛出，覆盖了应有的"空 OVER + 执行期校验"路径。

即：把"子句可选"实现成了"子句缺失也报错"，与 `over` 为空 `OverClause` 的后续逻辑
（`:89-95`）自相矛盾。

## 建议

1. 让 `OVER` 真正可选：缺失时不产生任何 parse error，构造空 `OverClause`，
   交由执行器 bare-id 校验统一报错（与 `algorithms.rs` 的既有文案对齐）；
   用 `check_token`/`match_token` 判定是否存在，而非 `recover_clause` 的失败分支。
2. 明确 bare-id 校验归属：端点标签无法从 schema 推导时应报执行期错误，
   与 `find-path-shortest-multi-edge-type-endpoint` 的端点标签解析修复共用一处口径。
3. 补测试：无 `OVER` 且端点带标签（可解析）/不带标签（bare id，报错）两条路径。

## 严重级

**P1（错误信息误导 + 执行器分支不可达）**

## 代码位置

`crates/linkrs-query/tests/dql/find_path.rs:589-601`（失败用例）、
`crates/linkrs-query/src/parser/parsing/traversal_parser/find_path.rs:71-95`（可选 OVER 解析）、
`crates/linkrs-query/src/executor/streaming/operators/algorithms.rs:22-47`（bare-id 报错）
