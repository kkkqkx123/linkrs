# Tree-Sitter 语法生成逻辑分析

## 1. 架构总览

Linkrs 的 tree-sitter 集成被隔离在单一叶子 crate `crates/linkrs-grammar` 中，位于依赖 DAG 底部，不依赖任何其他 linkrs crate。后端执行管线（`linkrs-query`、`linkrs-server`、根二进制）**不链接 tree-sitter**——只有 CLI 和工具链消费它。

核心设计原则：**语法由固定版本的 CLI 一次性生成，生成的产物提交到仓库，构建时仅用 `cc` 编译 C 源码**（无网络、无 CLI 依赖）。同一份 `parser.c` 同时产出原生静态库与前端 wasm 语法模块：前者随 Rust 侧编译入库，后者是前端构建时从同一份 `parser.c` 现场编译的产物。

```
┌─────────────────────────────────────────────────────────────────────┐
│  Phase A: 一次性生成（开发者机器，需要 Node + tree-sitter CLI）        │
│                                                                     │
│  lexing/spec.rs ──export──> keywords.json ──require──> grammar.js    │
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
              ┌───────────────┴───────────────┐
              ▼                               ▼
┌───────────────────────────┐  ┌────────────────────────────────────┐
│  Phase B: Rust 构建        │  │  Phase C: 前端构建                  │
│  (离线，无 CLI)            │  │  (需要 tree-sitter CLI + wasi-sdk)   │
│                           │  │                                    │
│  build.rs 验证产物同步性   │  │  vite 插件调用                      │
│  ──> cc 编译 parser.c     │  │  tree-sitter build --wasm           │
│  ──> lib.rs 绑定          │  │  ──> src/lib/assets/                │
│                           │  │       tree-sitter-linkrs.wasm       │
│  消费者: language()        │  │       (构建产物，不入库)            │
│  → parse()/tokens()       │  │                                    │
│                           │  │  以 ?url 导入 → 打包器输出为         │
│                           │  │  带哈希的静态资源                     │
└───────────────────────────┘  └────────────────────────────────────┘
```

两条链路消费的是同一份已提交的 `parser.c`，因此前端高亮用的语法与后端解析用的语法天然一致，不存在版本漂移。

后端执行管线（`linkrs-query`、`linkrs-server`、根二进制）**不链接 tree-sitter**：`linkrs-query` 只在 dev-dependency 中使用语法 crate，`linkrs-cli` 关闭 `parser` feature 后只读取词法清单，运行时本身仍然不进入任何服务端二进制。

## 2. 文件清单与职责

### 2.1 语法定义与生成产物

