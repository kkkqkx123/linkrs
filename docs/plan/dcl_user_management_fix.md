# DCL 用户管理修复方案（分阶段）

> 问题清单已按单问题文件落盘于 `docs/issue/`，本文各阶段开头注明对应的问题文件（ basename 省略 `.md`）。
> 本文只描述改什么、在哪改、按什么顺序改、如何验收；不附代码。
> 项目处于开发阶段，无兼容负担：失效语法可直接删除，不为从未生效的行为保留入口。

## 问题文件索引

| 问题文件 | 严重级 | 主题 |
|---|---|---|
| dcl-auth-bypass-prefix-matching | P0 | 授权分类按字符串前缀，粘连词形与空白变体可绕过用户管理门禁 |
| dcl-user-role-silently-dropped | P1 | CREATE/ALTER USER 的 role 在执行末端被丢弃 |
| dcl-set-role-locked-unparseable | P1 | SET ROLE / SET LOCKED 子句不可解析 |
| dcl-self-change-password-broken | P1 | 自助 CHANGE PASSWORD 断链 |
| dcl-revoke-drops-role | P1 | REVOKE 丢弃 role 参数 |
| dcl-create-user-if-not-exists-noop | P1 | IF NOT EXISTS 语义失效 |
| dcl-describe-user-show-roles-stubs | P1 | DESCRIBE USER / SHOW ROLES 为桩实现 |
| dcl-permission-matrix-drift | P2 | 文末权限矩阵与代码不符 |
| dcl-dual-permission-models | P2 | 两套并行权限模型 |
| dcl-doc-vs-code-drift | P2 | 改密通道分工、密码策略示例等零散漂移 |
| dcl-duplicate-parser-paths | P3 | 同一语法多份重复实现且行为漂移 |
| dcl-dead-code-and-hygiene | P3 | 死词元、错误信息错粘贴、凭据明文存储 |

## 总原则

1. **顺序约束：授权分类修复必须先于任何角色语义落地。** 当前存在可绕过的用户管理门禁
   （dcl-auth-bypass-prefix-matching），若先把 CREATE/ALTER USER 的 role 真正授予，
   绕过门禁的粘连词形与空白变体会立刻构成完整提权链。阶段零只收紧门禁，不新增能力。
2. 每个阶段独立可编译、可验证，结束时做一次合并验证（系统资源紧张，不为每阶段单独全量构建）。
3. 所有改动作用于现有模块，不新建替代模块；清理即删除，不留兼容层。
4. 语义决策优先于实现：每个「静默无效」要么变成有声失败，要么从语法中删除，不停留中间态。
5. 文档修订与代码同阶段推进，不遗留「代码已改、文档未动」的窗口。

## 阶段零：授权分类改为按语句变体判定（对应 dcl-auth-bypass-prefix-matching）

**目标**：用户管理语句的准入与执行后对齐只认解析结果，不认字符串形状。

**改动点**：

- `crates/linkrs-server/src/graph_service/guard.rs`：
  `check_user_management_permission` 的前缀清单替换为按 `Stmt` 变体匹配
  （解析能力已具备，`parse_stmt` 就在同文件）；`enforce_user_management` 的
  匹配臂保持不变，补齐对全部九个 DCL 变体的覆盖，删除前缀分支。
- `crates/linkrs-server/src/graph_service/query.rs`：`is_user_management_statement`
  改用同一变体判定；该函数与 guard 中的判定合并为一处共享实现，避免再次漂移。
- `crates/linkrs-server/src/graph_service/guard.rs` 的通用粗粒度映射
  （`extract_permission_from_statement`）本阶段不动，仅确保 DCL 语句不再落入其中。

**验收**：

- 新增测试：以 USER 角色执行 `CREATEUSER`、`DROPUSER`、`ALTERUSER USER ... PASSWORD`、
  `CHANGEPASSWORD` 及 `CREATE  USER`（双空格）、`CREATE\tUSER`、`CHANGE  PASSWORD`
  等变体，全部得到权限拒绝，而非降级放行。
