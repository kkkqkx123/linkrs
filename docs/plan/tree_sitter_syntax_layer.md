# Tree-sitter 语法层引入设计

本文回答三个问题：引入 tree-sitter 来做语法解析与高亮应该如何实现，通过哪些手段压低出错风险，以及现有语法定义是否需要为此改造。结论先行：

1. **实现方式**：新增一个位于依赖图最底层的 `linkrs-grammar` crate，承载 tree-sitter 语法定义与高亮查询；它只提供"结构视图"（token 分类、语句骨架、错误恢复树），**不接管查询执行**。执行真相仍然是现有手写词法器与递归下降解析器。
2. **降险手段**：核心是"单一事实来源 + 生成物 + CI 差异门禁 + 语料差分对等测试"。词法清单由 Rust 词法器导出并生成三处消费者（grammar、CLI、前端），消灭现在四份手工抄写的关键字表；解析行为用现有约 3800 条测试语句做对等校验（后端成功 ⇒ 树无错误节点），未覆盖语法必须显式登记原因而不是静默漂移。
3. **是否改造现有语法定义**：需要，但只做**收敛**不做扩展。必须收敛的是多词关键字的两套处理机制、词法器内的上下文状态（`VECTOR[...]`、`IS NULL` 合并）、注释与字符串的边界规则、以及缺失的 token 字节区间；明确不改造的是 AST、binder、optimizer、executor 与既有语句集合，绝不为迁就 tree-sitter 发明新语法。

---

## 1. 现状盘点

### 1.1 解析链路与规模

查询语言是 Nebula 语句族与 openCypher 的混合方言，解析完全手写：

| 组成 | 位置 | 规模 |
| --- | --- | --- |
| 词法器 | `crates/linkrs-query/src/parser/lexing/lexer/`（`scanner.rs`、`token.rs`） | 294 个 `TokenKind`，`lookup_keyword` 238 条 arm、234 个关键字词 |
| 语法分析 | `crates/linkrs-query/src/parser/parsing/**` | 约 1.5 万行递归下降，含错误恢复（`RecoveryScope`）、类型别名注入、表达式分析上下文 |
| AST | `crates/linkrs-query/src/parser/ast/**` | 约 4.7 千行，直接供给 binder / optimizer / executor |

关键耦合点：`Parser` 在构造时装配 `ExpressionAnalysisContext` 与可选的 `TypeAliasManager`，解析过程中同步产出 `ContextualExpression`。这意味着解析器不是"纯语法"组件，替换它的代价远超语法等价性问题，因此**替换路线在一开始就应排除**。

### 1.2 语法定义存在四份拷贝，且已经漂移

| 消费者 | 位置 | 内容 |
| --- | --- | --- |
| 权威源 | `lexer/token.rs` 的 `lookup_keyword` | 真正决定哪些词是保留字 |
| CLI 补全与高亮 | `crates/linkrs-cli/src/completion/completer.rs` | `GQL_KEYWORDS` 105 条 + 手写逐字符高亮状态机 |
| 前端高亮与补全 | `frontend/src/lib/utils/monacoCypher.ts` | `CYPHER_KEYWORDS` 约 50 条、`CYPHER_FUNCTIONS` 29 条、Monarch tokenizer |
| 文档 | `docs/release/reserved-keywords.md` | 手工维护"关键字 → TokenKind"对照表 |

已经可以观察到的漂移（都是高亮与执行不一致的实证）：

* `#` 被前端当作行注释高亮，而后端词法器中 `#` 仅在 `#>`、`#>>` 中合法，单独出现是词法错误。
* 块注释跨行语义相反：后端 `scanner.rs` 在换行处即终止块注释，前端 Monarch 允许跨到 `*/`。
* 字符串在后端不可跨行（遇到换行报未终止），前端同样按单行处理，这一条恰好一致。
* 函数清单两边都是手抄子集，后端 `executor/expression/functions/registry/builtin.rs` 才是事实来源。
* 保留字规则被重复实现：前端 `quoteIfNeeded` 用正则加关键字表判断是否需要反引号。
* 别名到实体的映射靠正则猜测（`buildVariableEntityMap`），没有语法结构可用。