| 路径 | 职责 |
|------|------|
| `crates/linkrs-grammar/grammar.js` | **手写语法定义**（780 行）。Linkrs 方言语法树的唯一真相源。加载时 `require` `data/keywords.json` 以程序化生成关键字规则（保留字 `kw_*` 规则与 `soft_keywords` 软关键字规则各一组）。定义 `name: 'linkrs'`、extras、word 规则、5 组声明的冲突、完整规则集。 |
| `crates/linkrs-grammar/tree-sitter.json` | **CLI 语法清单。** 声明语法名、作用域、文件类型与高亮查询位置，并提供生成器所需的元数据。缺少该文件时 CLI 会把 ABI 降级到 14，因此它必须入库。`metadata.version` 固定为 `0.0.0`，使重新生成得到与已提交 `parser.c` 完全一致的结果。 |
| `crates/linkrs-grammar/abi-version` | **语言 ABI 声明。** 内容为 `15`。生成脚本据此传 `--abi`，`build.rs` 据此断言 `parser.c` 与 wasm 指纹，两侧共用同一个来源。 |
| `crates/linkrs-grammar/src/parser.c` | **生成的 C 解析器**（~93k 行）。由 `tree-sitter-cli` 一次性产出。包含 `tree_sitter_linkrs()` 入口，`LANGUAGE_VERSION 15`。由 `build.rs` 通过 `cc` 编译。 |
| `crates/linkrs-grammar/src/node-types.json` | **生成的节点类型元数据**。供工具/编辑器内省节点。必须与 `parser.c` 一起提交。 |
| `crates/linkrs-grammar/src/grammar.json` | **生成的语法 JSON**（`grammar.js` 的展开、解析后形式）。记录 `"name": "linkrs"`——由 `build.rs` 验证。 |
| `crates/linkrs-grammar/src/tree_sitter/parser.h` | Tree-sitter 运行时头文件（vendored）。属于**运行时版本**，不由生成脚本刷新。 |
| `crates/linkrs-grammar/src/tree_sitter/alloc.h` | Tree-sitter 运行时头文件（vendored）。属于**运行时版本**，不由生成脚本刷新。 |
| `crates/linkrs-grammar/src/tree_sitter/array.h` | Tree-sitter 运行时头文件（vendored）。属于**运行时版本**，不由生成脚本刷新。 |
| `crates/linkrs-grammar/src/lib.rs` | **Rust 绑定层**。暴露 `language()`、`parse()`、`tokens()`、`inventory_json()`。声明 `extern "C" { fn tree_sitter_linkrs() -> *const (); }` 并通过 `tree_sitter_language::LanguageFn::from_raw` 包装为 `tree_sitter::Language`。通过 `include_str!` 嵌入 `highlights.scm` 和 `keywords.json`。除 `inventory_json()` 外全部由 `parser` feature 门控。 |
| `crates/linkrs-grammar/data/keywords.json` | **词法清单快照**（已提交）。233 个关键字条目（保留字 + `BALANCE`/`TRUE`/`FALSE` payload 拼写）、21 个软关键字、25 个运算符、20 个标点、280 个函数、多词序列。由 `linkrs-query` 从词法规格 `lexing/spec.rs` 生成（见下文）。`grammar.js` 在生成时读取此文件以产出 `kw_*` 与软关键字规则。 |
| `crates/linkrs-grammar/queries/highlights.scm` | **高亮查询**（231 行）。基于 capture 的分类（`@keyword`、`@operator`、`@number`、`@string`、`@variable`、`@property`、`@function`、`@comment`）。运行时由 `lib.rs` 加载，由冒烟测试验证。 |

### 2.2 构建脚本与生成脚本

| 路径 | 职责 |
|------|------|
| `crates/linkrs-grammar/build.rs` | **编译生成的解析器。** 验证已提交产物处于同步状态（断言 `parser.c` 的 `LANGUAGE_VERSION` 与 `abi-version` 一致，`grammar.json` 包含 `"name": "linkrs"`，`node-types.json` 存在，`keywords.json` 有 `"version": 1`），然后在 `parser` feature 打开时运行 `cc::Build::new().include(src).file(src/parser.c).compile("tree-sitter-linkrs")`。对所有输入发出 `cargo:rerun-if-changed`。**不调用 tree-sitter CLI。** |
| `crates/linkrs-grammar/package.json` | **固定生成工具链。** `devDependencies: tree-sitter-cli 0.27.1`。脚本：`"generate": "node scripts/generate.mjs"`，`"verify": "node --check grammar.js"`。要求 Node >= 22。 |
| `crates/linkrs-grammar/scripts/generate.mjs` | **生成脚本。** 在临时目录中调用 `tree-sitter generate --abi <abi-version>`，只把 `parser.c`、`grammar.json`、`node-types.json` 复制回 `src/`。运行时头文件不被覆盖，因为它们属于运行时版本而不是 CLI 版本。 |
| `crates/linkrs-grammar/Cargo.toml` | 声明 `build = "build.rs"`。`tree-sitter` + `tree-sitter-language` 为可选依赖，由默认开启的 `parser` feature 携带；build-dep `cc`（workspace）。 |
| `frontend/vite/grammarWasm.ts` | **前端 wasm 构建插件。** 在 `buildStart` 阶段调用 `tree-sitter build --wasm`，把 side module 写入 `frontend/src/lib/assets/tree-sitter-linkrs.wasm`。CLI 优先取语法 crate 中固定的版本，其次取 PATH 上的版本。产物缺失时构建直接失败，不会退化成运行时的静默 404。 |
| `frontend/.gitignore` | 排除 `src/lib/assets/tree-sitter-linkrs.wasm`：它是构建产物，与 `node_modules`、`.svelte-kit` 同级。 |
| `scripts/verify-grammar.sh` | **同步验证脚本。** 重新导出词法清单并断言 `keywords.json` 无漂移；重新生成前端 TS 表并断言无漂移；重放高亮 fixture 并断言 `outputs/` 无漂移；运行 `check-highlights.py`、`check-grammar-coverage.py`、`check-grammar-structure.py` 三个一致性脚本；运行语法 crate 冒烟测试与 `linkrs-query` 语料对等测试；在临时目录重新生成 `parser.c`/`grammar.json`/`node-types.json` 并逐字节比对；编译一次 wasm 并确认它导出语法入口。 |
| `tools/grammar-codegen/gen.mjs` | **前端 TypeScript 代码生成。** 读取 `data/keywords.json`，写入 `frontend/src/lib/utils/generated/keywords.ts`、`operators.ts` 和 `functions.ts`（Monarch 兼容数组）。 |
| `tools/grammar-codegen/package.json` | 上述脚本的 Node 包；脚本 `"gen": "node gen.mjs"`。 |
| `frontend/package.json` | 暴露 `"gen:grammar": "npm run gen --prefix ../tools/grammar-codegen"`；运行时依赖 `web-tree-sitter`。 |
| `frontend/src/lib/utils/treeSitterCypher.ts` | **前端高亮适配层。** 以 `?url` 导入 wasm、以 `?raw` 导入高亮查询，注册 Monaco 语义 token provider。 |

