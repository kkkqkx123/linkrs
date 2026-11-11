# 核心类型与全文基准（2c9b92f8）

## serde_bench（通过）

单顶点序列化约 503ns，100 顶点批量约 45µs，反序列化约 1.5µs。

## fulltext_bench（已修复，全绿）

两处阻塞均已修复：Cargo `[patch]` 扩大到 tantivy 闭包全部 7 个 crate（columnar 等本地快照 API 已更新，含 `PlainBytes/PlainStr`）；`receiver.rs` 的 `map_err(SearchError::SerializationError)` 改为闭包（E0308）。另修 bench 自身 bug：`to_async` 闭包内再 `block_on` 会 panic（cannot start a runtime from within a runtime），index_build 改为同步 `iter_batched` + 外层 `block_on`，时序等价。

数据（Bm25，node1）：index_build/100 中位数 1.31s/iter（含 drop+create+100 篇+commit），/500 与 /100 几乎持平（1.34s，固定开销主导，约 76µs/篇增量）；search_rust 1.72µs，search_database_performance 5.05µs，search_search_query 4.49µs。