### 1.3 多词关键字与词法上下文（后续改造的重点）

同一类"多词关键字"存在两套互不一致的机制：

* **词法器内合并**：`IS NULL`、`IS NOT NULL`、`IS EMPTY`、`IS NOT EMPTY` 在 `lexer/token.rs` 里通过前瞻直接合成单 token（`Tk::IsNull` 等）。
* **解析期组合**：`STARTS WITH`、`ENDS WITH`、`USING JOIN`、`DETACH DELETE`、`LOAD EXTENSION`、`UPDATE EXTENSION`、`MATCH VECTOR`、`SEARCH VECTOR`、`SEARCH INDEX`、`LOOKUP VECTOR`、`SHOW FULLTEXT`、`DESCRIBE FULLTEXT` 由 `parse_context.rs` 的 `check_keyword_sequence` 在解析时前瞻两个 token 判断。

此外 `lookup_keyword` 中 `"STARTS WITH"`、`"ENDS WITH"` 两个 arm 实际不可达（该函数只接收单个标识符），属于应清理的死分支。

词法器还带上下文状态：遇到 `VECTOR` 且下一非空白字符是 `[` 时，直接在词法层吞掉整个 `VECTOR[...]` 并产出 `VectorLiteral(Vec<f32>)`，其 `lexeme` 还是重排后的字符串（`VECTOR[1, 2]`），不等于源码原文。

### 1.4 Token 缺少精确区间

`Token` 只有 `line` / `column`，没有字节区间；反引号标识符的 `lexeme` 又剥掉了反引号。做高亮需要把 token 映射回源码范围，目前要么重新扫一遍字符，要么接受偏差。tree-sitter 原生给出每个叶子节点的精确字节区间，这是它对本项目最直接的收益之一。

### 1.5 环境能力（已实测）

* cargo 离线缓存中已有 `tree-sitter 0.26.9`、`tree-sitter-language 0.1.7`、`cc`、`gcc`。用一个临时 crate 验证：`cargo metadata --offline` 解析通过，`cargo build --offline` 编译通过（约 4 秒）。即**运行时依赖可以离线获得**。
* `tree-sitter 0.26.9` 声明的语言版本为 15，最低兼容 13。
* `tree-sitter` CLI 不在本地任何缓存中，需联网安装；开发网可达、部署网不可达。因此 `parser.c` 必须作为生成物入库，由固定版本 CLI 一次性生成，构建期只用 `cc` 编译 C 源码。这与根 `build.rs` 已有的一次性生成 + 提交产物模式一致。
* 前端 `node_modules` 与 npm 缓存中都没有 `web-tree-sitter`。浏览器内做树解析需要新增前端依赖并把 grammar 的 wasm 入库，打包体积与加载链路都要重新评估，**现阶段不做**，只在阶段三列为待评估项。
* `ref/ladybug`、`ref/neug` 中只有 ANTLR 的 `Cypher.g4`，没有 tree-sitter grammar 可直接复用；且本方言独有 `GO/FETCH/LOOKUP`、管道 `|`、`$^.tag` 这类语法，照抄会引入大量假冲突。它们只能作为规则起草时的参考，不能作为基线。

---

## 2. 设计原则与边界

三条职权分立，是整套方案不失控的前提：

1. **执行真相在手写解析器**：AST、binder、语义检查、optimizer、executor 一行不动。查询能否执行、报什么错，仍由现有链路决定。tree-sitter 的任何结论都不得反向否决或改写执行结果。
2. **结构真相在 tree-sitter 语法**：语句骨架、子句顺序、表达式优先级、token 分类、高亮与折叠范围，以 `grammar.js` 与查询文件为准，供编辑器类能力消费。
3. **词法清单真相在 Rust 词法器**：哪些词是保留字由 `lookup_keyword` 决定（含大小写折叠、反引号例外、保留字不能用作属性名等语义）。grammar 与前端只消费导出的清单，不再各自维护。

明确的非目标：

