# GPUI 客户端：依赖决策与构建方法

`linkrs-gpui`（`crates/linkrs-gpui`）是基于 GPUI 的原生桌面客户端。
本文记录它与 `gpui-kit` 的依赖接入决策、独立 workspace 的原因，
以及两套 workspace 各自的构建方法。设计文档中的阶段规划见
`docs/plan/gpui_client_design.md` 与
`docs/plan/gpui_client_implementation_plan.md`（其中 §1.1“git 依赖”
决策已被 §11 修订记录废止，以本文为准）。

## 1. 依赖关系

- `linkrs-gpui` 依赖 `crates/gpui-kit`（fork `kkkqkx123/gpui-kit` 的
  submodule，只读引用）中的 umbrella crate `gpui-kit`，
  外加 `../linkrs-client`、`../linkrs-wire` 两个站内 path 依赖。
- `gpui-kit` 自带完整 workspace（`crates/gpui-kit/Cargo.toml` 含
  `[workspace]`），其成员大量使用 `workspace = true` 继承，
  私有依赖表含 `gpui = gpui-pre`（精确 pin）、edition 2024、
  `[workspace.lints]` 等条目。
- 当前实际启用的特性：`component`、`assets`、`syntect`、`tree-sitter-sql`。

## 2. 设计决策

### D1：gpui-kit 只用本地 submodule 引用，不用 crates.io / git 依赖

- submodule 工作树必须保持干净；在树补丁能力保留给将来
  （框架能力变更先推 fork，再更新指针），且与 tantivy 等子模块的
  引用方式一致。
- crates.io 官方版本不可用：`gpui-component` 与站内 `linkrs-grammar`
  对 `tree-sitter` 的大版本要求冲突（见 D3），且官方版本滞后于 fork
  分支，特性与 API 均可能对不上。
- git 依赖虽然技术上可行，但会让每次本地构建都重新触碰远端，
  违背“本地 vendor 仓库自给自足”原则，故不采用。

### D2：`linkrs-gpui` 是独立 workspace，不是根 workspace 成员

根 `Cargo.toml` 的 `members` 有意不含 `crates/linkrs-gpui`，
该 crate 用自己的 `[workspace]` 自成一体。两个独立 blocker：

1. **嵌套 workspace 继承规则**。经最小探针验证：workspace 目录树内的
   path 依赖（含 `[patch]` path），其 `workspace = true` 继承一律按
   消费方外层 root 解析，内层 `[workspace]` 被忽略；`exclude` 在
   root-package workspace 下不改变该行为。因此从根 workspace 用 path
   指入 `crates/gpui-kit` 会在 `gpui` 条目上解析失败，
   并导致整个根 workspace 无法加载（连 `cargo check -p linkrs-wire`
   都失败）。把 gpui-kit 私有依赖表镜像进根 workspace 被设计否决
   （键冲突、版本 pin 冲突、edition 冲突）。
2. **`links` 冲突**。`gpui-component 0.8.1` 要求 `tree-sitter ^0.26`，
   `linkrs-grammar` 要求 `tree-sitter ^0.27`，两者都声明
   `links = "tree-sitter"`，单个 lockfile 不可能同时容纳。
   这意味着即使继承问题绕过去，GUI 也不可能留在根 workspace 里。

拆分后目录归属自然解耦：从 `crates/linkrs-gpui` 看过去，
`../gpui-kit` 在其 workspace 目录之外，继承按 gpui-kit 自身 root 解析；
`../linkrs-client`、`../linkrs-wire` 落在 linkrs 根内，继承继续按根解析。

### D3：与站内其他嵌套 workspace 的处置对照

| 子模块 | 处置方式 | 适用条件 |
|---|---|---|
| `tantivy` | `[patch.crates-io]` + 本地 path | 被 patch 的根 manifest **零继承**（全显式版本），无继承可误解析 |
| `llm-suite` | 根 `[workspace.dependencies]` 扁平声明内部引用 | 规模小、edition 与根一致（2021）、外部键在根已存在；根注释写明了本规则 |
| `gpui-kit` | 消费方独立 workspace + 本地 path | 私有依赖表无法镜像 + `links` 冲突，只能隔离解析上下文 |

新增嵌套 workspace 依赖时，按上表从上往下选第一种满足条件的方案。

### D4：双 lockfile 与版本同步

- 根 `Cargo.lock` 与 `crates/linkrs-gpui/Cargo.lock` 独立演进，
  前者不含任何 gpui 条目，后者不含任何 git 源（本地构建零远端获取）。
- `crates/linkrs-gpui` 的 `[workspace.dependencies]` 是根对应项的人工镜像
  （anyhow/log/serde/serde_json/tokio/futures/csv），升级其中一处时
  检查另一处是否需要同步。

## 3. 构建方法

前置（两种构建都需要，先做一次）：

```shell
git submodule update --init crates/gpui-kit
```

服务端 / 根 workspace（不受 GUI 拆分影响，命令不变）：

```shell
cargo check -p linkrs-wire
cargo check -p linkrs-server --features vector,fulltext
cargo test --workspace
```

GUI 客户端（独立 workspace，根命令覆盖不到）：

```shell
cd crates/linkrs-gpui && cargo build
# 或在仓库根执行：
cargo build --manifest-path crates/linkrs-gpui/Cargo.toml
cargo test --manifest-path crates/linkrs-gpui/Cargo.toml
cargo run --manifest-path crates/linkrs-gpui/Cargo.toml
```

CI 中根的 `cargo build/test --workspace` 不再覆盖 GUI，必须单独加一步
构建 `crates/linkrs-gpui`。

更新 `gpui-kit` 指针的流程：更新 submodule 提交 → 在 `crates/linkrs-gpui`
内 `cargo check`（验证继承与 API）→ 在仓库根 `cargo check -p linkrs-wire`
（验证根解析未受牵连）→ 提交两个 lockfile 的变更。

## 4. 常见错误速查

- `error inheriting 'gpui' from workspace root manifest`：
  某处又把 gpui-kit 用 path 接进了根 workspace。检查根 `members`
  是否误加回 `crates/linkrs-gpui`，或新增了直连 gpui-kit 的成员。
- `failed to select a version for 'tree-sitter' ... links`：
  同上，GUI 依赖混入了根解析图。修法同上，不要试图统一两个大版本。
- `failed to load manifest ... crates/gpui-kit ... No such file`：
  submodule 未初始化，执行前置命令。
- GUI 构建出现 git 拉取：`crates/linkrs-gpui/Cargo.lock` 应无 git 源
  （`grep -c 'source = "git+'` 为 0）；若出现，说明有人引入了 git 依赖，
  按 D1 改回本地引用。
