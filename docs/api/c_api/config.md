# C API 配置

源码：`crates/linkrs-api/src/embedded/c_api/config.rs`。

## 构造期：配置句柄

`linkrs_config_new/file/memory` 创建，`linkrs_config_free` 释放，
以 `linkrs_open_with_config` 消费（消费后句柄仍有效、可复用）。

| 函数 | 说明 |
|---|---|
| `linkrs_config_new()` | 默认配置 |
| `linkrs_config_file(path)` | 文件库配置 |
| `linkrs_config_memory()` | 内存库配置 |
| `linkrs_config_free(config)` | 释放 |
| `linkrs_config_set_cache_size(config, size_mb)` | 缓存 MB |
| `linkrs_config_set_timeout(config, timeout_ms)` | 超时毫秒 |
| `linkrs_config_set_read_only(config, read_only)` | 只读开关 |
| `linkrs_config_set_create_if_missing(config, create)` | 缺失即建开关 |
| `linkrs_config_set_enable_wal(config, enable)` | WAL 开关 |
| `linkrs_config_set_checkpoint_on_close(config, enable)` | 关闭时写终 checkpoint（仅文件库） |
| `linkrs_config_set_sync_mode(config, mode)` | 0=Full、1=Normal（默认）、2=Off；越界 `LINKRS_MISUSE` |
| `linkrs_config_set_vector_enabled(config, enable)` | 向量子系统开关；无显式本地目录的内存库保持关闭 |
| `linkrs_config_set_vector_local_dir(config, dir)` | 本地向量引擎数据目录；未设时由库路径派生 |
| `linkrs_config_set_vector_qdrant(config, client_json)` | 远端 Qdrant 引擎（`vector-qdrant` 特性编译时才有该符号），`client_json` 为引擎配置 serde 文档 |
| `linkrs_config_is_memory(config)` | 是否内存配置：1 内存、0 文件、-1 空句柄 |
| `linkrs_config_get_path(config)` | 读回库路径堆字符串，内存配置返回 NULL；`linkrs_free_string` 释放 |

## 运行期：已打开库的 `section.key`

支持键：`cache.size_mb`、`timeout.default_ms`、`wal.enabled`、`sync.mode`、
`storage.read_only`、`storage.create_if_missing`、`storage.checkpoint_on_close`。
其中 `storage.*` 即时生效，WAL/sync 下次 open 生效。

| 函数 | 说明 |
|---|---|
| `linkrs_config_get_value(db, section, key)` | 读单值，堆字符串，未知键返回 NULL |
| `linkrs_config_update_value(db, section, key, value)` | 写单值（字符串形态），未知键/不可解析值失败，细节进线程错误槽 |
| `linkrs_config_reset_value(db, section, key)` | 重置单值为编译默认值 |
| `linkrs_config_bulk_update(db, updates_json)` | 批量更新，JSON 对象 `"section.key" -> value`，如 `{"timeout.default_ms":"2000"}` |
