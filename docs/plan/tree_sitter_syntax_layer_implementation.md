# Tree-sitter 语法层落地细化方案

本文是 `tree_sitter_syntax_layer.md` 的落地细化，记录实现期发现的约束、已完成的代码改动、以及后续分阶段任务。设计层面的三权分立与边界不变，本文只补充"如何落到具体文件与规则"。

## 1. 范围与当前进度

### 1.1 已完成

| 文件 | 改动 |
| --- | --- |
| 根 `Cargo.toml` | `members` 增加 `crates/linkrs-grammar`；`[workspace.dependencies]` 增加 `tree-sitter = "0.26.9"`、`tree-sitter-language = "0.1.7"`、`cc = "1.2"` |
| `crates/linkrs-query/src/parser/lexing/inventory.rs` | 新增：保留字表、软关键字、多词关键字分类、与 `lookup_keyword` 的对齐测试 |
| `crates/linkrs-query/src/parser/lexing.rs` | 注册 `inventory` 模块并再导出 |
| `crates/linkrs-query/src/parser/lexing/lexer/token.rs` | `lookup_keyword` 提升为 `pub(crate)`（仅供同 crate 校验，不对外暴露实现） |
| `crates/linkrs-query/examples/export_keyword_inventory.rs` | 导出 `data/keywords.json`（233 条保留字记录、280 条函数记录；函数按注册键导出，别名拼写保留且无重复） |
| `crates/linkrs-query/src/executor/expression/functions/registry.rs` | 新增 `FunctionSpec` 与 `FunctionRegistry::function_specs()`，函数清单从注册表单一来源导出 |
| `crates/linkrs-grammar/**` | 新增 crate：`Cargo.toml`、`build.rs`、`src/lib.rs`、`queries/highlights.scm`、`grammar.js`、`data/keywords.json` |

### 1.2 已完成（本轮收敛后）

- `grammar.js` 建模修正并用 `tree-sitter-cli 0.27.1` 重生成，三件套（`src/parser.c`、`src/node-types.json`、`src/grammar.json`）同一次生成、原子提交；生成无未登记冲突（已登记项见设计文档三元组表）。
- `linkrs-grammar` 编译验证通过，`tests/smoke.rs` 覆盖高亮查询加载与最小冒烟语句。
- `linkrs-query` 收敛改造完成：死分支清理、`VECTOR[...]` 解析期组装、`Token` 字节区间、函数导出键去重与别名保留、保留字文档一致性测试。
- CLI 接入完成：`linkrs-cli` 依赖 `linkrs-grammar`，关键字与函数补全消费清单，手抄清单与分类映射受生成物校验。
- 前端接入完成：`tools/grammar-codegen` 生成 `frontend/src/lib/utils/generated/`，`gen:grammar` 脚本、`#` 与块注释漂移修正。
- 对等测试落地：`linkrs-query/tests/corpus_parity.rs` 与允许清单（当前为空，只减不增）。

阶段二（门禁固化与结构能力）与阶段三（按需评估）尚未开始。

## 2. 实现期发现的关键约束

这些约束在原设计文档中未充分展开，是落地前必须固化的决策。

### 2.1 工具链与 ABI

- 生成器：`tree-sitter-cli 0.27.1`（npm 包，Linux x64 预编译二进制）。`tree-sitter 0.26.9` 运行时声明语言版本 15、最低兼容 13；CLI 0.27.1 生成 `LANGUAGE_VERSION 15`，与运行时匹配。
- 部署环境无外网且缓存中没有 CLI，因此 `parser.c` / `node-types.json` / `grammar.json` 必须入库，构建期只用 `cc` 编译 C 源码。这与根 `build.rs`（`cbindgen` 生成 `include/linkrs.h`）的既有模式一致。
- 文档需登记三元组：CLI 版本、生成 ABI 版本、运行时版本。升级任一项都必须重生成并跑对等测试。

### 2.2 生成物一致性

- `data/keywords.json` 由 `linkrs-query` 的 example 生成，`linkrs-grammar` 只读取，不回依赖 `linkrs-query`。这样既避免 dev-dependency 环，也满足"grammar 生产依赖只有 tree-sitter"。
- 一致性校验：`cargo run -p linkrs-query --example export_keyword_inventory` 后 `git diff --exit-code`。
- 保留字表与 `lookup_keyword` 的对齐由 `inventory.rs` 内的单元测试保证；任何一侧改动而另一侧未同步都会失败。

### 2.3 词法清单的数据结构