* 不做查询语言的新版本，不为树解析新增、删除或改写任何既有语句语法。
* 不在前端引入 wasm 运行时。
* 不新增服务端语法接口（gRPC/HTTP 的语法诊断、高亮端点），除非阶段三出现明确需求；现有 `ValidateQuery` 语义不变。
* 不把手写解析器换成树驱动解析器；也不做"树解析成功才允许执行"这类门禁。

---

## 3. 架构设计

### 3.1 新增 crate：`linkrs-grammar`

职责单一：把本方言的 tree-sitter 语法定义、生成的解析器、高亮查询，以及面向消费者的薄封装收在一处。它位于依赖图最底层，与 `linkrs-metrics`、`linkrs-core` 同级，**不依赖任何 linkrs crate**（生产依赖只有 `tree-sitter` 与 `tree-sitter-language`）。

对外只暴露四类能力，均以自然语言描述为准，不暴露树遍历细节：

* **语言句柄**：返回 `tree_sitter::Language`，供 CLI 与工具链使用。
* **解析入口**：输入源码返回解析结果（树 + 错误节点列表）。输入不完整或有错时同样返回树，调用方自行决定如何呈现。
* **token 序列**：按源码顺序产出带精确字节区间与分类的 token 列表，分类名与高亮查询中的捕获名一一对应，供 CLI 行内高亮直接使用。
* **词法清单**：导出声明的关键字、运算符、标点集合及其分类，供一致性测试与前端代码生成使用。

生产代码不暴露树给 `linkrs-query`；`linkrs-query` 只以 dev-dependency 身份出现在对等测试中。

### 3.2 依赖方向与编译成本

`linkrs-grammar` 只被 `linkrs-cli`（REPL 高亮与结构补全）与工具链（代码生成、对等测试）依赖。后端主链路（`linkrs-query` 生产依赖、`linkrs-server`、根二进制）完全不引入 tree-sitter，parser.c 的 C 编译开销与二进制体积增量只落在 CLI 一侧，服务端产物零变化。若要整体摘除，改动限于一个新 crate 与 CLI 的一处高亮实现。

落地时需同步两处登记：根 `Cargo.toml` 的 `[workspace] members` 增加新 crate，`AGENTS.md` 的依赖 DAG 一行补上它与 `linkrs-metrics`、`linkrs-core` 同级且无 linkrs 依赖的说明。

### 3.3 目录布局

```text
crates/linkrs-grammar/
├── grammar.js                 # 语法定义（关键字规则由 data/keywords.json 生成）
├── data/keywords.json         # 词法清单，单一事实来源的导出产物
├── queries/highlights.scm     # token → 高亮分类
├── queries/folds.scm          # 折叠范围（阶段二）
├── src/parser.c               # CLI 一次性生成并提交
├── src/scanner.c              # 外部扫描器（关系模式等需要时）
├── src/node-types.json        # 生成产物，供代码生成与文档
├── src/lib.rs                 # 薄封装：language() / parse() / tokens() / inventory()
├── examples/export_inventory.rs  # 从 linkrs-query 导出词法清单（dev-dependency）
└── tests/corpus_parity.rs     # 与手写解析器的差分对等测试
```

工具侧新增 `tools/grammar-codegen/`（Node 脚本），前端生成物落在 `frontend/src/lib/utils/generated/`，沿用 `tools/openapi-codegen` 已有的"生成物入库 + 文件头标注 auto-generated"模式，并在 `frontend/package.json` 增加与 `gen:api` 同形的脚本。

---

## 4. 单一事实来源与生成链路

### 4.1 词法清单：Rust 词法器为权威，向下导出

`data/keywords.json` 由 `examples/export_inventory.rs` 从 `linkrs-query` 的 `lookup_keyword` 与运算符枚举读出后写出，入库并由 CI 校验重生成结果与提交一致。它同时服务三处：

* **grammar.js**：以 `require` 方式读取并动态生成关键字规则。本方言关键字大小写不敏感，若手写需要为 234 个词各写大小写组合，等于放弃维护；由清单按字符类生成是唯一可维护的写法。
* **CLI**：高亮与补全的关键字来源改为清单，删除 `GQL_KEYWORDS`。
* **前端**：`tools/grammar-codegen` 从同一份清单生成 Monarch 的关键字数组，删除 `CYPHER_KEYWORDS`；函数清单从 Rust 函数注册表生成，删除 `CYPHER_FUNCTIONS`。