- 原有 `crates/linkrs-query/tests/dcl` 与 `tests/integration_permission.rs` 全绿。

**风险与回退**：误收窄会把合法 DCL 挡在门外；以九个变体的正向用例（Admin/God 正常执行）
作为回归保护。本阶段无行为新增，回退即还原判定函数。

**明确不做**：不修 SET 子句解析，不动 role 授予，不改权限矩阵。

## 阶段一：解析与执行语义修复

对应 dcl-set-role-locked-unparseable、dcl-self-change-password-broken、
dcl-user-role-silently-dropped、dcl-revoke-drops-role、dcl-create-user-if-not-exists-noop。

按依赖排序，先解析后执行；role 语义决策是本阶段核心。

**1. SET 子句解析（dcl-set-role-locked-unparseable）**

- `crates/linkrs-query/src/parser/parsing/user_parser.rs` 与
  `crates/linkrs-query/src/parser/parsing/ddl_parser/alter_parser.rs` 两处
  ALTER USER 实现：等号改按 `Tk::Assign` 接受；布尔值按 `BooleanLiteral` 读取，
  其余词元显式报「期望布尔值」，取消 `to_lowercase() == "true"` 的模糊匹配。
  （两处重复实现按 dcl-duplicate-parser-paths 先收敛为一份再修亦可，但不得只改一处。）
- 补解析测试：`SET ROLE = DBA`、`SET LOCKED = true/false` 正向；
  `SET LOCKED = yes`、`SET LOCKED = 1` 报错。

**2. 自助 CHANGE PASSWORD（dcl-self-change-password-broken）**

- 沿 `password_history_depth` 已有的 threading 模式
  （`linkrs-server` 注入 → `crates/linkrs-api/src/api_core/query_api.rs` → `QueryContext`）
  增加已认证用户名的传递，落点是 `crates/linkrs-query/src/context/query_context.rs`
  及其 builder，注入点在 `crates/linkrs-server/src/http/server.rs` 与
  `crates/linkrs-server/src/graph_service/factory.rs` 中与 `with_password_history_depth`
  相同的位置。
- `crates/linkrs-query/src/planning/statements/ddl/user_management_planner.rs` 在
  `ChangePasswordStmt.username` 为空时回填该用户名。
- 补执行测试：正确旧密码的自助改密成功；错误旧密码失败；改密后旧密码失效。

**3. role 语义决策与落地（dcl-user-role-silently-dropped）**

决策：**从 CREATE USER / ALTER USER 语法中删除 role 子句**。理由：角色在数据模型中
按 (user, space) 归属，这两条语句没有空间上下文；若要授予只能落到 GOD_SPACE_ID
形成隐式全局特权，这与层级约束（`can_grant`）的初衷冲突，也是滥用面最大的形态。
角色分配统一走空间显式的 GRANT（已有层级校验），文档同步改写。
备选方案（落到 GOD_SPACE_ID 并写进文档）显式否决，不保留开关。

- parser：删除 `crates/linkrs-query/src/parser/parsing/user_parser.rs` 中
  `parse_create_user_internal` 的 `WITH ROLE` 分支与 `parse_alter_user_internal` 的
  `WITH ROLE` / `SET ROLE` 赋值；AST 的 `CreateUserStmt.role`、
  `AlterUserStmt.new_role`（`crates/linkrs-query/src/parser/ast/stmt/dcl.rs`）一并删除。
- planner 与 executor：`CreateUserNode.role`、`AlterUserNode.new_role`、
  `UserManageCommand::Create.role` / `Alter.new_role` 及相应校验
  （`crates/linkrs-query/src/executor/streaming/operators/ddl_operator/auth_executor.rs`）
  随之删除；`specs/ddl.rs` 去掉 role 传递。
- 测试：`WITH ROLE` 变为语法错误；`crates/linkrs-query/tests/dcl` 中依赖 `WITH ROLE`
  的用例改写为 GRANT 流程。