### 2.3 依赖引用

| 路径 | 引用 |
|------|------|
| `Cargo.toml`（根） | Workspace 依赖：`tree-sitter = "0.27.1"`、`tree-sitter-language = "0.1.9"`、`cc = "1.2"`、`sha2 = "0.10"`。Workspace 成员 `crates/linkrs-grammar`。 |
| `crates/linkrs-grammar/Cargo.toml` | `tree-sitter`/`tree-sitter-language` 为可选依赖，由 `parser` feature 携带；build-dep `cc`（workspace）。 |
| `crates/linkrs-grammar/package.json` | `tree-sitter-cli 0.27.1`（dev-dep，仅生成用）。 |
| `crates/linkrs-cli/Cargo.toml` | `linkrs-grammar` 以 `default-features = false` 依赖，只读取词法清单，不链接 tree-sitter 运行时。 |
| `crates/linkrs-query/Cargo.toml` | `linkrs-grammar` 仅作为 dev-dependency，用于语料对等测试。 |

**版本三元组：** CLI 0.27.1 / ABI 15 / 运行时 0.27.1。ABI 由 `abi-version` 声明，运行时版本由 `tree-sitter` crate 决定，生成器版本由 `package.json` 固定。

## 3. 生成管线详解

### 3.1 Phase A — 一次性生成（开发者机器）

```
┌─────────────────────────────────────────────────────────────────┐
│ Step 1: 导出词法清单                                             │
│                                                                 │
│   cargo run -p linkrs-query --example export_keyword_inventory   │
│                                                                 │
│   读取词法规格 lexing/spec.rs（关键字表、符号表、软关键字、       │
│   多词序列）+ 函数注册表                                         │
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
│   - soft_keywords 生成软关键字规则（供补全/高亮，非保留规则）      │
│   - TRUE/FALSE/BALANCE 被跳过（非独立关键字 token）               │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 3: 运行生成器                                               │
│                                                                 │
│   npm run generate  (node scripts/generate.mjs)                   │
│                                                                 │
│   在临时目录执行 tree-sitter generate --abi $(cat abi-version)   │
│   展开 grammar.js → 三个已提交产物：                              │
│   - src/parser.c          (~93k 行 C 代码)                       │
│   - src/node-types.json   (节点类型元数据)                        │
│   - src/grammar.json      (展开后的语法 JSON)                     │
│                                                                 │
│   只把上述三个文件复制回 src/，运行时头文件保持原样               │
│   必须运行直到没有未注册的冲突为止                                 │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────────┐
│ Step 4: 前端构建时编译 wasm 语法模块                              │
│                                                                 │
│   前端 vite 插件在 buildStart 阶段执行：                          │
│   tree-sitter build --wasm -o <frontend>/src/lib/assets/…       │
│                                                                 │
│   - 生成 side module（dylink.0），导出 tree_sitter_linkrs         │
│   - 输入是已提交的 src/parser.c，与 Rust 侧同一份源码             │
│   - 产物不入库，随前端构建产生，随前端构建消失                    │
└─────────────────────────────────────────────────────────────────┘
```

**关键点：** `linkrs-query` 的词法规格（`lexing/spec.rs`）是词法的真相源，而非反过来。关键字变更起源于规格表，语法从它重新生成。

