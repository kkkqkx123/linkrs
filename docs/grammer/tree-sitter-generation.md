# Tree-Sitter 语法生成逻辑分析

## 1. 架构总览

Linkrs 的 tree-sitter 集成被隔离在单一叶子 crate `crates/linkrs-grammar` 中，位于依赖 DAG 底部，不依赖任何其他 linkrs crate。后端执行管线（`linkrs-query`、`linkrs-server`、根二进制）**不链接 tree-sitter**——只有 CLI 和工具链消费它。

核心设计原则：**语法由固定版本的 CLI 一次性生成，生成的产物提交到仓库，构建时仅用 `cc` 编译 C 源码**（无网络、无 CLI 依赖）。

```
┌─────────────────────────────────────────────────────────────────────┐
│  Phase A: 一次性生成（开发者机器，需要网络 + Node）                   │
│                                                                     │
│  Rust Lexer ──export──> keywords.json ──require──> grammar.js       │
│                                              │                      │
│                                              ▼                      │
│                                    tree-sitter generate              │
│                                              │                      │
│                          ┌───────────────────┼───────────────────┐  │
│                          ▼                   ▼                   ▼  │
│                     parser.c         node-types.json      grammar.json│
│                     (提交)             (提交)              (提交)    │
└─────────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────────┐
│  Phase B: 构建/部署（离线，无 CLI）                                   │
│                                                                     │
│  build.rs 验证产物同步性 ──> cc 编译 parser.c ──> lib.rs 绑定        │
│                                                                     │
│  消费者: language() → parse() / tokens() → 语法树 + 分类 token       │
└─────────────────────────────────────────────────────────────────────┘
```

## 2. 文件清单与职责

### 2.1 语法定义与生成产物

| 路径 | 职责 |
|------|------|
| `crates/linkrs-grammar/grammar.js` | **手写语法定义**（600 行）。Linkrs 方言语法树的唯一真相源。加载时 `require` `data/keywords.json` 以程序化生成关键字规则。定义 `name: 'linkrs'`、extras、word 规则、5 组声明的冲突、完整规则集。 |
| `crates/linkrs-grammar/src/parser.c` | **生成的 C 解析器**（~45k 行）。由 `tree-sitter-cli 0.27.1` 一次性产出。包含 `tree_sitter_linkrs()` 入口，`LANGUAGE_VERSION 15`。由 `build.rs` 通过 `cc` 编译。 |
| `crates/linkrs-grammar/src/node-types.json` | **生成的节点类型元数据**。供工具/编辑器内省节点。必须与 `parser.c` 一起提交。 |
| `crates/linkrs-grammar/src/grammar.json` | **生成的语法 JSON**（`grammar.js` 的展开、解析后形式）。记录 `"name": "linkrs"`——由 `build.rs` 验证。 |
| `crates/linkrs-grammar/src/tree_sitter/parser.h` | Tree-sitter 运行时头文件（vendored）。 |
| `crates/linkrs-grammar/src/tree_sitter/alloc.h` | Tree-sitter 运行时头文件（vendored）。 |
| `crates/linkrs-grammar/src/tree_sitter/array.h` | Tree-sitter 运行时头文件（vendored）。 |
| `crates/linkrs-grammar/src/lib.rs` | **Rust 绑定层**。暴露 `language()`、`parse()`、`tokens()`、`inventory_json()`。声明 `extern "C" { fn tree_sitter_linkrs() -> *const (); }` 并通过 `tree_sitter_language::LanguageFn::from_raw` 包装为 `tree_sitter::Language`。通过 `include_str!` 嵌入 `highlights.scm` 和 `keywords.json`。 |
| `crates/linkrs-grammar/data/keywords.json` | **词法清单快照**（已提交）。~233 个保留字、280 个函数、运算符、标点、多词序列。由 `linkrs-query` 生成（见下文）。`grammar.js` 在生成时读取此文件以产出 `kw_*` 规则。 |
| `crates/linkrs-grammar/queries/highlights.scm` | **高亮查询**（176 行）。基于 capture 的分类（`@keyword`、`@operator`、`@number`、`@string`、`@variable`、`@property`、`@function`、`@comment`）。运行时由 `lib.rs` 加载，由冒烟测试验证。 |

### 2.2 构建脚本与生成脚本