**4. REVOKE 的 role 生效（dcl-revoke-drops-role）**

- `crates/linkrs-query/src/planning/plan/core/nodes/management/user_nodes.rs` 的
  `RevokeRoleNode` 恢复 role 字段（planner 传递 `revoke_stmt.role`）；
  executor 在撤销前读取该用户在该空间的现有角色，与命名角色不一致则报错。
- 测试：撤销不匹配的角色报错；撤销匹配角色成功；撤销后该用户在空间内无角色。

**5. 重名语义（dcl-create-user-if-not-exists-noop）**

- `crates/linkrs-core/src/types/user_storage.rs`：`create_user` 对已存在用户
  区分 `IF NOT EXISTS`：带旗标静默成功，否则返回「用户已存在」错误；
  旗标由 executor 随命令传入（`UserManageCommand::Create` 增加旗标参数）。
- 测试：重名创建默认失败；带 `IF NOT EXISTS` 成功且不改动原账户。

**验收**：`cargo test -p linkrs-query -p linkrs-core -p linkrs-server`（一次合并运行）。

**风险与回退**：删语法会打断现存的 `WITH ROLE` 脚本——开发阶段可接受，
但需在阶段内一次性改完文档与测试，不留半删除状态。回退粒度按上述五个子项独立进行。

**明确不做**：权限矩阵与粗粒度映射（阶段二）；DESCRIBE/SHOW ROLES 桩（阶段三）。

## 阶段二：权限模型统一与文档对齐

对应 dcl-permission-matrix-drift、dcl-dual-permission-models、dcl-doc-vs-code-drift。

**目标**：查询通道的权限判定从「首关键词前缀」升级为「语句类别推导」，两套模型收敛为一套。

**改动点**：

- 复用 parser 已有的 `StmtCategory` 作为推导输入，在
  `crates/linkrs-server/src/graph_service/guard.rs` 中把
  `extract_permission_from_statement` 改为按类别与语句种类映射：
  DML 写（INSERT/UPDATE/UPSERT/MERGE/DELETE）→ Write 或 Delete；
  schema 语句（CREATE/DROP/ALTER TAG/EDGE/INDEX/SPACE）→ Schema；
  数据读 → Read；配置语句维持 Admin。映射表集中在一处并配注释说明来源。
- 修正具体越权行：UPDATE/UPSERT/MERGE 不再落入 default Read；
  CREATE INDEX / CREATE TAG / CREATE SPACE 不再按 Write 放行 USER。
- `crates/linkrs-server/src/permission/permission_checker.rs` 的 OperationType 体系
  与上述推导收敛：让 HTTP 探测 handler（`crates/linkrs-server/src/http/handlers/authz.rs`）
  改消费同一推导结果；OperationType 中查询通道用不到的语义（WriteRole 层级、
  ReadUser 的 self-or-admin）保留在 checker 内，并在代码注释与文档中写明分工。
- 层级口径统一：`can_grant` 允许 DBA 授予 USER/GUEST 是既定模型
  （`crates/linkrs-core/src/permission.rs`），矩阵行按代码修正，
  文档 §5 与矩阵保持一致。
- 改密通道分流（dcl-doc-vs-code-drift 第 1 条）：DCL 维持「本人或 GOD」，
  HTTP 管理通道维持 Admin 可重置；写进文档 §4。
- 密码策略（dcl-doc-vs-code-drift 第 2 条）：文档示例密码改为符合默认策略；
  文档写明默认策略取值与强制范围（仅 server 查询通道）。嵌入式路径是否纳入强制，
  作为独立议题记录，不在本阶段实现。

**验收**：

- 矩阵测试（`tests/integration_permission.rs` 扩充）：USER 不能建索引/标签/空间；
  GUEST 不能 UPDATE/UPSERT/MERGE；DBA 授予矩阵逐格验证；God/Admin 行为不变。
- 探测接口与查询通道对同一操作判定一致的集成测试。

