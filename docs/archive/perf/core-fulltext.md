# 核心类型与全文基准（2c9b92f8）

## serde_bench（通过）

单顶点序列化约 503ns，100 顶点批量约 45µs，反序列化约 1.5µs。

## fulltext_bench（缺失，既有问题）

构建被 vendored tantivy 子模块的编译错误阻塞：`top_hits.rs` 引用的 `DynamicColumn::PlainBytes` / `PlainStr` 变体在当前 columnar 版本中不存在（E0599）。属仓库既有状态（子模块版本漂移），与本次基准执行无关。待 tantivy 子模块同步后补跑，命令：`cargo bench -p linkrs-fulltext --features fulltext --bench fulltext_bench`。