清单至少包含四类条目，缺一类都会导致 grammar 或高亮误判：

- 普通保留字（`lookup_keyword` 的 arm）。
- 软关键字：`GRAPH`、`CHECKPOINT`、`PURGE`、`EXPIRED`。它们不在 `lookup_keyword`，是普通 `Identifier`，由解析器按上下文识别。清单必须显式登记，否则 grammar 会把它们当普通标识符。
- 无条件多词序列：`IS NULL`、`IS NOT NULL`、`IS EMPTY`、`IS NOT EMPTY`、`NOT IN`。词法层合并产出单 token。
- 条件多词序列：`SEARCH VECTOR`、`SEARCH INDEX`、`LOOKUP VECTOR`、`MATCH VECTOR`、`DESCRIBE VECTOR`、`SHOW VECTOR`、`DESCRIBE FULLTEXT`、`SHOW FULLTEXT`、`LOAD EXTENSION`、`UPDATE EXTENSION`、`DETACH DELETE`、`USING JOIN`。第二词不是关键字专属，必须保留在解析期前瞻。

函数清单从 `FunctionRegistry::function_specs()` 导出 `name/arity/variadic/description`。注意 CLI 的 `FunctionEntry` 还有 `category`，注册表没有该信息；生成物只能提供名字、参数个数、描述，`category` 需要一张受生成物校验的受控映射（生成物列出全部函数名，手工条目不得出现在生成物之外）。

### 2.4 grammar.js 的结构规则

实现期踩到两个必须遵守的 tree-sitter 约定：

- **起始符号是 `rules` 对象里的第一个键**。`source_file` 必须保持第一位，否则生成的 parser.c 会退化。
- **生成的关键字规则必须放在词法 `word`（`identifier`）规则之前**。实测确认：若生成块置于末尾（`identifier` 之后），关键字在"关键字与标识符均可出现"的位置会被一律词法化为标识符，导致 `SHOW TAG`、`LOOKUP ON` 等分支永远走标识符分支并误报。正确写法是 `source_file` 第一、生成块第二、其余显式规则随后。
- `optional` 只接受一个规则参数；多个规则必须包成 `seq`。

### 2.5 关键字的大小写不敏感表达

本方言关键字大小写不敏感。tree-sitter 不支持全局大小写开关，采用"逐字母字符类"生成：`MATCH` 生成 `[mM][aA][tT][cC][hH]`。`TRUE`/`FALSE` 不作为关键字规则，而是布尔字面量；`BALANCE` 在词法器里显式映射为标识符，grammar 也不生成关键字规则。

### 2.6 多词关键字在 grammar 中的建模

tree-sitter 不知道 Rust 词法器的合并行为，因此两类多词关键字都要在 grammar 里用 `seq(...)` 表达：

- 无条件类：`is_null`、`is_not_null`、`is_empty`、`is_not_empty`、`not_in`。
- `EMPTY` 不是保留字，需要单独的关键字式正则（如 `empty_word`）参与 `is_empty` / `is_not_empty` 的构造。
- `STARTS WITH` / `ENDS WITH` 同理：`starts_with` / `ends_with` 由 `kw_starts` + `kw_with`、`kw_ends` + `kw_with` 组合。

### 2.7 语句与分段模型

采用"分段（segment）"模型：一条语句由若干以 `|` 分隔的关键字子句组成，段之间可省略 `;`。这样 `MATCH ... RETURN ...`、`GO ... YIELD ... | WHERE ...` 都能自然表达，且树结构对折叠、高亮、结构补全友好。

- 读子句：`MATCH`、`OPTIONAL MATCH`、`GO`、`LOOKUP`、`FETCH`、`FIND`、`GET SUBGRAPH`。
- 投影子句：`RETURN`、`WITH`、`UNWIND`、`YIELD`。
- 修饰子句：`WHERE`、`ORDER BY`、`GROUP BY`、`HAVING`、`SKIP`、`LIMIT`。
- DML：`INSERT`、`UPDATE`、`DELETE`、`UPSERT`、`MERGE`。
- DDL：`CREATE`、`DROP`、`ALTER`、`SHOW`、`DESCRIBE`、`USE`。

### 2.8 冲突处理

已确认会触发生成冲突的模式：**以可选关键字开头、后面又跟可选子句**的段。例如 `SHOW` 段里 `optional(kw_create)` 与 `optional(identifier)` 组合，会产生"消耗 `CREATE` 还是留给下一个段"的二义性。

