# DDL / 事务算子分析文档

## 概述

本文档分析 Linkrs 查询模块中 DDL（模式与权限管理）与事务控制语句在流式执行器中的
算子化实现。本文档与 2026-11-01 代码现状对齐。

DDL 与事务命令同样走统一的算子骨架：解析为 AST → 规划为 `DdlSpec` / `TxnSpec` →
`DdlOperator` / `TxnOperator` 执行，结果以单行/少量行的 DataChunk 形式流出。

---

## 一、DDL 算子（DdlOperator）

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/ddl_operator.rs`
（spec：`spec/ddl.rs`，经 `spec.rs` shim re-export）

### 1.1 命令载荷分类（`DdlSpec` 按域拆分）

| 命令枚举 | 覆盖范围 |
|----------|----------|
| `SpaceManageCommand` | 图空间：Create / Drop / Desc / Show / ShowCreate / Switch / Clear / Checkpoint / ExportDatabase / ImportDatabase / AttachDatabase / DetachDatabase / PurgeExpired |
| `TagManageCommand` | Tag：Create（含 TTL：`ttl_duration`/`ttl_col`）/ Alter（增删改属性、重命名注释）/ Rename / Desc / Drop |
| `IndexManageCommand` | 属性索引管理 |
| `EdgeManageCommand` | Edge 类型管理 |
| `TypeManageCommand` | 自定义类型 Create / Drop |
| `SequenceManageCommand` | 序列 Create / Drop（`start` 等参数） |
| `MacroManageCommand` | 宏定义 Create / Drop / Show（宏体在 binder 阶段展开，执行器只存定义） |
| `UserManageCommand` | 用户 Create（`PasswordInfo`）/ 删除 / 授权等，经 `exec_auth` 走权限层（`RoleType`） |

### 1.2 执行机制

```
open()  → 校验运行时上下文
next()  → 按命令域分发：
          exec_ddl          —— 模式类命令（Space/Tag/Edge/Index），落到元数据与存储
          exec_auth         —— 权限类命令（User/Role），落到 permission 层
          execute_sequence_manage / execute_macro_manage / execute_type_manage
                            —— 序列 / 宏 / 自定义类型
          结果经 make_manage_result(s) 构造为统一布局（LAYOUT）的 DataChunk 输出
close() → 释放
```

`manage_result_layout`（`:19`）固定了管理命令的输出列布局，保证不同 DDL 命令的
结果行结构一致。

---

## 二、事务算子（TxnOperator）

**文件路径**: `crates/linkrs-query/src/executor/streaming/operators/txn_operator.rs`
（spec：`spec/txn.rs`）

### 2.1 命令变体（`TxnOperatorKind`，6 种）

| 变体 | 说明 |
|------|------|
| `BeginTransaction` | 显式开启事务 |
| `Commit` | 提交 |
| `Rollback` | 回滚 |
| `Savepoint { name }` | 建立保存点 |
| `RollbackToSavepoint { name }` | 回滚到保存点 |
| `ReleaseSavepoint { name }` | 释放保存点 |

### 2.2 执行机制

- 会话级的事务操作（begin/commit/rollback/savepoint 的**会话状态管理**）由 API 层
  在算子运行前完成；执行器内的 `TxnOperator` 负责在查询计划中**落成一条命令记录**并
  产出结果行（`TransactionCommandResult::into_data_chunk`）；
- `controller()`：命令执行需要事务控制器；无活动事务时提交/回滚按语义报错
  （测试 `test_commit_without_active_transaction` 保障）；
- 每个命令**恰好产出一次**结果行（`emitted` 标志，测试 `transaction_command_emits_once` 保障）。

> 事务语义本身（MVCC、SSI、WAL）属存储与事务层，见 `docs/storage/`、
> `docs/research/ssi_serializable_isolation.md`；本文档只覆盖查询执行器内的算子形态。

---

## 三、关键文件路径汇总（2026-11-01 核实）

| 类别 | 文件路径 |
|------|----------|
| **DDL 执行算子** | `crates/linkrs-query/src/executor/streaming/operators/ddl_operator.rs` |
| **DDL spec（命令载荷）** | `crates/linkrs-query/src/executor/streaming/operators/spec/ddl.rs` |
| **事务执行算子** | `crates/linkrs-query/src/executor/streaming/operators/txn_operator.rs` |
| **事务 spec** | `crates/linkrs-query/src/executor/streaming/operators/spec/txn.rs` |
| **权限类型** | `crates/linkrs-core/src/permission.rs`（`RoleType`） |
| **DDL 语句语法** | `docs/release/03_ddl_schema_definition.md`、`docs/release/04_dcl_user_management.md`、`docs/release/06_transaction_management.md` |

---

## 四、总结

1. **命令载荷按域枚举**：Space/Tag/Edge/Index/Type/Sequence/Macro/User 八个命令域
   各自独立枚举，spec 即完整命令描述，执行器无需二次解析；
2. **统一输出布局**：管理命令结果经固定 LAYOUT 输出，客户端处理一致；
3. **职责分层清晰**：会话事务状态归 API 层、命令记录归 TxnOperator、
   持久化语义归存储层；
4. **可测试性**：无事务提交报错、单次产出等边界均有单元测试锚定。

---

**文档更新日期**: 2026-11-01  
**分析基于项目版本**: Linkrs Rust 实现（linkrs）