| 路径 | 职责 |
|------|------|
| `crates/linkrs-grammar/build.rs` | **编译生成的解析器。** 验证三个已提交产物处于同步状态（断言 `parser.c` 包含 `#define LANGUAGE_VERSION 15`，`grammar.json` 包含 `"name": "linkrs"`，`node-types.json` 存在，`keywords.json` 有 `"version": 1`），然后运行 `cc::Build::new().include(src).file(src/parser.c).compile("tree-sitter-linkrs")`。对所有输入发出 `cargo:rerun-if-changed`。**不调用 tree-sitter CLI。** |
| `crates/linkrs-grammar/package.json` | **固定生成工具链。** `devDependencies: tree-sitter-cli 0.27.1`。脚本：`"generate": "tree-sitter generate"`，`"verify": "node --check grammar.js"`。要求 Node >= 22。 |
| `crates/linkrs-grammar/Cargo.toml` | 声明 `build = "build.rs"`，依赖 `tree-sitter` + `tree-sitter-language`（workspace），build-dep `cc`（workspace）。 |
| `scripts/verify-grammar.sh` | **手动同步验证脚本。** 运行 `cargo run -p linkrs-query --example export_keyword_inventory`，断言 `keywords.json` 的 `git diff --exit-code`；然后运行 `node tools/grammar-codegen/gen.mjs`，断言 `frontend/src/lib/utils/generated/` 的 `git diff --exit-code`。这是保持生成产物同步的人工门禁。 |
| `tools/grammar-codegen/gen.mjs` | **前端 TypeScript 代码生成。** 读取 `data/keywords.json`，写入 `frontend/src/lib/utils/generated/keywords.ts` 和 `functions.ts`（Monarch 兼容数组）。 |
| `tools/grammar-codegen/package.json` | 上述脚本的 Node 包；脚本 `"gen": "node gen.mjs"`。 |
| `frontend/package.json` | 暴露 `"gen:grammar": "npm run gen --prefix ../tools/grammar-codegen"`。 |

### 2.3 依赖引用

| 路径 | 引用 |
|------|------|
| `Cargo.toml`（根） | Workspace 依赖：`tree-sitter = "0.27.1"`、`tree-sitter-language = "0.1.9"`、`cc = "1.2"`。Workspace 成员 `crates/linkrs-grammar`。 |
| `crates/linkrs-grammar/Cargo.toml` | `tree-sitter.workspace = true`、`tree-sitter-language.workspace = true`、`cc.workspace = true`（build-dep）。 |
| `crates/linkrs-grammar/package.json` | `tree-sitter-cli 0.27.1`（dev-dep，仅生成用）。 |

**版本三元组：** CLI 0.27.1 / ABI 15 / 运行时 0.27.1。

## 3. 生成管线详解

### 3.1 Phase A — 一次性生成（开发者机器）

```
┌─────────────────────────────────────────────────────────────────┐
│ Step 1: 导出词法清单                                             │
│                                                                 │
│   cargo run -p linkrs-query --example export_keyword_inventory   │
│                                                                 │
│   读取 Rust lexer 的 lookup_keyword + 函数注册表                  │
│   序列化到 crates/linkrs-grammar/data/keywords.json              │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 2: 语法定义加载清单                                         │
│                                                                 │
│   grammar.js:  const inv = require('./data/keywords.json');      │
│                                                                 │
│   对每个 keyword，程序化生成 kw_* 正则规则：                       │
│   - 大小写不敏感：通过逐字母字符类 [aA][bB]... 实现               │
│   - 多词关键字（含空格）通过 seq(...) 处理                        │
│   - TRUE/FALSE/BALANCE 被跳过（非独立关键字 token）               │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 3: 运行生成器                                               │
│                                                                 │
│   npm run generate  (即 tree-sitter generate, CLI 0.27.1)        │
│                                                                 │
│   展开 grammar.js → 三个已提交产物：                              │
│   - src/parser.c          (~45k 行 C 代码)                       │
│   - src/node-types.json   (节点类型元数据)                        │
│   - src/grammar.json      (展开后的语法 JSON)                     │
│                                                                 │
│   必须运行直到没有未注册的冲突为止                                 │
└─────────────────────────────────────────────────────────────────┘
```

**关键点：** `linkrs-query` 是词法的真相源，而非反过来。关键字变更起源于 Rust lexer，语法从它重新生成。

### 3.2 Phase B — 构建/部署（离线）

```
┌─────────────────────────────────────────────────────────────────┐
│ Step 1: build.rs 验证产物同步性                                  │
│                                                                 │
│   assert parser.c 包含 "#define LANGUAGE_VERSION 15"             │
│   assert grammar.json 包含 "\"name\": \"linkrs\""                │
│   assert node-types.json 存在                                    │
│   assert keywords.json 包含 "\"version\": 1"                     │
│                                                                 │
│   任一断言失败 → 编译快速失败（防止部分重新生成导致的静默漂移）     │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 2: cc 编译 parser.c                                         │
│                                                                 │
│   cc::Build::new()                                               │
│       .include(src)        // 使 tree_sitter/parser.h 可见       │
│       .file(src/parser.c)                                        │
│       .compile("tree-sitter-linkrs");                            │
│                                                                 │
│   产出静态库，C 编译开销和体积仅落在 CLI/工具侧                    │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 3: lib.rs 绑定                                              │
│                                                                 │
│   unsafe extern "C" { fn tree_sitter_linkrs() -> *const (); }   │
│                                                                 │
│   Language::new(                                                 │
│       tree_sitter_language::LanguageFn::from_raw(               │
│           tree_sitter_linkrs                                     │
│       )                                                          │
│   )                                                              │
│                                                                 │
│   消费者调用 language() 获取句柄，然后 parse()/tokens()           │
│   产出语法树和分类的 token span                                   │
│   高亮查询在运行时从 queries/highlights.scm 加载                  │
└─────────────────────────────────────────────────────────────────┘
```

