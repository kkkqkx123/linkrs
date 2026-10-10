# 问题：tantivy 子工作区 bench 基建三连——lib test 编译失败 / jitexpr 需要 rustc 1.94 / common 空跑

- 状态：新建（待修复）
- 类型：bench 基建缺陷（crates/tantivy 为独立 workspace，经 [patch.crates-io] 接入主工程）
- 环境：rustc 1.93.0（2026-10-09）

## 问题描述

1. **lib test 目标编译失败**：`cargo bench --workspace`（会构建 lib test）报
   `error[E0422]: cannot find struct, variant or union type 'Bm25Params' in this scope`
   （编译器建议 `use crate::index::Bm25Params;`，位于 tantivy 库的测试模块内，约 index.rs:442 附近）。
   后果：workspace 级 `cargo bench`/`cargo test` 直接不可用；本次改为逐 `--bench` 绕过
   （bench 目标不依赖 lib test）。

2. **jitexpr 阻断 workspace 解析**：workspace 成员 `jitexpr` 依赖
   `cranelift = "0.134.3"`，解析到 0.134.4 后要求 **rustc 1.94.0**，cargo 在 1.93 上直接拒绝：
   `error: rustc 1.93.0 is not supported by the following packages: cranelift@0.134.4 ...`。
   根包对 jitexpr 只是 optional 依赖（feature `jitexpr`），未被 bench 使用，
   却因 `--workspace` 被拖入解析。当前只能 `--exclude jitexpr`。

3. **tantivy-common bench 静默空跑**：common/benches/bench.rs 使用 **binggan** 框架，
   但 common/Cargo.toml 无 `[[bench]] harness = false` 声明也无 `autobenches = false`，
   bench.rs 被自动发现为默认 libtest 目标——文件内没有 #[test] 函数，
   `cargo bench -p tantivy-common` 编译为空测试二进制，binggan 逻辑永不执行，无任何报错。

## 影响

- tantivy fork 的 14 个根 bench + 子包 bench 无法用 workspace 一键运行；
  common 的 vint/BitSet 基线缺失；jitexpr 锁死工具链升级路径（主工程 rust-version 1.88，
  但 jitexpr 传递要求 ≥1.94）。

## 修复方向

- 修复 `Bm25Params` 导入（补 `use crate::index::Bm25Params;`）。
- jitexpr：锁 `cranelift = "=0.134.3"`（或兼容 1.93 的版本），或把 jitexpr 移出
  默认 workspace members（改为 optional member / 独立目录）。
- common：补 `[[bench]] name = "bench" harness = false`；同时审查其余子包
  （bitpacker/columnar/sstable/stacker 已有 harness = false，无此问题）。
- 若需在 rustc 1.93 环境构建：`rustup update` 至 ≥1.94 亦可整体解掉 #2。
