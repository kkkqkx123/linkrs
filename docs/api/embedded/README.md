# Linkrs 嵌入式 API 文档

## 概述

Linkrs 的嵌入式 API 允许开发者把数据库直接嵌入到应用程序中使用，适合单机应用、桌面工具和测试环境。

嵌入式 API 有两层：Rust 原生 API（`GraphDatabase/Session/Transaction`）与其 FFI 投影 C API（172 个导出函数，见 [C API 文档](../c_api/README.md)），后者是全部多语言绑定的底层（见[绑定文档](../bindings/README.md)）。

## 启用方式

嵌入式 API 通过 `embedded` feature 暴露：

```bash
cargo build --features embedded
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

C API（`c_api` 特性，隐含 `embedded`）是嵌入式 Rust API 的 FFI 投影，头文件 `include/linkrs.h` 由 cbindgen 生成，详见 [C API 文档](../c_api/README.md)。多语言绑定（Python/JS/Go/Java/R）均构建于其上，见[绑定文档](../bindings/README.md)。