## 4. crate 结构

```
crates/linkrs-grammar/
├── build.rs              # cc 编译 + 产物同步验证
├── Cargo.toml            # deps: tree-sitter, tree-sitter-language; build-dep: cc
├── package.json          # 固定 tree-sitter-cli 0.27.1; generate/verify 脚本
├── grammar.js            # 手写语法（require data/keywords.json）
├── data/
│   └── keywords.json     # 已提交词法清单（由 linkrs-query 生成）
├── queries/
│   └── highlights.scm    # 高亮查询（基于 capture）
├── src/
│   ├── lib.rs            # Rust API: language(), parse(), tokens(), inventory_json()
│   ├── parser.c          # 生成的 C 解析器（已提交）
│   ├── node-types.json   # 生成的节点元数据（已提交）
│   ├── grammar.json      # 生成的展开语法（已提交）
│   └── tree_sitter/
│       ├── parser.h      # vendored 运行时头文件
│       ├── alloc.h       # vendored 运行时头文件
│       └── array.h       # vendored 运行时头文件
└── tests/
    └── smoke.rs          # 高亮加载、解析冒烟、token 快照、运算符覆盖
```

该 crate **自包含**：生产依赖仅 `tree-sitter` + `tree-sitter-language`；不依赖任何 linkrs crate。

## 5. 同步验证机制

### 5.1 编译时验证（build.rs）

`build.rs` 在编译时断言已提交的 `parser.c`/`grammar.json`/`node-types.json`/`keywords.json` 内部一致（正确的 `LANGUAGE_VERSION`、正确的语法名称、文件存在）。这确保部分重新生成会在编译时快速失败，而非在运行时表现为静默漂移。

### 5.2 手动/CI 验证（verify-grammar.sh）

```sh
#!/usr/bin/env sh
set -eu
root=$(dirname "$0")/..

# 1. 重新导出词法清单，断言无漂移
cargo run -q -p linkrs-query --example export_keyword_inventory
git diff --exit-code -- crates/linkrs-grammar/data/keywords.json

# 2. 重新生成前端 TS 表，断言无漂移
node tools/grammar-codegen/gen.mjs
git diff --exit-code -- frontend/src/lib/utils/generated/

echo "grammar inventory in sync"
```

这是保持生成产物同步的人工/CI 门禁。

## 6. 版本更新记录

### 6.1 tree-sitter 0.26.9 → 0.27.1 升级

**更新内容：**
- `tree-sitter` 运行时 crate：0.26.9 → 0.27.1
- `tree-sitter-language` crate：0.1.7 → 0.1.9
- `tree-sitter-cli`：0.27.1（已固定）

**API 变更：**
- `QueryMatch` 的 `captures` 字段从公开字段变为私有，需使用 `captures()` 方法

**修复：**
- `crates/linkrs-grammar/src/lib.rs`：`m.captures` → `m.captures()`

### 6.2 完成 arrow_right/hash_arrow/hash_arrow_right 重新生成

**问题：** `grammar.js` 定义了 `arrow_right`/`hash_arrow`/`hash_arrow_right` 规则，但未被任何规则引用，tree-sitter 优化掉了未使用规则。

**修复：**
- `edge_pattern`：添加 `arrow_right`/`hash_arrow`/`hash_arrow_right` 到 choice
- `value_entry`：添加 `arrow_right` 到边箭头 choice
- `delete_segment`：添加 `arrow_right` 到边箭头 choice
- `highlights.scm`：添加新箭头类型到 `@operator` 列表

**验证：**
- `parser.c` 中 `arrow_right`/`hash_arrow` 出现 47 处
- `node-types.json` 中出现 8 处
- 所有 4 个冒烟测试通过

## 7. 设计决策总结

| 决策 | 理由 |
|------|------|
| 生成产物提交到仓库 | 部署环境无需 tree-sitter CLI 或网络 |
| 固定 CLI 版本 0.27.1 | 确保生成产物可复现 |
| 词法清单由 Rust lexer 导出 | 单一真相源，避免关键字定义漂移 |
| 语法 crate 不依赖其他 linkrs crate | 隔离性，C 编译开销仅落在工具侧 |
| 5 组声明式冲突 | 处理 SHOW/DELETE/MERGE/ORDER BY/UPDATE 的裸前导关键字歧义 |
| 大小写不敏感通过字符类实现 | 避免正则标志依赖，兼容 tree-sitter 的 DFA 生成 |