### 3.2 Phase B — 构建/部署（离线）

```
┌─────────────────────────────────────────────────────────────────┐
│ Step 1: build.rs 验证产物同步性                                  │
│                                                                 │
│   abi = 读取 abi-version                                        │
│   assert parser.c 包含 "#define LANGUAGE_VERSION <abi>"           │
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
│   仅在 parser feature 打开时执行：                                │
│   cc::Build::new()                                               │
│       .include(src)        // 使 tree_sitter/parser.h 可见       │
│       .file(src/parser.c)                                        │
│       .compile("tree-sitter-linkrs");                            │
│                                                                 │
│   产出静态库，C 编译开销和体积仅落在工具侧                          │
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

### 3.3 Phase C — 前端消费

前端以 Vite 资源导入的方式拿到 wasm，不依赖任何固定路径约定：

```ts
import grammarWasmUrl from '../assets/tree-sitter-linkrs.wasm?url';
```

wasm 位于 `frontend/src/lib/assets/`，**不是** SvelteKit 的静态目录，因此必须用 `?url` 导入：打包器会把它输出为带内容哈希的静态资源（`_app/immutable/assets/tree-sitter-linkrs.<hash>.wasm`）。这样带来两个性质：

- 产物缺失时**构建直接失败**，而不是运行时才发现 404；
- 产物路径由打包器决定，语法模块改名或移动都不会造成引用失效。

wasm 由 `frontend/vite/grammarWasm.ts` 在 `buildStart` 阶段编译，源码是语法 crate 已提交的 `src/parser.c`。因此前端高亮用的语法与后端解析用的语法是同一份源码编译出来的两个目标，不存在版本漂移的可能，也不需要额外的指纹机制。

高亮查询以 `?raw` 导入 `crates/linkrs-grammar/queries/highlights.scm`，Rust 侧则用 `include_str!` 读取同一份文件，两端共用一套 capture 词表；该文件是共享源码而非构建产物，因此仍留在语法 crate。

`web-tree-sitter` 的 `Language.load()` 接受 URL 字符串，浏览器端走 `fetch` + `WebAssembly.compileStreaming`；若静态服务器未按 `.wasm` 后缀给出 `application/wasm`，运行时会退回 `arrayBuffer` 方式加载。

## 4. crate 结构

```
crates/linkrs-grammar/
├── build.rs              # cc 编译 + 产物同步验证
├── Cargo.toml            # deps: tree-sitter, tree-sitter-language（可选，parser feature）; build-dep: cc
├── package.json          # 固定 tree-sitter-cli 0.27.1; generate/verify 脚本
├── grammar.js            # 手写语法（require data/keywords.json）
├── tree-sitter.json      # CLI 语法清单（生成器必需，决定 ABI 不降级）
├── abi-version           # 语言 ABI 声明（15）
├── data/
│   └── keywords.json     # 已提交词法清单（由 linkrs-query 从词法规格导出）
├── examples/
│   ├── highlight.rs      # 单条查询的高亮输出调试工具
│   └── highlight_fixtures.rs  # 重放语料 fixture 到 outputs/（verify 脚本比对用）
├── outputs/
│   └── *.txt             # 高亮 fixture 期望输出（已提交，防漂移）
├── queries/
│   └── highlights.scm    # 高亮查询（基于 capture）
├── scripts/
│   └── generate.mjs      # 生成脚本（临时目录 + 只回写语法产物）
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

该 crate **自包含**：不依赖任何 linkrs crate。`parser` feature 默认开启并携带 tree-sitter 运行时；只读取词法清单的消费者关闭该 feature，因此不会把运行时链接进二进制。crate 内不再保留 wasm 产物，前端构建时直接从 `src/parser.c` 编译。

## 5. 同步验证机制

### 5.1 编译时验证（build.rs）

`build.rs` 在编译时断言已提交的 `parser.c`/`grammar.json`/`node-types.json`/`keywords.json` 内部一致（正确的 `LANGUAGE_VERSION`、正确的语法名称、文件存在）。这确保部分重新生成会在编译时快速失败，而非在运行时表现为静默漂移。

前端语法模块不入库，因此没有需要校验的产物副本：它由前端构建从同一份 `parser.c` 现场编译，天然与后端一致。工具链不可用时前端构建直接失败并给出安装提示。

### 5.2 手动/CI 验证（verify-grammar.sh）

