# C API 会话

源码：`crates/linkrs-api/src/embedded/c_api/session.rs`。

## 会话生命周期与空间

| 函数 | 说明 |
|---|---|
| `linkrs_session_create(db, session)` | 创建会话 |
| `linkrs_session_close(session)` | 关闭会话 |
| `linkrs_session_use_space(session, space_name)` | 切换图空间 |
| `linkrs_session_current_space(session)` | 当前空间堆字符串；`linkrs_free_string` 释放 |
| `linkrs_session_set_autocommit(session, autocommit)` / `linkrs_session_get_autocommit(session)` | 自动提交开关读写 |

## 超时建议与中断

| 函数 | 说明 |
|---|---|
| `linkrs_busy_timeout(session, timeout_ms)` / `linkrs_busy_timeout_get(session)` | 建议式会话锁等待超时，引擎不据此强制等待，取消走中断与事务终止 |
| `linkrs_connection_interrupt(session)` | 协作取消运行中查询（单进程嵌入式主要取消手段） |
| `linkrs_connection_clear_interrupt(session)` | 清除中断标记 |
| `linkrs_connection_is_interrupted(session)` | 查询中断标记状态 |

## 跟踪与钩子

| 函数 | 说明 |
|---|---|
| `linkrs_trace(session, callback, user_data)` | SQL 跟踪回调，传 NULL 回调即卸载 |
| `linkrs_commit_hook(session, callback, user_data)` | 提交钩子，返回前一钩子指针 |
| `linkrs_rollback_hook(session, callback, user_data)` | 回滚钩子，返回前一钩子指针 |
| `linkrs_update_hook(session, callback, user_data)` | 行变更钩子（`LINKRS_HOOK_INSERT/UPDATE/DELETE` + 库/表/rowid），返回前一钩子指针 |

`user_data` 由调用方保证在钩子存续期有效。
