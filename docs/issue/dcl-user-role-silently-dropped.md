# P1 [dcl] CREATE/ALTER USER 的 role 在执行末端被丢弃，WITH ROLE 静默无效

## 现象

role 参数完整穿过 parser → planner → executor command 四层，最后在 executor 被丢弃：

- `CREATE USER ... WITH ROLE X`：`CreateUserNode.role`
  （`crates/linkrs-query/src/planning/statements/ddl/user_management_planner.rs:88-98`）
  传到 `UserManageCommand::Create`
  （`crates/linkrs-query/src/executor/streaming/plan/arena_builder/specs/ddl.rs:455-459`），
  executor 只校验 `role.parse::<RoleType>()` 可解析，随后仅执行 `UserInfo::new` +
  `create_user`（`crates/linkrs-query/src/executor/streaming/operators/ddl_operator/auth_executor.rs:25-51`）。
  `UserInfo` 本身没有角色字段（`crates/linkrs-core/src/types/user.rs:68-91`），
  创建出的用户不带任何角色，连 `check_permission` 的 `no_role_in_space` 都无法通过。
- `ALTER USER ... WITH ROLE` / `SET ROLE =`：`UserAlterInfo` 没有 role 字段
  （`user.rs:184-203`），`UserStorage::alter_user` 不处理角色
  （`crates/linkrs-core/src/types/user_storage.rs:193-221`），executor 同样只校验不应用
  （`auth_executor.rs:66-92`）。

文档 §1「支持创建时分配角色」、§2「支持角色修改」两条特性从未生效。
现有测试只覆盖 `WITH ROLE <非法值>` 报错的负数场景
（`crates/linkrs-query/tests/dcl/security_tests.rs:167`、
`crates/linkrs-query/tests/dcl/transaction_consistency.rs:33`），
没有任何测试验证合法角色被真正授予。

## 影响

按文档创建带角色用户，得到的是无任何权限的账户；ALTER 改角色是静默无效操作。
「成功但无效果」比直接报错更糟，且会掩盖配置错误。

## 根因

角色在数据模型中按 (user, space) 归属（`user_storage.rs` 的
`HashMap<username, HashMap<space_id, RoleType>>`），而 CREATE/ALTER USER 语句
没有空间上下文。设计上这两条语句就不该承担角色授予；当前实现既不该悄悄丢弃参数，
也无法在现有模型内正确落地。

## 建议

从 CREATE USER / ALTER USER 语法中删除 role 子句，角色分配统一走空间显式的 GRANT
（已有层级校验）。若产品坚持「创建时带角色」，必须先定义目标空间语义
（唯一候选是 GOD_SPACE_ID 全局授予），并评估其对 `can_grant` 层级约束的冲击——
该候选方案形成隐式全局特权，建议否决。详见修复方案阶段一的决策记录。

## 严重级

**P1（功能缺失伴随静默失败）**

## 代码位置

`crates/linkrs-query/src/executor/streaming/operators/ddl_operator/auth_executor.rs:25-92`、
`crates/linkrs-query/src/planning/statements/ddl/user_management_planner.rs:88-116`、
`crates/linkrs-core/src/types/user.rs:68-91,184-203`、
`crates/linkrs-core/src/types/user_storage.rs:193-221`
