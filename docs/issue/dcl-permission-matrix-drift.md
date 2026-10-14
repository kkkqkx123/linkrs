# P2 [dcl] 文末权限矩阵与代码不符

## 现象

查询通道唯一的粗粒度门是 `extract_permission_from_statement` 的关键字前缀映射
（`crates/linkrs-server/src/graph_service/guard.rs:30-49`）：
SELECT/MATCH→Read；INSERT/CREATE→Write；DELETE/DROP→Delete；ALTER/ADD→Schema；
**其余一律 default Read**。逐行对照文档矩阵：

| 矩阵行 | 文档 | 代码实际 |
|---|---|---|
| 创建索引 | GOD/ADMIN/DBA | `CREATE INDEX`→Write，USER 可建 |
| 创建/删除标签、边类型 | GOD/ADMIN/DBA | →Write/Delete，USER 可建可删 |
| 创建/删除 Space | GOD/ADMIN | →Write/Delete，USER 可建可删 |
| 更新数据 | USER 起 | `UPDATE` 未映射落入 default Read，**GUEST 可改数据** |
| 授权/撤销角色 | 矩阵仅 GOD/ADMIN | 代码允许 DBA（`linkrs-core/src/permission.rs:56-66`），且与文档 §5 自身文字矛盾 |

`UPSERT`、`MERGE` 同样落入 default Read，GUEST 可执行这两类写操作。
另有两个叠加因素：`is_admin` 对 God/Admin 是跨空间全放行
（`crates/linkrs-server/src/permission/permission_manager.rs:188-197`）；
`check_permission` 在目标空间无角色时回退到 GOD_SPACE_ID 的全局角色
（`permission_manager.rs:225-246`）。

## 影响

矩阵承诺的 schema 管控整体失效；GUEST 可改数据是最严重的一条。

## 根因

权限推导以「首关键词」为输入，无法区分同前缀的不同语句
（CREATE TAG 与 CREATE INDEX 与 CREATE SPACE 同前缀但权限需求不同），
写语句枚举不全（UPDATE/UPSERT/MERGE 缺失）时落入最宽松的 default Read。
底层是「字符串近似匹配」代替「语句类别判定」的同一病根，
与 `dcl-auth-bypass-prefix-matching.md` 同源。

## 建议

以解析后的语句类别作为推导输入，把 DML/DDL/维护语句逐类映射到所需权限，
映射表集中在一处并注明来源；矩阵、代码、文档三者对齐。
另需明确 `is_admin` 跨空间放行与 GOD_SPACE_ID 回退是否为预期语义并写进文档。

## 严重级

**P2（矩阵失效，GUEST 可写数据）**

## 代码位置

`crates/linkrs-server/src/graph_service/guard.rs:30-49,77-89`、
`crates/linkrs-server/src/permission/permission_manager.rs:188-197,225-246`、
`crates/linkrs-core/src/permission.rs:56-66`
