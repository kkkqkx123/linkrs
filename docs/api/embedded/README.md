# GraphDB 嵌入式 API 文档

## 概述

GraphDB 的嵌入式 API 允许开发者把数据库直接嵌入到应用程序中使用，适合单机应用、桌面工具和测试环境。

当前仓库只保留 Rust 嵌入式 API，不再维护独立的 C API 实现。

## 启用方式

嵌入式 API 通过 `embedded` feature 暴露：

```bash
cargo build --features embedded
```

## 模块结构

| 模块 | 功能 |
|------|------|
| `database` | `GraphDatabase` 入口与数据库打开逻辑 |
| `session` | 会话管理与查询执行上下文 |
| `transaction` | 事务控制 |
| `config` | 嵌入式配置类型 |
| `result` | 查询结果封装 |
| `batch` | 批量写入辅助 |
| `statement` | 预编译语句 |

## 核心对象

### GraphDatabase

数据库入口，负责打开数据库并创建会话。

### Session

会话对象，负责切换图空间、执行查询和管理事务。

### Transaction

事务对象，负责提交、回滚和保存点操作。

### QueryResult

查询结果封装，提供行列访问与类型化读取。

## 使用示例

```rust
use graphdb::api::embedded::{DatabaseConfig, GraphDatabase};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let db = GraphDatabase::open("my_database")?;
    let mut session = db.session()?;

    session.use_space("test_space")?;
    let result = session.execute("MATCH (n) RETURN n")?;

    println!("{:?}", result);
    Ok(())
}
```

## 配置

常用配置项由 `DatabaseConfig` 和 `TransactionConfig` 管理，具体字段请参考 [配置参考](../../release/07_configuration_reference.md)。

## 相关文档

- [配置参考](../../release/07_configuration_reference.md)
- [向量引擎文档索引](../../vector/README.md)
