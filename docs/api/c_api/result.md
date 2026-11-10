# C API 结果集

源码：`crates/linkrs-api/src/embedded/c_api/result.rs`。
结果集整体物化，不引入游标。

## 元信息

| 函数 | 说明 |
|---|---|
| `linkrs_result_free(result)` | 释放结果集 |
| `linkrs_column_count(result)` | 列数 |
| `linkrs_row_count(result)` | 行数 |
| `linkrs_column_name(result, col)` | 列名堆字符串；`linkrs_free_string` 释放 |
| `linkrs_column_type(result, col)` | 列 `linkrs_value_type_t` 标签 |
| `linkrs_result_execution_time_ms(result, out_ms)` | 执行耗时毫秒 |
| `linkrs_result_rows_scanned(result, out_rows)` | 扫描行数 |

## 逐格访问器

按名与按下标两套，`bool/float` 具备按名版本：

| 按名 | 按下标 | 说明 |
|---|---|---|
| `linkrs_get_int(result, row, col, value)` | `linkrs_get_int_by_index(result, row, col, value)` | 64 位整数 |
| `linkrs_get_string(result, row, col)` | `linkrs_get_string_by_index(result, row, col)` | 堆字符串，`linkrs_free_string` 释放 |
| `linkrs_get_bool(result, row, col, value)` | `linkrs_get_bool_by_index(result, row, col, value)` | 布尔 |
| `linkrs_get_float(result, row, col, value)` | `linkrs_get_float_by_index(result, row, col, value)` | 双精度浮点 |
| `linkrs_get_blob(result, row, col, len)` | `linkrs_get_blob_by_index(result, row, col, len)` | 借用字节视图，`len` 输出长度；内存随结果集释放 |

类型失配返回 `LINKRS_MISMATCH`。

| 函数 | 说明 |
|---|---|
| `linkrs_cell_is_null(result, row, col)` | 空值判断：1 空、0 非空 |

## JSON 导出

| 函数 | 说明 |
|---|---|
| `linkrs_row_to_json(result, row, out_json)` | 单行对象堆 JSON |
| `linkrs_result_to_json(result, out_json)` | 整集 `{columns, rows, metadata}` 信封堆 JSON |

单元格保持引擎 `Value` 标签序列化形态；`out_json` 以 `linkrs_free_string` 释放。
结果句柄可直接导出到文件，见 `linkrs_export_result_to_file`（[库生命周期](database.md)）。