方向上要强调的一点：把权威放在 Rust 词法器而不是 grammar.js，是因为保留字语义（能否作为属性名、反引号例外、大小写折叠）由 `lookup_keyword` 实际决定，grammar 只是消费者。反过来做会让保留字语义出现第二个真相。

### 4.2 语法结构：grammar.js 为权威

语句骨架、子句顺序、表达式优先级以 `grammar.js` 为准。它对下游的承诺是"结构分类"，不是"能否执行"。即便某条语句后端支持而 grammar 暂未覆盖，也只会表现为结构能力缺位（不高亮某个实体、不折叠、不给结构补全），绝不影响执行。

### 4.3 高亮分类：查询文件为权威

`queries/highlights.scm` 定义 token 到分类的映射，分类命名直接复用编辑器生态通行的捕获名（关键字、字符串、数字、注释、运算符、标点、类型、函数、变量、参数、属性、内置常量），使 Rust 侧与前端侧无需再建一张翻译表。需要如实说明的不对称：**前端没有树**，只能消费其中词法类分类（关键字、运算符、数字、字符串、注释、标识符）；变量、函数这类依赖结构的分类在前端仍由 Monarch 的函数表与 schema 名规则近似。这一点要在代码与文档中写明，避免误以为两端完全等价。

### 4.4 生成与校验流程

四个生成物与三道校验构成闭环：

* 生成物：`data/keywords.json`（Rust 导出）、`src/parser.c`（CLI 生成）、`queries/highlights.scm` 对应的前端 TS、`src/node-types.json`。
* 校验一：词法清单一致性——`cargo run -p linkrs-grammar --example export_inventory` 后 `git diff --exit-code`。
* 校验二：前端生成物一致性——`npm run gen:grammar` 后 `git diff --exit-code`。
* 校验三：语料差分对等——见第 8 节。

三道校验都在普通构建之外按需执行，不进常规编译路径，符合"合并验证、减少编译轮次"的约束。

---

## 5. 分阶段落地

### 阶段一：语法骨架与词法清单统一（低风险、纯增量）

目标是不改变任何执行行为的前提下，先把"漂移"和"手抄"两个存量问题解决掉。

* 建立 `linkrs-grammar`，parser.c 由固定版本 CLI 生成入库，构建期只用 `cc`；先在 CI 记录 CLI 版本与语言 ABI 版本。
* grammar 首批覆盖：读子句族（`MATCH`/`OPTIONAL MATCH`/`GO`/`LOOKUP`/`FETCH`/`FIND PATH`/`GET SUBGRAPH`）、管道与集合运算、投影子句（`RETURN`/`WITH`/`UNWIND`/`YIELD`/`ORDER BY`/`GROUP BY`/`HAVING`）、表达式与优先级、常见 DDL/DML/DCL 骨架。**允许不覆盖**，但每处不覆盖都必须写进对等测试的允许清单并注明原因。
* CLI 高亮改为消费树产出的 token 分类，替换 `completer.rs` 中的逐字符状态机。
* 前端 Monarch 与词法清单对齐：修正 `#` 注释、块注释跨行两处漂移（以 Rust 词法器为准），关键字与函数清单改为生成。
* 删除三处手抄清单（CLI `GQL_KEYWORDS`、前端 `CYPHER_KEYWORDS`、`CYPHER_FUNCTIONS` 的手写数组）。

本阶段可独立回滚：停用新 crate、还原 CLI 高亮与前端 Monarch 即可，执行链路从未被触碰。

### 阶段二：结构能力接入与门禁固化

