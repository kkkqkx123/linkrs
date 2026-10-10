# 问题：fulltext_bench 双缺陷——迭代内 create_index 无清理 + 默认 features 静默空跑

- 状态：新建（待修复）
- 类型：bench 基建缺陷
- 复现：
  - `cargo bench -p linkrs-fulltext --features fulltext --bench fulltext_bench` → panic（2026-10-09 实测）
  - `cargo bench -p linkrs-fulltext` → 编译为惰性占位，零覆盖（代码路径确认）

## 问题描述

1. **warmup 二轮 panic**：fulltext_bench.rs:46-48 把 `mgr.create_index(1, "Article", "content", ...)`
   放在 `b.iter(...)` 被测闭包内且无任何清理；criterion warmup 会执行闭包多次，
   第二次即触发 `FulltextIndexManager::create_index` 的
   `IndexAlreadyExists("space_ft_1_Article_content")`（manager.rs:469 返回，bench `.expect("create")` panic）。

```text
thread 'main' panicked at crates/linkrs-fulltext/benches/fulltext_bench.rs:47:26:
create: IndexAlreadyExists("space_ft_1_Article_content")
```

2. **默认 features 静默空跑**：bench 的实现全部被 `#[cfg(feature = "fulltext")]` 门控
   （fulltext_bench.rs 头部注释即声明需 `--features fulltext`），而 linkrs-fulltext 的
   `[features]` 未把 `fulltext` 列入 default。`cargo bench -p linkrs-fulltext` 会"成功"运行
   `#[cfg(not(feature))]` 的占位组——无报错、无数据，极易被误认为通过。

## 影响

- 全文索引构建/搜索路径当前零基准覆盖。

## 修复方向

- 把 `create_index` 移到迭代外（或每轮迭代先 `drop_index`/重建 manager），
  被测对象改为纯 indexing 吞吐；
- 将 `fulltext` 加入 linkrs-fulltext 的 `default` features（或至少在 bench 的
  `#[cfg(not(feature))]` 占位组里 `eprintln!` 提示，避免静默）；
- README/BENCHMARK_REPORT 的运行命令统一为带 `--features fulltext` 的版本。