处理规则：把这类段从 `optional(...)` 改为显式 `choice(...)`，让下一个 token 唯一决定分支。已识别需要按此修正的段：

- `SHOW`：区分 `SHOW CREATE <tag|edge|space> <name>`、`SHOW <tag|edge|space> <name>`、`SHOW <name>`、`SHOW`。
- `DESCRIBE`：区分带对象类型与不带、带名字与不带。
- `ALTER`：动作与括号定义分离，避免 `optional('(')` 与列定义互相吞并。
- `DELETE`：`DELETE VERTEX <expr_list>` 与 `DELETE EDGE <type> <src>-><dst>` 用显式二选一。
- `LOOKUP`：`ON [TAG|EDGE] <name>` 与裸 `<name>` 显式区分。

原则：**冲突不是错误，未登记的冲突才是**。每消解一处都要在生成日志里确认无残留冲突；若某处确实无法靠结构调整消解，才在 `grammar.js` 的 `conflicts` 中登记并注明原因。

### 2.9 对等断言的方向性

对等测试只断言一个方向：**手写解析成功 ⇒ 树无 ERROR/MISSING**。反方向（树认为合法而后端拒绝）只记录不阻断。因此 grammar 可以"偏宽松"，但不能对后端合法的语句产生 ERROR。宽松带来的风险（树接受非法输入）不影响执行，只影响结构能力，可接受。

### 2.10 本轮复核发现的阻塞性问题

以下问题若不先修，后续"重新生成、接入 CLI、跑对等测试"都会建立在不可用的树上返工。它们不是新增需求，而是把原方案已承诺的"单一事实来源、宽松但不误报"落到可验证状态。

- 生成物三件套不一致。`parser.c` 与 `node-types.json` 处于退化状态，`grammar.json` 已是完整规则。三者必须由同一版本 CLI 一次生成并原子提交，缺一不可。后续校验必须覆盖三件套的一致性，而不只是清单。
- 高亮查询引用了不存在的节点。多词谓词在语法中是普通规则（如后文的后缀与二元拆分），查询文件却按关键字规则名引用；布尔字面量在语法中被显式排除，查询文件却仍引用；个别多词缩写的下划线拼写与生成规则不一致。结果是高亮查询加载即失败，`tokens` 入口不可用。修复原则是查询文件只引用语法中真实存在的节点，并用加载测试兜底。
- 同义词只用其一。保留字中存在同形异拼但同语义的条目，语法在部分位置只接受其中之一，导致常用拼写反而不可解析；类型名仅覆盖少数几种，大量已有类型不可解析。修复原则是同义词处用显式二选一，类型名覆盖全部已有类型，缺失项进入允许清单而不是静默不支持。
- 表达式建模有三处必修。与、或链中有一条递归引用了自身而非下一层优先级；区间谓词自身已消费两侧操作数，外层又要求再跟一个操作数；后缀谓词被当成二元运算符要求右侧再跟操作数。以上任一处都会让后端合法的常见谓词在树侧产生误报，违背单向断言的前提，必须在重生成前修正。
- 后缀与二元必须分开。后端合并词与语法展开词的切分不一致，后端单词元对应语法双节点，直接逐条比对必然失配。对等测试需要一层归一化：把后端的合并词展开为序列后再比对，而不是要求双方切分完全相同。
- 词法细节存在多处建模缺口。转义字符串、参数前缀与标识符的切分、位运算与管道符的复用、大小写与字符集、注释与空词的分类，均需按后端行为补齐或显式登记为例外，否则对等测试会把建模缺口误判为回归。
- 清单导出链路有洞。运算符与标点清单是手写列表而非派生，新增词法项需手动同步；函数导出按注册值而非注册键遍历，导致别名丢失且出现重复记录；保留字语义用调试字符串表达，消费者只能依赖拼写，同义词需显式别名表。这些都需在接入 CLI 与前端前收敛，否则生成物会再次漂移。

## 3. 阶段一剩余任务

### 3.1 消解 grammar 冲突并重新生成

1. 先修阻塞性建模，再谈冲突。按第 2.10 节修正高亮查询的非法引用、同义词二选一、类型名补齐、与或链递归、区间与后缀谓词的元数、主表达式的后缀迭代形态、缺失运算符与词法细节。未修这些就重生成，只会得到"无冲突但误报"的树。
2. 再按第 2.8 节把 `SHOW` / `DESCRIBE` / `ALTER` / `DELETE` / `LOOKUP` 段改为显式 `choice`。
3. 反复执行 `tree-sitter generate`，直到输出无未登记冲突、`parser.c` 的规模与三百量级保留字加表达式规则相称。
4. 同一次生成产出的 `src/parser.c`、`src/node-types.json`、`src/grammar.json` 必须原子提交，不允许只更新其一。

