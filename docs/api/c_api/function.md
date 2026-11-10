# C API 函数与扩展

源码：`crates/linkrs-api/src/embedded/c_api/function.rs`。
本地注册、无服务端持久化。

## 标量与聚合 UDF

| 函数 | 说明 |
|---|---|
| `linkrs_create_function(session, name, argc, user_data, x_func, x_destroy)` | 注册标量函数；`argc` 为参数个数，-1 表变参；`user_data` 透传回调并须在函数存续期有效 |
| `linkrs_create_aggregate(session, name, argc, user_data, x_step, x_final, x_destroy)` | 注册聚合函数（step + final 两回调） |
| `linkrs_delete_function(session, name)` | 注销自定义函数 |

## 函数执行上下文

`linkrs_context_t` 以借用指针传入回调，仅经以下函数访问：

| 函数 | 说明 |
|---|---|
| `linkrs_context_set_result(context, value)` | 在标量函数或聚合 xFinal 回调内设置返回值；`value` 可为 NULL 表空结果 |
| `linkrs_context_result_type(context)` | 已设返回值的类型标签 |
| `linkrs_context_set_error(context, error_msg)` | 执行失败时设置错误信息 |
| `linkrs_context_get_arg(context, index)` | 取第 `index` 个入参借用指针，越界返回 NULL，指针仅回调期间有效 |
| `linkrs_context_arg_count(context)` | 入参数 |

## 动态扩展

| 函数 | 说明 |
|---|---|
| `linkrs_load_extension(session, path)` | 装载 UDF 动态库（`.so/.dylib/.dll`）并注册其导出函数 |
| `linkrs_unload_extension(session, name)` | 按函数名卸载已装载的动态 UDF |
| `linkrs_reload_extension(session, name, reloaded)` | 按名从原库路径重载；`reloaded` 输出 1 表已重载、0 表文件未变跳过，可传 NULL 忽略 |
| `linkrs_list_extensions(session, out_json)` | 已装载扩展列表堆 JSON 数组，按名排序，条目含 `name/path/description/min_arity/max_arity/is_pure` |
| `linkrs_install_extension(session, source, out_name)` | 从来源字符串安装扩展：本地路径直接装载，远端来源遵循嵌入式语义（不支持的来源报 `LINKRS_ERROR`）；成功时 `out_name` 为堆注册名 |

堆字符串出参统一以 `linkrs_free_string` 释放。
