# C API 向量与全文

向量源码 `crates/linkrs-api/src/embedded/c_api/vector.rs`，
全文源码 `c_api/fulltext.rs`。
请求与结果走 JSON，阻塞式桥接（向量在一次性当前线程运行时上 park 异步协调器）。
符号始终声明，但无对应编译特性或后端未配置时返回
`LINKRS_NOT_IMPLEMENTED` 或透出嵌入式错误。

## 向量索引

| 函数 | 说明 |
|---|---|
| `linkrs_vector_create_index(session, tag, field, vector_size, distance, out_collection)` | 在 `(tag, field)` 上建索引；`distance` 为 `cosine/euclid(dot 别名 euclidean)/dot/manhattan`（大小写不敏感）；`out_collection` 可为 NULL，否则为堆集合名 |
| `linkrs_vector_drop_index(session, tag, field)` | 删除 `(tag, field)` 上的索引 |
| `linkrs_vector_search(session, tag, field, vector_json, limit, threshold, out_json)` | 相似搜索；`vector_json` 为浮点数组 JSON；`threshold` 为负表无截断；结果堆 JSON 数组 |

调用需先选中空间。

## 全文与混合搜索

| 函数 | 说明 |
|---|---|
| `linkrs_fulltext_search(session, tag, field, query, limit, out_json)` | `(tag, field)` 全文检索；结果为 `{"doc_id","score","highlights","matched_fields"}` 对象数组堆 JSON，`doc_id` 用引擎 `Value` serde 编码 |
| `linkrs_hybrid_search(session, tag, field, strategy, alpha, per_branch_limit, fused_limit, yield_fields_json, query_text, vector_json, out_json)` | 全文+向量融合检索；`strategy` 仅 `rrf`（倒数排名融合，k=60）或 `weighted`（`alpha` 权重混合），其他名 `LINKRS_MISUSE`；`yield_fields_json` 为两分支共享的字段名数组 JSON，`query_text` 喂全文分支，`vector_json`（浮点数组 JSON）喂向量分支；结果为 `{"id","score","highlights","payload","source"}` 数组堆 JSON，按融合分降序 |

两分支须同时编译并配置，否则报未实现或透出嵌入式错误。
