# C API 统计

源码：`crates/linkrs-api/src/embedded/c_api/statistics.rs`。
嵌入式以单进程自监控为主，无服务端 system/search/freeze 全局注册表。

## 会话统计

| 函数 | 说明 |
|---|---|
| `linkrs_session_changes(session)` | 最近一次操作影响行数 |
| `linkrs_session_total_changes(session)` | 累计影响行数 |
| `linkrs_session_last_insert_vertex_id(session, vertex_id)` | 最近插入点 id（无则 0） |
| `linkrs_session_last_insert_edge_id(session, edge_id)` | 最近插入边 id（无则 0） |
| `linkrs_session_get_statistics(session, out_stats)` | 会话统计结构 `linkrs_session_statistics_t`（`last_changes/total_changes/last_insert_vertex_id/last_insert_edge_id`） |

库级概览 `linkrs_database_overview`（空间数/事务指标/计数器子集堆 JSON）
见[库生命周期](database.md)；会话 `statistics` 的 JSON 形态为各绑定
`statistics()` 的上游口径。

## 繁忙处理器

源码 `c_api/busy_handler.rs`。调用方自驱动的重试记账，引擎不自动调用；
与建议式 `linkrs_busy_timeout`（见[会话](session.md)）配合使用。

| 函数 | 说明 |
|---|---|
| `linkrs_busy_handler_create(timeout_ms)` | 创建；`timeout_ms` 为 0 表不等待 |
| `linkrs_busy_handler_free(handler)` | 释放 |
| `linkrs_busy_handler_handle(handler)` | 忙时处理：1 继续等、0 超时/放弃 |
| `linkrs_busy_handler_is_timeout(handler)` | 是否已超时：1/0 |
| `linkrs_busy_handler_retry_count(handler)` | 当前重试次数，出错返回 0 |
| `linkrs_busy_handler_elapsed_ms(handler)` | 已流逝毫秒，出错返回 0 |
| `linkrs_busy_handler_reset(handler)` | 重置状态 |