* 差分对等测试转入 CI 门禁，新增语法若无语料支撑即失败。
* 基于树的能力：括号匹配、折叠范围、基于树节点的结构补全（替代前端正则猜别名与 CLI 的字符串嗅探）。
* CLI 多行缓冲打通：rustyline 的 `Highlighter::highlight` 只回调单行，而结构分类需要完整语句。方案是给 completer 增加一个由 REPL 循环维护的"当前语句缓冲"共享句柄，每轮 `readline` 前更新。需要说明的是：本方言词法状态不跨行（字符串遇换行报未终止、块注释在换行处结束），因此**纯词法分类在行级就是安全的**；只有管道与位或二义、`VECTOR[...]` 这类结构判断才必须依赖完整缓冲。可据此把阶段二的改造拆成"先词法、后结构"两步。

### 阶段三：按需评估，不预设立项

* 浏览器内树解析：需先给出打包体积预算与编辑体验增益的量化对比，再决定是否 vendor wasm。
* 服务端语法诊断接口：只有当"结构级诊断"确实无法用现有 `ValidateQuery` 表达时才考虑，且必须走 `linkrs-wire` 既有 DTO 约定。
* 格式化器与静态检查规则：依赖树稳定后再评估，避免规则半成品进入发布路径。

---

## 6. 风险与对策

**语法冲突被隐性接受。** `|` 同时是管道分隔符、表达式位或、列表推导式的分隔符；`::` 与 `:`、`->` 与 `-->`、`#>` 与 `#>>` 需要最长匹配；`-[...]->` 关系模式中括号内容上下文敏感（属性 map、变长 `*1..3`、`*WEIGHTED(w)`）。对策：在 grammar.js 中显式声明优先级与冲突项，`tree-sitter generate` 输出中不允许出现未登记冲突；关系模式若要外部扫描器，则 `scanner.c` 与 `parser.c` 一并入库并纳入同一版本管理。冲突不是错误，未登记的冲突才是。

**双源漂移。** 对策即第 4 节的生成链路：不允许存在第四份手抄清单，代码评审时把"新增手写关键字/函数表"列为阻塞项；生成物每次提交前重跑并用 `git diff` 校验。

**树被误当成执行真相。** 对策：高亮与补全只消费 token 分类与结构节点；任何"树说它错所以拒绝执行"的逻辑一律不写；`ValidateQuery` 仍走手写 parser。错误树同样是树，ERROR 节点不赋予其否决权。

**词法上下文项无法在 grammar 中表达。** `VECTOR[...]` 现在是在词法器里吞掉的，`IS NULL` 类合并也藏在词法层。对策见第 7 节改造项：把这些状态从词法器上移到解析期组合，grammar 用普通规则表达（`VECTOR` 关键字后接方括号数字列表，`IS NOT NULL` 作为多词关键字规则），行为保持不变但表达力回到语法层。

**编译开销与体积。** parser.c 是数十万行量级的 C。对策：只有 `linkrs-grammar` 及其消费者编译它，服务端与根二进制不引入；CLI 侧关注启动时间与按键到渲染延迟，树解析应在亚毫秒级，必要时再上增量解析。

**CLI 单行回调拿不到完整语句。** 对策见阶段二，并先把"词法分类行级安全"这一事实用测试固定住，防止后来者误以为需要一次性做完多行改造。

**工具链与 ABI 漂移。** CLI 版本、生成的语言 ABI 版本、`tree-sitter` 运行时版本三者在文档中登记；升级运行时需同步重生成 parser.c 并跑对等测试，不允许只升一边。

**语料覆盖不足导致假信心。** 对策：语料同时取自存量测试（约 3800 条语句起点）与 `docs/release/0x_*.md` 的示例块，并统计每个语句类别的覆盖条数；对等断言只做"后端解析成功 ⇒ 树无 ERROR/MISSING"这一方向，反方向（树认为合法而后端拒绝）只记录不阻断，避免用树放宽执行语义。

---

## 7. 现有语法定义的改造清单

结论是需要改造，但全部属于**收敛既有机制**，不新增语法能力。

### 必须改造

