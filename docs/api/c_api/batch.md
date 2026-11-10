# C API 批量写入

源码：`crates/linkrs-api/src/embedded/c_api/batch.rs`。
`linkrs_batch_inserter_create` 创建，`linkrs_batch_free` 释放；
写入器借用会话，会话须在写入器之前保持有效。

## 创建与执行

| 函数 | 说明 |
|---|---|
| `linkrs_batch_inserter_create(session, batch_size, batch)` | 创建；`batch_size <= 0` 默认 100 |
| `linkrs_batch_execute(batch)` | 刷出全部缓冲 |
| `linkrs_batch_free(batch)` | 释放；NULL 空操作 |

## 单条添加

| 函数 | 说明 |
|---|---|
| `linkrs_batch_add_vertex(batch, vid, tags)` | 单点；`tags` 为恰好一个非空标签，空或逗号分隔多标签拒绝 |
| `linkrs_batch_add_edge(batch, src_vid, dst_vid, edge_type)` | 单边；`edge_type` 非空 UTF-8 |

## 多条添加

| 函数 | 说明 |
|---|---|
| `linkrs_batch_add_vertices(batch, vids, count, tags)` | 多点：同享一个标签，`count` 为 0 时 `vids` 可为 NULL；遇首错即停 |
| `linkrs_batch_add_edges(batch, src_vids, dst_vids, count, edge_type)` | 多边：`src_vids[i]/dst_vids[i]` 组成第 i 条边；遇首错即停 |

多条添加与单条保持相同的转换规则、入序缓冲与按窗口自动刷出。

## 进度查询

| 函数 | 说明 |
|---|---|
| `linkrs_batch_vertices_inserted(batch, count)` | 已插入点数 |
| `linkrs_batch_edges_inserted(batch, count)` | 已插入边数 |
| `linkrs_batch_buffered_vertices(batch)` | 缓冲中点数，NULL 句柄返回 -1 |
| `linkrs_batch_buffered_edges(batch)` | 缓冲中边数，NULL 句柄返回 -1 |
| `linkrs_batch_result_json(batch, out_json)` | 进度汇总堆 JSON：已插入数、缓冲数、刷出错误数与错误信息；句柄调用后仍可用，`linkrs_free_string` 释放 |
