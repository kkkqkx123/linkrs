# C API 迁移

源码：`crates/linkrs-api/src/embedded/c_api/migration.rs`。
共享在线编排：写阻塞仅跨越改模式步骤，数据回填保持在线；调用阻塞式。

| 函数 | 说明 |
|---|---|
| `linkrs_migration_execute(db, plan_json)` | 执行 JSON 形态 `MigrationPlan`；成功返回 `linkrs_migration_report_t *`（以 `linkrs_migration_report_free` 释放），失败返回 NULL（细节经 `linkrs_errmsg`） |
| `linkrs_migration_rollback(db, plan_json)` | 回滚：有生成回滚计划则执行，否则从迁移前备份恢复；返回值约定同上 |
| `linkrs_migration_plan_json(db, space, label, is_edge, from_version, to_version, expand_contract)` | 生成迁移计划堆 JSON；`linkrs_free_string` 释放 |
| `linkrs_migration_report_free(report)` | 释放报告；NULL 空操作 |

报告结构：

```c
typedef struct linkrs_migration_report_t {
  uint8_t success;
  uint64_t steps_completed;
  uint64_t rows_migrated;
  char *errors_json;   /* 错误数组 JSON，以 linkrs_free_string 随报告一并释放 */
} linkrs_migration_report_t;
```