**风险与回退**：判定收紧可能挡掉现有合法用法；以「默认拒绝、按类别显式放行」的写法
把偏差变成显式错误而非静默放行。回退即还原单一映射函数。

**明确不做**：新增 OperationType、新增权限种类；只做收敛。

## 阶段三：桩实现补齐与代码卫生

对应 dcl-describe-user-show-roles-stubs、dcl-duplicate-parser-paths、dcl-dead-code-and-hygiene。

**目标**：让读语句可用或让文档诚实；删除死代码与重复实现。

**改动点**：

- DESCRIBE USER（`crates/linkrs-query/src/executor/streaming/operators/ddl_operator/auth_executor.rs`）：
  输出用户名、锁定状态、创建/最近登录时间、各空间角色。space_id 到 space 名的反查通过
  `list_spaces` 扫描获得（storage 已有 `list_user_roles` 与 `list_spaces`，无需新增存储能力）。
- SHOW ROLES：输出全部 用户→空间→角色 分配；带 `IN <space>` 时按空间过滤
  （space 名已在 plan node 中，接上即可）。
- 三个读语句的准入（当前仅需 Read，GUEST 可枚举用户）：建议收紧为
  SHOW USERS / SHOW ROLES / DESCRIBE USER 他人需 Admin，DESCRIBE 本人允许 self；
  这是产品口径决策，与文档同步写明。
- 重复实现收敛（dcl-duplicate-parser-paths）：
  `crates/linkrs-query/src/parser/parsing/ddl_parser/alter_parser.rs` 的 ALTER USER
  私有实现删除，统一走 `user_parser.rs` 的唯一版本；
  `crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs` 中的 CREATE USER
  副本确认不可达后删除；`UserParser` 中三个不可达的 describe/show 方法删除，
  存活路径的 space 名解析统一为 `expect_dcl_name`。
- 死代码清理（dcl-dead-code-and-hygiene）：`TokenKind::AdminRole`
  （`crates/linkrs-query/src/parser/core/token.rs`）及其在 `user_parser.rs`、
  `parse_context.rs` 的匹配分支删除；`crates/linkrs-query/src/parser/ast/stmt/dcl.rs`
  的 FromStr 错误信息改为 "Unknown role type"。
- `users.json` 明文哈希：补充文件权限说明与安全注释；是否加密作为独立议题记录，
  不在本阶段实现。

**验收**：DESCRIBE/SHOW ROLES 的输出断言（含空间名反查、空分配、不存在的用户）；
死代码删除后 `cargo test -p linkrs-query -p linkrs-server -p linkrs-core` 一次合并运行全绿；
`cargo clippy` 对改动 crate 无新增警告。

**风险与回退**：输出格式是新增契约，需与前端（`frontend/src/lib/services/users.ts`
的 DCL 回退路径）确认列名；准入收紧可能挡掉既有 GUEST 用法，以集成测试兜底。

**明确不做**：存储加密改造、权限模型再扩展。

## 文档同步修订清单

代码每个阶段结束时同步改 `docs/release/04_dcl_user_management.md`：

1. 阶段一：§1 删除「创建时分配角色」，补 `IF NOT EXISTS` 真实行为；§2 修正 SET 子句
   （修复后的语法）并说明角色改经 GRANT；§4 自助形态行为与示例密码；§6 REVOKE 的
   role 校验语义。
2. 阶段二：文末权限矩阵逐行重写；§5 层级说明与矩阵对齐；§4 改密通道分流说明；
   密码策略默认值与强制范围。
3. 阶段三：§7/§9 按补实现后的输出重写；三个读语句的准入角色写入 §7/§8/§9。

## 非目标

- 不改认证主流程（登录、锁定、会话驱逐）与 bootstrap 种子逻辑。
- 不引入新权限种类、新角色等级；不为 DCL 增加 HTTP/gRPC 传输面
  （管理通道已存在，保持分流）。
- 不做 `users.json` 加密与存储格式升级；不做版本升级与数据迁移。
