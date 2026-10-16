# DQL 查询语法对齐：分阶段修改方案

对应问题清单：`docs/issue/dql-query-syntax-audit.md`。

## 总原则

1. 发布文档是查询契约。每条不一致二选一：补齐实现，或同步从文档和语法里移除。
   不保留“可解析但静默失效”的中间态。
2. 不新增兼容层。所有改动落在原有模块里。
3. 默认真值占位、空名分支、整数外挂 1 之类的静默 fallback 都必须显式化。
4. 每阶段结束时仓库可编译、已有 DQL 测试全绿。

## 问题文件索引

| 问题文件主题 | 严重级 | 处置 | 主题 |
|---|---|---|---|
| dql-optional-match-inner-join | P1 | 补充 | OPTIONAL MATCH 退化为内联匹配 |
| dql-recursive-comprehension-rejected | P1 | 移除 | 递归模式推导语法可解析但被拒绝 |
| dql-match-merge-clause-dropped | P1 | 移除 | MATCH 后 MERGE 子句被消费后丢弃 |
| dql-varlen-oversize-silent-degrade | P2 | 补充 | 超大可变长范围静默退化 |
| dql-go-steps-range-variable-unimplemented | P2 | 补充 | GO 步数区间/变量未接入 |
| dql-get-subgraph-flags-dropped | P2 | 移除 WITH EDGE，补充 WITH PROP | GET SUBGRAPH 开关被丢弃 |
| dql-find-path-to-optional-default | P2 | 移除 | FIND PATH 可选 TO 真值占位 |
| dql-unbounded-enumeration-safety | P2 | 补充+收紧 | 无界枚举资源边界 |
| dql-fetch-property-list-parsed-and-discarded | P3 | 移除 | FETCH 属性列表被丢弃 |
| dql-yield-standalone-distinct-sample | P3 | 补充 | 独立 YIELD 丢失 DISTINCT/SAMPLE |

## 阶段零：显形化——让假能力 either 可用或明确失败

目标：消除 parser 接受但执行丢失/占位的语法。

任务：

1. OPTIONAL MATCH（dql-optional-match-inner-join）
   - 先把 `match_stmt.optional` 缺失的执行路径暴露为显式错误或明确的 TODO 限界：
     若继续接受语法，则 planner 须拒绝仅占位路径（当前 generic chain）并报
     “OPTIONAL MATCH execution is not implemented yet”。
   - 同时从文档里把 OPTIONAL MATCH 的执行语义声明降级为“仅解析/设计中”，避免契约空挂。

2. MATCH ... MERGE（dql-match-merge-clause-dropped）
   - 在 `parse_match_clause` 里移除无消费结果的 `DmlParser::parse_merge_statement` 分支；
     若 grammar 仍保留 Merge 词，则 parser 直接报 unsupported parse error。
   - 文档 §1 删除或明确“MERGE 子句只允许作为独立语句”。

3. FETCH 属性列表（dql-fetch-property-list-parsed-and-discarded）
   - parser 对 `<prop_list>` 直接报 unsupported，或在 AST 中真解析并贯通。
     建议先报错，把 silent acceptance 砍掉。

4. GO 非整数步数（dql-go-steps-range-variable-unimplemented）
   - `parse_steps` 对 Range/Variable 不再 `Fixed(1)` 静默；改为 parse error，
     同时更新 doc 示例，只说明整数支持。

5. GET SUBGRAPH WITH EDGE（dql-get-subgraph-flags-dropped）
   - parser 要么接受并把字段接入 AST，要么拒绝 `WITH EDGE`（当前 `WITH PROP`
     至少 parser 已消费；WITH EDGE 也应一致化）。

6. FIND PATH TO 默认（dql-find-path-to-optional-default）
   - parser 不再把 true 表达式塞入 `to`，改为 missing 时报 parse error 或把 TO 设为可选项 AST。

验收：上述语法的不可用写法得到明确错误，可用写法保持原行为；DQL 解析测试全绿。

## 阶段一：语义修复

目标：把显形化后保留的声明能力真正执行出来。

任务：

1. OPTIONAL MATCH 左连接
   - 在 `plan_match_pattern` 中根据 `match_stmt.optional` 生成 left join；空匹配时右值
     填充 NULL，并让 WHERE 不把左行过滤掉（语义等价于 LEFT OUTER）。
   - 需要配合 row shape 中 nullable 列。

2. 递归模式推导
   - 定义 `RecursiveComprehension` 的执行语义：step-level binding、projection、filter。
   - 要么实现 dedicated expander，要么将语法拆进可执行的 CALL/eval 模型。

3. GET SUBGRAPH WITH PROP / WITH EDGE
   - `SubgraphStmt` 增加 `with_prop`/`with_edge` 字段，并执行时控制属性与边输出。

## 阶段二：接线与补强

目标：把已 parsing 但 downstream 丢失的 AST 字段贯通。

任务：

1. FETCH 属性列表
   - `FetchTarget::Vertices.properties` 真正接通 planner/executor，支持列裁剪。

2. GO 区间/变量步数
   - parser/planner/executor 支持 `Steps::Range` 与 `Steps::Variable`。

3. FIND PATH TO 可选
   - 支持全可达域/源到所有节点模式，或在文档明确 TO 必填。

4. YIELD standalone DISTINCT/SAMPLE
   - 独立与后缀形态共享同一条 lowering.

5. 大范围 threshold 显式化
   - 超出执行边界时抛明确错误或切到分块遍历，不要再静默 single-hop。

## 阶段三：清理与文档同步

任务：

1. 文档 §1/§2/§4/§5/§8 与代码最终状态一一对应，删除仍不可运行的能力描述。
2. 删除 parser 中所有静默 fallback 与 dead default 路径。
3. 补 parser/plan/execution tests，把每个 phase 的语法 either 通过或拒绝锁定。

## 验收

- 所有文档示例语句都能运行或被明确拒绝。
- 没有“语法成功但语义 silently 丢失”的中间状态。
- 无界查询要求显式给 timeout/guard，不 silently 完整跑满。
