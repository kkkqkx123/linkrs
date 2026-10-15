# P1 [query] FIND SHORTEST PATH 跨多边类型时终点标签解析失败，路径止步于中间顶点

## 现象

`test_find_shortest_path_over_multiple_edge_types`
（`crates/linkrs-query/tests/dql/find_path.rs:586`）失败：

```text
assertion `left == right` failed
  left: [[1, 2, 2]]
 right: [[1, 2, 3]]
```

测试图（`two_edge_type_graph`，`find_path.rs:437`）为：

- 顶点 `Person`：1、2、3；
- 边 `KNOWS`：`1 -> 2`、`2 -> 3`；
- 边 `LIKES`：`2 -> 1`、`1 -> 2`。

查询 `FIND SHORTEST PATH FROM 1 TO 3 OVER KNOWS, LIKES` 期望得到 `1 -> 2 -> 3`
（`1->2` 走 KNOWS，`2->3` 走 KNOWS）。实际返回 `[1, 2, 2]`：路径在顶点 2 处
以自环形式收敛，第三跳没有按 KNOWS 走到 3，终点标签没有被正确解析/绑定。

值得注意的是 `left` 的末位是 `2` 而非 3——这是把终点顶点 ID 回填成了当前顶点，
属于端点解析错误，不是单纯"路径更短"。

## 影响

多边类型列举（`OVER A, B`）这一常见写法在最短路径上返回错误结果，且是静默错误
（`assert_success` 通过，无报错）。用户无法从返回值区分"确实没有更短路径"与
"解析失败退回中间点"。

## 根因

端点标签需跨 `OVER` 列出的多个边类型联合解析（测试注释明写"No single edge type
spans 1 to 3; the endpoint labels must be resolved across all listed types"）。
当前实现对多边的端点标签解析只取了首条边类型（或未合并多边类型的端点域），导致
第三跳的 `dst` 标签缺失，退化为占位。

需核对的位置是 `FIND SHORTEST PATH` 的端点标签解析与逐跳 `dst` 绑定逻辑
（`crates/linkrs-query/src/planning/statements` 下的 path/find 规划，以及
`crates/linkrs-query/src/executor` 中 path 算子的 `dst` 回填）。

## 建议

1. 多边类型 `OVER` 应把各边类型的 `(src_label, dst_label)` 合并为端点域，
   逐跳按当前顶点所属标签与边类型推导下一个顶点标签；
2. 找不到可用标签时应显式报错（对齐下条 bare-id 口径），而不是回填当前顶点；
3. 补测试：多边类型下最短路径的每一跳端点标签断言。

## 严重级

**P1（静默错误结果）**

## 代码位置

`crates/linkrs-query/tests/dql/find_path.rs:579-587`（失败用例）、
`crates/linkrs-query/tests/dql/find_path.rs:437-448`（测试图）、
`crates/linkrs-query/src/planning/statements`（FIND PATH 规划）、
`crates/linkrs-query/src/executor`（path 算子 dst 绑定）
