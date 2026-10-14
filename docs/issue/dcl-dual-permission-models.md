# P2 [dcl] 两套并行权限模型，查询通道与 HTTP 探测各说各话

## 现象

仓库内存在两套权限实现：

1. **粗粒度**：`guard.rs` 的关键字前缀映射 + `PermissionManager::check_permission`，
   用于查询通道；
2. **细粒度**：`crates/linkrs-server/src/permission/permission_checker.rs:72-155` 的
   OperationType 体系（WriteSchema / WriteData / WriteUser / WriteRole /
   ChangePassword …），其**全部外部调用方只有 HTTP 探测 handler**
   （`crates/linkrs-server/src/http/handlers/authz.rs:108,167,191`）与单元测试，
   查询通道从不经过它。

两套模型对同一操作的判定互相打架：探测接口告诉前端「USER 不能创建索引」，
而查询通道实际允许（`CREATE INDEX`→Write）。且 `Permission::Schema` 只在
`ALTER/ADD` 前缀产生，`permission_checker.rs` 的 WriteSchema 规则与查询通道
实际行为不一致。

## 影响

前端依据探测结果禁用按钮，后端却放行；两套规则各自演化必然继续漂移，
排障时无法回答「以哪套为准」。

## 根因

细粒度 checker 先行落地但未接入查询通道，粗粒度门禁作为事实标准长期存在；
两者之间没有共享的判定内核。

## 建议

收敛为单一模型：查询通道改走按语句类别判定的权限推导（见
`dcl-permission-matrix-drift.md`），HTTP 探测 handler 改消费同一推导结果；
OperationType 中查询通道用不到的语义（WriteRole 层级、ReadUser 的 self-or-admin）
保留在 checker 内，但要在代码注释与文档中写明分工，避免再次漂移。

## 严重级

**P2（架构重复，行为不一致）**

## 代码位置

`crates/linkrs-server/src/permission/permission_checker.rs:72-155`、
`crates/linkrs-server/src/permission/permission_manager.rs:225-246`、
`crates/linkrs-server/src/http/handlers/authz.rs:108,167,191`、
`crates/linkrs-server/src/graph_service/guard.rs:30-89`