### 3.2 编译 linkrs-grammar

- `cargo build -p linkrs-grammar --offline`，确认 `cc` 编译 `parser.c` 成功、`lib.rs` 的 `language()` / `parse()` / `tokens()` / `inventory_json()` 可链接。
- 用一个最小冒烟测试确认 `parse("MATCH (a) RETURN a")` 返回的树无 ERROR，且 `tokens()` 的区间与分类非空。
- 冒烟测试之前先断言高亮查询可加载；加载失败即阻断，不进入后续接入。缺失节点检查显式遍历确认无 MISSING，而不只依赖根节点的错误标志。

### 3.3 CLI 接入

- `linkrs-cli` 增加对 `linkrs-grammar` 的依赖（该 crate 无 linkrs 依赖，不引入环）。
- 用树产出的 token 分类替换 `completer.rs` 的逐字符状态机；分类名直接复用 `highlights.scm` 的捕获名，不再自建颜色映射表。
- 关键字与函数补全改为消费 `inventory_json()`：关键字用清单，函数用清单的 `name/arity/description`。
- 删除 `GQL_KEYWORDS` 与 `get_function_completions` 两处手抄清单。
- 行级回调：由于本方言词法状态不跨行（字符串遇换行报未终止、块注释在换行处结束），`Highlighter::highlight` 按单行解析是安全的；先固定这一事实的测试，再决定是否进入多行结构改造。

### 3.4 前端接入

- 新增 `tools/grammar-codegen/`（Node 脚本，仿 `tools/openapi-codegen`），从 `data/keywords.json` 生成 `frontend/src/lib/utils/generated/keywords.ts` 与 `functions.ts`，文件头标注自动生成。
- `frontend/package.json` 增加与 `gen:api` 同形的 `gen:grammar` 脚本。
- `monacoCypher.ts` 删除 `CYPHER_KEYWORDS` 与 `CYPHER_FUNCTIONS`，改为 import 生成物。
- 修正两处漂移，以 Rust 词法器为准：`#` 不是注释；块注释在换行处结束。字符串不可跨行这条保持现状（两端一致）。
- `quoteIfNeeded` 改为消费生成清单，删除对 `CYPHER_KEYWORDS` 的依赖。

### 3.5 对等测试

- `linkrs-query` 增加对 `linkrs-grammar` 的 dev-dependency，新增 `tests/corpus_parity.rs`（测试位置统一在查询侧，语法侧不再另建对等测试，避免双向 dev 依赖）。
- 语料从存量测试与 `docs/release/0x_*.md` 的示例块抽取，并统计每个语句类别的覆盖条数。
- 断言：手写 `Parser::new(stmt).parse()` 成功 ⇒ 树无 ERROR/MISSING；token 分类逐条比对六类。
- token 比对前先做归一化：把后端合并词展开为词序列后再与树节点对齐，不要求双方切分字节一致，只要求分类与覆盖范围一致。
- 同义词与别名按展开后的规范形比对，避免因拼写差异误报。
- 未覆盖语句进入允许清单文件，每条带原因注释；允许清单只减不增作为健康度指标。
- 由于当前 grammar 是"核心子集 + 宽松建模"，首批语料应取已覆盖形态（MATCH/GO/LOOKUP/FETCH/FIND/GET 与投影子句），再随 grammar 扩展逐步扩大。

### 3.6 linkrs-query 收敛改造

在阶段一收尾时完成，且每项都要有测试兜底：

1. 删除 `lookup_keyword` 中不可达的 `"STARTS WITH"` / `"ENDS WITH"` arm，以及 `check_keyword` 中永不相等的 `StartsWith` / `EndsWith` 多词比较 arm，一并检查同类永假分支并清理。
2. `VECTOR[...]` 从词法层上移到解析期组装，词法层不再吞掉并重排 `lexeme`。
3. `Token` 增加精确字节区间（`start`/`end`），保留 `line`/`column` 既有用法；因涉及多处构造，需一次性说明兼容策略，避免分批改动造成编译轮次膨胀。
4. `reserved-keywords.md` 改为由清单生成，或在 CI 中断言一致。
5. 函数清单导出按注册键去重并显式保留别名映射，消除重复记录；运算符与标点清单改为派生而非手写，避免新增词法项时手动同步遗漏。