1. **多词关键字机制统一**。现在同一概念两套实现（词法器合并 `IS NULL` 一族、解析期组合 `STARTS WITH` 一族），既难维护也无法直接生成 grammar。建议统一为一份多词关键字登记表，由词法器负责合并产出单 token，解析期只匹配单 token；登记表同时驱动 grammar 的多词规则与高亮。附带清理 `lookup_keyword` 中不可达的 `"STARTS WITH"`、`"ENDS WITH"` arm。
2. **词法器上下文状态外移**。`VECTOR[...]` 不应在词法层被吞掉并重排 `lexeme`。改为产出普通 token 序列、由解析期组装为向量字面量。收益：grammar 可表达、高亮区间精确、词法错误定位更准，且执行语义不变。
3. **注释与字符串规则成文并对齐**。明确三条：`#` 不是注释；块注释在换行处结束；字符串不可跨行。以 Rust 词法器为准修正前端高亮，然后三处（词法器、grammar、前端）一致。这是当前最容易被用户察觉的不一致。
4. **保留字规则单一实现**。删除前端 `quoteIfNeeded` 的正则近似，改消费词法清单（含大小写折叠与反引号例外语义）。
5. **函数清单生成化**。前端与 CLI 的函数补全表改为从 Rust 函数注册表生成，避免建议不存在的函数。
6. **Token 增加精确字节区间**。为高亮与树对齐提供区间来源；不影响 line/column 既有用法。反引号标识符的 `lexeme` 与源码不一致的问题一并修正。
7. **保留字文档改为生成**。`docs/release/reserved-keywords.md` 目前手工对应 `lookup_keyword`，改为由导出清单生成，或在 CI 中以断言校验一致。

### 明确不改造

AST 与其 `Stmt`/`StmtCategory`、binder、optimizer、executor、错误恢复机制、既有语句集合与关键字集合一律不动。尤其注意：**不得为了让 tree-sitter 好写而调整语法**，例如把管道分隔符换成别的符号、把大小写敏感化、或新增/删除语句。语法演进的需求走正常语言特性流程，与本次接入解耦。

---

## 8. 质量门禁与验收

* **对等测试**：从 `crates/linkrs-query/tests/**` 与 `docs/release/0x_*.md` 抽取语料；断言"手写解析成功 ⇒ 树无 ERROR/MISSING"；token 分类逐条比对（关键字/标识符/字面量/注释/算符/标点六类）；未覆盖项进允许清单文件，每条必须带原因注释。允许清单只减不增是长期健康度指标。
* **生成物一致性**：三条 `git diff --exit-code` 校验（词法清单、前端生成物、parser.c 与 grammar.js 的同步记录）。
* **快照测试**：对固定语句样本断言高亮分类输出，防止后续为高亮"优化"而悄悄改变分类口径。
* **冲突与覆盖统计**：grammar 每个语句类别至少要有语料条数下限；`tree-sitter generate` 不允许未登记冲突。
* **性能基线**：记录 parser.c 编译时长、CLI 二进制体积增量、按键到高亮渲染延迟，作为阶段二是否上增量解析的判据。

---

## 9. 待确认问题

* 多词关键字统一后，`IS NULL` 一族合并为单 token 是否会影响 binder 或表达式中对该 token 的既有判断；落地前需全量检索这些 token 的使用点。
* `VECTOR[...]` 上移到解析期后，`VectorLiteral(Vec<f32>)` 是否保留为独立 token 类型（保留则 parser 组装，取消则由 parser 直接构造字面量值）；倾向于后者以减少 token 类型的职责混淆。
* 外部扫描器是否必需，取决于关系模式规则在纯 JS 规则下的冲突规模，需在首批 grammar 落地后按实测冲突数决定。
* CLI 的多行缓冲句柄是否需要同时服务补全上下文判断，若需要应一次性设计，避免两套缓冲。

---

## 10. 决策记录

* 不替换手写解析器：解析器与类型别名、表达式上下文深度耦合，替换风险与收益不对称。
* parser.c 入库而非构建期生成：部署环境无外网，CLI 工具不在缓存中；与根 `build.rs` 的"生成物入库"模式一致。
* 词法清单权威放在 Rust 词法器：保留字是执行语义，不能由编辑器侧定义。
* 前端暂不引入 wasm：新增前端依赖与 wasm 入库成本明确，而阶段一的词法清单统一已能消掉现存漂移。
* 不新增服务端语法接口：现有 `ValidateQuery` 已覆盖"能否执行"的诉求，树的能力先用编辑器侧消费。
