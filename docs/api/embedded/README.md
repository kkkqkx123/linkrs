# Linkrs 嵌入式 API 文档

## 概述

Linkrs 的嵌入式 API 允许开发者把数据库直接嵌入到应用程序中使用，适合单机应用、桌面工具和测试环境。

仓库同时提供两层嵌入式入口：

- 嵌入式 Rust API（`embedded` feature）：主要入口，文档见下方模块表。
- C API（`c_api` feature）：嵌入式 Rust API 的薄 FFI 投影，头文件为 `include/linkrs.h`。

## 启用方式

嵌入式 Rust API 通过 `embedded` feature 暴露：

```bash
cargo build --features embedded
```

C API 通过 `c_api` feature 暴露（同时启用 `embedded`）：

```bash
cargo build --features c_api
```

## 模块结构

| 模块文档 | 功能 |
|----------|------|
| [00_overview.md](00_overview.md) | 入口与数据库打开逻辑 |
| [graph_database.md](graph_database.md) | `GraphDatabase` 入口与数据库打开逻辑 |
| [session.md](session.md) | 会话管理与查询执行上下文 |
| [transaction.md](transaction.md) | 事务控制与保存点 |
| [query_result.md](query_result.md) | 查询结果封装 |
| [row.md](row.md) | 行访问与类型化获取 |
| [prepared_statement.md](prepared_statement.md) | 预编译语句 |
| [batch_inserter.md](batch_inserter.md) | 批量写入辅助 |
| [config_types.md](config_types.md) | 嵌入式配置类型 |
| [examples.md](examples.md) | 完整使用示例 |

## 核心对象

### GraphDatabase

数据库入口，负责打开数据库并创建会话。

### Session

会话对象，负责切换图空间、执行查询和管理事务。

### Transaction

事务对象，负责事务控制、保存点管理。

## C API 说明

C API 位于 `crates/linkrs-api/src/embedded/c_api`，是嵌入式 Rust API 的薄 FFI 投影，由 `c_api` feature 编译。
它覆盖查询、事务与保存点、Schema 与版本历史、批量写入、配置（含批量更新）、迁移、UDF、统计、观测
（health/metrics/事务快照）、会话变量等能力。

头文件 `include/linkrs.h` 在 `c_api` 构建时由 `cbindgen` 从 Rust 源码重新生成，因此声明的 `linkrs_*`
始终与 Rust 导出保持一致。字符串返回值需由调用方使用 `linkrs_free_string` 释放；错误码通过返回值和
`linkrs_errmsg` 获取。
