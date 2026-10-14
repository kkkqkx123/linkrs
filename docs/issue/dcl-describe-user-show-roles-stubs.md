# P1 [dcl] DESCRIBE USER / SHOW ROLES 为桩实现

## 现象

- `DESCRIBE USER` 只返回一行 `User '<name>' exists`
  （`crates/linkrs-query/src/executor/streaming/operators/ddl_operator/auth_executor.rs:93-107`），
  文档 §7 承诺的「用户信息、角色分配、权限详情」均无。
- `SHOW ROLES` 无论是否带 `IN <space>` 永远返回空结果集
  （`auth_executor.rs:147-150`）；space 名被解析进 plan node 后从未使用
  （`user_management_planner.rs:158-162`）。
- 三个读语句（SHOW USERS / DESCRIBE USER / SHOW ROLES）准入仅需 `Permission::Read`
  （`crates/linkrs-server/src/graph_service/guard.rs:169-172`），GUEST 可枚举全部用户名；
  文档未定义这三者的准入角色，属于未声明语义。

数据侧并不缺素材：`list_user_roles` / `list_all_user_roles`
（`crates/linkrs-storage/src/client/traits_admin.rs:20-21`）与 `list_spaces`
（`crates/linkrs-storage/src/client/traits_reader.rs:357`）均已具备，
space_id 到 space 名的反查可通过扫描 `list_spaces` 获得。

## 影响

文档描述的能力不存在；管理员无法通过 DCL 巡检角色分配；
读语句的准入口径（含 GUEST 可枚举用户）是隐式行为。

## 根因

executor 的读分支从未接存储（构造空 DataChunk 占位），
planner 已准备的 space_name 参数没有消费方。

## 建议

补实现：DESCRIBE USER 输出用户名、锁定状态、创建/最近登录时间、各空间角色
（空间名经 `list_spaces` 反查）；SHOW ROLES 输出全部 用户→空间→角色 分配，
带 `IN <space>` 时过滤。准入收紧为：SHOW USERS / SHOW ROLES / DESCRIBE 他人需 Admin，
DESCRIBE 本人允许 self（产品口径决策，与文档同步）。
若暂不实现，文档必须显式降级为「仅返回存在性 / 暂不支持」，不停留中间态。

## 严重级

**P1（承诺能力缺失，准入口径未声明）**

## 代码位置

`crates/linkrs-query/src/executor/streaming/operators/ddl_operator/auth_executor.rs:93-150`、
`crates/linkrs-query/src/planning/statements/ddl/user_management_planner.rs:154-169`、
`crates/linkrs-server/src/graph_service/guard.rs:169-172`