### 3.7 文档同步

- `tree_sitter_syntax_layer.md` 登记 CLI 版本、生成 ABI 版本、运行时版本三元组，并以表格形式固定生成时间与产物哈希，避免注释与正文各说一处。
- 更新 `AGENTS.md` 依赖 DAG 一行：`linkrs-grammar` 与 `linkrs-metrics`、`linkrs-core` 同级且无 linkrs 依赖。
- 统一对外命名：查询侧测试位置、清单入口名、产物目录以本文为准，旧设计文档中的不一致路径以本文修订为准。

## 4. 阶段二任务

1. 对等测试转入 CI 门禁，新增语法若无语料支撑即失败。
2. 基于树的能力：括号匹配、折叠范围（`queries/folds.scm`）、基于树节点的结构补全（替代前端 `buildVariableEntityMap` 的正则猜测与 CLI 的字符串嗅探）。
3. CLI 多行缓冲：给 completer 增加由 REPL 循环维护的"当前语句缓冲"共享句柄，每轮 `readline` 前更新。仅管道与位或二义、`VECTOR[...]` 这类结构判断依赖完整缓冲；纯词法分类保持行级。
4. 快照测试：对固定语句样本断言高亮分类输出，防止后续悄悄改变分类口径。

## 5. 阶段三（按需评估，不预设立项）

- 浏览器内树解析：需先给出打包体积预算与编辑体验增益的量化对比，再决定是否 vendor wasm。
- 服务端语法诊断接口：仅当结构级诊断确实无法用现有 `ValidateQuery` 表达时才考虑，且走 `linkrs-wire` 既有 DTO 约定。
- 格式化器与静态检查规则：依赖树稳定后再评估。

## 6. 验收标准

- `cargo build -p linkrs-grammar --offline` 通过；`cargo test -p linkrs-query` 与新增对等测试全绿。
- `data/keywords.json`、前端生成物、`parser.c` 与 `node-types.json`、`grammar.json` 四处 `git diff --exit-code` 校验通过，且三件套来自同一次生成。
- `tree-sitter generate` 输出无未登记冲突；高亮查询可加载，冒烟语句无 ERROR 且无 MISSING。
- CLI 与前端不再存在手抄的关键字/函数清单；`#`、块注释跨行两处漂移消失。
- 手写解析器、binder、optimizer、executor、AST 语义不变。
- 对等语料按语句类别统计覆盖条数，未覆盖项均在允许清单中注明原因；允许清单条数只减不增。

## 7. 风险与对策

- **grammar 冲突反复**：冲突消解是迭代过程，每次只处理生成器报出的冲突，避免一次性大改。冲突无法靠结构调整消解时，显式登记并注明原因。
- **parser.c 体积与编译时长**：数十万行量级 C，仅 `linkrs-grammar` 及其消费者编译；服务端与根二进制不引入。记录编译时长、CLI 二进制体积增量、按键到高亮延迟作为是否上增量解析的判据。
- **对等测试误报**：只断言单向；宽松 grammar 导致的"树接受非法输入"只记录不阻断，避免用树放宽执行语义。token 比对必须经过合并词归一化，否则切分差异会被误判为回归。
- **收敛改造影响解析器**：`VECTOR[...]` 与多词关键字上移属行为保持型重构，落地前全量检索这些 token 的使用点（已确认 `IS NULL` 一族集中在 `expr_parser/property.rs`），改完跑全量解析测试。
- **生成物部分更新**：三件套必须原子提交，CI 同时校验三者与当前语法定义一致，防止再次出现"定义已新、产物一半是旧"的状态。
- **高亮查询与语法脱节**：新增或重命名规则时必须同步更新查询文件，并以加载测试为门禁，防止引用不存在节点导致运行时失败。

## 8. 方案是否需要调整

总体判断是方向不变、任务顺序与验收口径需要调整。原方案的三权分立、离线生成物入库、单向对等等原则依然成立，不需要推翻；但按原文顺序直接"消解冲突、编译、接入、对等"会返工，因为树本身尚未达到可用的正确性基线。

调整为先修正确性基线，再做接入。必修项是高亮查询与语法节点的对齐、同义词与类型覆盖、表达式三处建模、函数导出去重与别名、运算符标点派生、对等归一化。这些完成后，再重生成三件套并原子提交，再进入编译验证与 CLI、前端接入。阶段二的门禁与结构能力、阶段三的按需评估保持不变，只把可合并的编译与校验集中执行，以符合减少编译轮次的约束。
