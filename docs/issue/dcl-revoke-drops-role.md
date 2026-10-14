# P1 [dcl] REVOKE 丢弃 role 参数，删除整个 (user, space) 映射

## 现象

`REVOKE ROLE <role> ON <space> FROM <user>` 中的 role 在 planner 层即被丢弃：
`RevokeRoleNode` 没有 role 字段
（`crates/linkrs-query/src/planning/plan/core/nodes/management/user_nodes.rs:225-239`），
planner 不传递
（`crates/linkrs-query/src/planning/statements/ddl/user_management_planner.rs:146-153`），
`specs/ddl.rs:479-482` 同样丢弃。`UserStorage::revoke_role` 删除该 (user, space) 的
整条映射（`crates/linkrs-core/src/types/user_storage.rs:283-304`）。

底层模型本就是「每用户每空间一个角色」，GRANT 是覆盖写入
（`user_storage.rs:274-278`）。因此文档 §6「撤销指定角色权限、不影响其他角色权限」
描述的按角色撤销语义在该模型中不存在：

- 撤销 ADMIN 与撤销 GUEST 的效果完全相同（都是清掉整条映射）；
- 撤销一个用户并不拥有的角色也返回成功，无任何反馈。

## 影响

`REVOKE GUEST ON s FROM u` 会把 u 在 s 空间的 ADMIN 一并清掉；拼错角色名无感知，
误操作直接剥夺权限且难以排查。

## 根因

数据模型为单角色每空间，与文档暗示的多角色语义存在结构性落差；
实现选择了「忽略参数」而不是「让参数生效或拒绝参数」。

## 建议

让 role 参数产生语义：执行时读取该用户在该空间的现有角色，与命名角色不一致则报错。
备选是把 role 从语法中删除（`REVOKE ON <space> FROM <user>`）——两种都可接受，
共同底线是不再静默忽略用户输入。

## 严重级

**P1（误操作无反馈、权限被静默剥夺）**

## 代码位置

`crates/linkrs-query/src/planning/plan/core/nodes/management/user_nodes.rs:225-239`、
`crates/linkrs-query/src/planning/statements/ddl/user_management_planner.rs:146-153`、
`crates/linkrs-query/src/executor/streaming/plan/arena_builder/specs/ddl.rs:479-482`、
`crates/linkrs-core/src/types/user_storage.rs:283-304`