```sh
#!/usr/bin/env sh
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
cd "$root"

# 1. 重新导出词法清单，断言无漂移
cargo run -q -p linkrs-query --example export_keyword_inventory
git diff --exit-code -- crates/linkrs-grammar/data/keywords.json

# 2. 重新生成前端 TS 表，断言无漂移
node tools/grammar-codegen/gen.mjs
git diff --exit-code -- frontend/src/lib/utils/generated/

# 3. 重放高亮 fixture，断言 outputs/ 无漂移；运行一致性脚本与测试
cargo run -q -p linkrs-grammar --example highlight_fixtures
git diff --exit-code -- crates/linkrs-grammar/outputs/
python3 scripts/check-highlights.py
python3 scripts/check-grammar-coverage.py
python3 scripts/check-grammar-structure.py
cargo test -q -p linkrs-grammar
cargo test -q -p linkrs-query --test corpus_parity

# 4. 在临时目录重新生成语法产物，逐字节比对
# 5. 编译一次 wasm 并确认它导出语法入口
```

脚本全程不修改工作区：语法产物在临时目录生成（fixture 重放除外，其期望输出本就入库，无漂移时 diff 为空），门禁只报告差异而不留下脏文件。

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

### 6.3 前端 wasm 语法模块接入

**内容：**
- 前端引入 `web-tree-sitter`，Monaco 语义高亮改由语法驱动。
- 新增 `frontend/vite/grammarWasm.ts`：在 `buildStart` 阶段把 `crates/linkrs-grammar/src/parser.c` 编译成 wasm side module，输出到 `frontend/src/lib/assets/tree-sitter-linkrs.wasm`，该文件由 `.gitignore` 排除。
- 新增 `crates/linkrs-grammar/scripts/generate.mjs`，`package.json` 的 `generate` 脚本改为调用它。
- 新增 `tree-sitter.json` 与 `abi-version`，把 ABI 目标显式写进仓库。
- `scripts/verify-grammar.sh` 增加语法产物的逐字节比对与 wasm 编译检查。
- 语法 crate 拆分 `parser` feature：只读词法清单的消费者不再链接 tree-sitter 运行时。
- 前端以 `?url` 导入 wasm，语法模块由打包器输出为带哈希的静态资源。

**工具链约束：**
- wasm 编译需要 wasi-sdk，CLI 会缓存到 `~/.cache/tree-sitter/wasi-sdk`，首次使用需要网络。
- 不同 CLI 版本生成的 `src/tree_sitter/*.h` 存在差异，这些头文件属于运行时版本，因此生成脚本不再覆盖它们；语法产物本身在 CLI 0.26.9 与 0.27.1 之间逐字节一致。
- `tree-sitter.json` 缺失时 CLI 会把 ABI 降级到 14，因此该文件必须入库。

## 7. 设计决策总结

| 决策 | 理由 |
|------|------|
| 生成产物提交到仓库 | 部署环境无需 tree-sitter CLI 或网络 |
| 固定 CLI 版本 0.27.1 | 确保生成产物可复现 |
| 词法清单由 Rust 词法规格导出 | 单一真相源，避免关键字定义漂移 |
| 语法 crate 不依赖其他 linkrs crate | 隔离性，C 编译开销仅落在工具侧 |
| `parser` feature 隔离运行时 | 只读清单的消费者不链接 tree-sitter 运行时 |
| 5 组声明式冲突 | 处理 SHOW/DELETE/MERGE/ORDER BY/UPDATE 的裸前导关键字歧义 |
| 大小写不敏感通过字符类实现 | 避免正则标志依赖，兼容 tree-sitter 的 DFA 生成 |
| ABI 由 `abi-version` 声明 | 生成脚本与 `build.rs` 共用同一来源，避免隐式默认值 |
| wasm 是前端构建产物，不入库 | 它是编译输出而非源码；不入库避免仓库携带二进制与重复真相 |
| wasm 与 parser.c 同源编译 | 前端高亮与后端解析必然一致，无需额外的指纹机制 |
| wasm 放非静态目录并以 `?url` 导入 | 缺失时构建失败，而不是运行时的静默 404 |
| 运行时头文件不随生成刷新 | 头文件属于运行时版本，与 CLI 版本无关 |
| 高亮查询留在语法 crate | 它是 Rust 与前端共享的源码，不是构建产物 |
