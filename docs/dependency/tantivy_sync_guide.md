# Tantivy 上游同步指南

## 目标

本仓库把 `crates/tantivy` 作为 submodule 使用。根仓库通过 `[patch.crates-io]` 将 `tantivy` 指向本地 submodule，因此同步上游时不要改动 `crates/tantivy/Cargo.toml` 的 workspace 结构，也不要把上游 workspace 扁平化到根仓库。

本指南只描述两件事：

1. 如何把上游 `tantivy` 的修改同步到本地分支。
2. 冲突出现时，如何让后续同步尽量自动化。

## 推荐配置

先给 submodule 增加官方上游远程：

```bash
git -C crates/tantivy remote add upstream https://github.com/quickwit-oss/tantivy.git
```

然后给当前功能分支配置跟踪关系。当前分支名以仓库实际使用的 `feat/add-configurable-k1-b` 为例：

```bash
git -C crates/tantivy config branch.feat/add-configurable-k1-b.remote upstream
git -C crates/tantivy config branch.feat/add-configurable-k1-b.merge refs/heads/main
```

建议同时打开这些选项：

```bash
git -C crates/tantivy config rerere.enabled true
git -C crates/tantivy config rerere.autoupdate true
git -C crates/tantivy config merge.conflictstyle zdiff3
```

如果你希望这个 fork 以功能分支作为默认入口，可以在第一次推送后把本地分支指向 fork 远程：

```bash
git -C crates/tantivy config branch.feat/add-configurable-k1-b.remote origin
git -C crates/tantivy config branch.feat/add-configurable-k1-b.merge refs/heads/feat/add-configurable-k1-b
```

可选地，在根仓库打开 submodule 递归处理：

```bash
git config submodule.recurse true
git config submodule.crates/tantivy.update merge
```

## 同步流程

建议用脚本执行，脚本见 [scripts/sync_tantivy.sh](../../scripts/sync_tantivy.sh)。

手工流程如下：

```bash
git -C crates/tantivy fetch upstream
git -C crates/tantivy checkout feat/add-configurable-k1-b
git -C crates/tantivy merge --no-edit upstream/main
```

如果合并成功，再执行根仓库验证：

```bash
cargo check --workspace --features server,fulltext-search,grpc,qdrant
```

然后把功能分支推到 fork 远程。推送时使用本地代理，避免网络波动：

```bash
export https_proxy="http://localhost:7890"
export http_proxy="http://localhost:7890"
git -C crates/tantivy push -u origin feat/add-configurable-k1-b
```

如果你已经在 GitHub 网站上把 fork 的默认分支切到 `feat/add-configurable-k1-b`，本地可以同步远程默认分支指针：

```bash
git -C crates/tantivy remote set-head origin -a
```

默认分支切换本身需要在 GitHub 仓库的 Settings 页面手动完成，本地 Git 只能在切换后刷新指针。

最后把 submodule 指针更新回根仓库：

```bash
git add crates/tantivy
git commit -m "chore: bump tantivy submodule"
```

## 冲突处理

优先级建议如下：

1. 如果冲突在与本地 BM25 相关的代码里，先保留本地 `k1/b` 逻辑，再把上游改动重新套回去。
2. 如果冲突在和本地功能无关的上游文件里，优先采用上游内容。
3. 如果同一类冲突反复出现，`rerere` 会自动复用上次的解决结果。

常用命令：

```bash
git -C crates/tantivy status
git -C crates/tantivy rerere status
git -C crates/tantivy merge --abort
git -C crates/tantivy add <file>
git -C crates/tantivy merge --continue
```

## 维护原则

- 保持 `crates/tantivy` 尽量贴近上游，不在其中引入根工程专用的 workspace 改造。
- 根仓库只负责依赖重定向和 submodule gitlink 更新。
- 所有同步都先在 submodule 内完成，再回到根仓库做一次完整编译验证。
