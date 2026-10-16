# Linkrs 数据控制语言 (DCL)

## 概述

数据控制语言 (DCL) 用于管理用户、角色和权限，控制对图数据库的访问。

---

## 1. CREATE USER - 创建用户

### 功能
创建数据库用户。

### 语法结构
```cypher
CREATE USER [IF NOT EXISTS] <user_name> [WITH] PASSWORD '<password>'
```

### 关键特性
- 支持 IF NOT EXISTS：重名且带该旗标时静默成功且不改动既有账户；不带旗标的重名创建报「用户已存在」
- 支持密码设置（`PASSWORD` 前的 `WITH` 可省略）
- 用户名与密码支持 Unicode 与特殊字符
- 角色不在此处分配，统一通过空间显式的 `GRANT`（见 §5）
- 也可通过通用 `CREATE USER ...` 语法触发（与 DDL 的 CREATE 语句共用入口）

### 示例
```cypher
CREATE USER IF NOT EXISTS alice WITH PASSWORD 'SecurePass123'
CREATE USER bob PASSWORD 'SecurePass456'   -- WITH省略形式
```

---

## 2. ALTER USER - 修改用户

### 功能
修改用户信息。

### 语法结构
```cypher
ALTER USER <user_name> [WITH PASSWORD '<new_password>']
ALTER USER <user_name> SET LOCKED = <true|false>
```

### 关键特性
- 支持密码修改
- 支持账户锁定/解锁；`SET LOCKED` 等号用单个 `=`，布尔值仅接受 `true`/`false`（`yes`、`1` 等显式报错）
- 角色不在此处修改，改经空间显式的 `GRANT` / `REVOKE`

### 示例
```cypher
ALTER USER alice WITH PASSWORD 'NewSecurePass456'
ALTER USER alice SET LOCKED = true
```

---

## 3. DROP USER - 删除用户

### 功能
删除用户。

### 语法结构
```cypher
DROP USER [IF EXISTS] <user_name>
```

### 关键特性
- 支持IF EXISTS
- 清理用户权限

### 示例
```cypher
DROP USER IF EXISTS alice
```

---

## 4. CHANGE PASSWORD - 修改密码

### 功能
修改本人密码（GOD 可修改其他用户密码）。

### 语法结构
```cypher
CHANGE PASSWORD [<user_name>] '<old_password>' TO '<new_password>'
```

### 关键特性
- 需要验证旧密码
- 支持密码强度检查，默认策略与强制范围见下
- 自助改密：省略 `<user_name>` 时目标为当前已认证用户，仅需正确旧密码
- 改密通道分流：本语句（DCL）仅允许“本人或 GOD”；HTTP 管理通道由 Admin 直接重置他人密码，不校验旧密码

### 默认密码策略与强制范围

- 默认取值（`PasswordPolicyConfig::default`）：长度 ≥ 8，且必须同时包含大写字母、
  小写字母与数字；默认不要求特殊字符，无过期、无历史保留；密码不得包含用户名。
- 强制范围：仅 server 查询通道在执行前强制（`guard.rs` 的
  `enforce_new_password_policy`，覆盖 CREATE USER / ALTER USER / CHANGE PASSWORD）；
  嵌入式与 API 直调路径不经过该门禁。嵌入式路径是否纳入强制作为独立议题记录，
  本阶段不实现。

### 示例
```cypher
-- 修改当前用户密码（自助形态）
CHANGE PASSWORD 'OldPass123' TO 'NewPass123'

-- GOD 修改其他用户密码
CHANGE PASSWORD alice 'OldPass123' TO 'NewPass123'
```

---

## 5. GRANT - 授予角色

### 功能
授予用户在指定图空间上的角色权限。

### 语法结构
```cypher
GRANT [ROLE] <role_type> ON <space_name> TO <user_name>
```

### 角色类型

| 角色 | 权限说明 |
|------|----------|
| `GOD` | 超级管理员，拥有所有权限 |
| `ADMIN` | 管理员，可管理图空间和用户 |
| `DBA` | 数据库管理员，可管理数据 |
| `USER` | 普通用户，可读写数据 |
| `GUEST` | 访客，只读权限 |

### 关键特性
- ROLE关键字可选
- 支持多种角色类型
- 基于图空间的权限控制
- 授权受角色层级约束：GOD 可授予除 GOD 外的所有角色；ADMIN 可授予 DBA/USER/GUEST；DBA 可授予 USER/GUEST

### 示例
```cypher
GRANT ROLE ADMIN ON social_network TO alice
GRANT DBA ON test_space TO bob
GRANT GUEST ON public_space TO visitor
```

---

## 6. REVOKE - 撤销角色

### 功能
撤销用户在指定图空间上的角色权限。

### 语法结构
```cypher
REVOKE [ROLE] <role_type> ON <space_name> FROM <user_name>
```

### 关键特性
- ROLE关键字可选
- 模型为「每用户每空间一个角色」：`REVOKE` 前会校验该用户在当前空间的现有角色与命名角色一致，不一致或不存在均报错，不再静默清除
- 因此撤销成功即表示该用户在该空间已无角色
- 撤销同样受角色层级约束（与授权规则一致）

### 示例
```cypher
REVOKE ROLE ADMIN ON social_network FROM alice
REVOKE DBA ON test_space FROM bob
REVOKE GUEST ON public_space FROM visitor
```

---

## 7. DESCRIBE USER - 描述用户

### 功能
显示用户的详细信息和权限。

### 语法结构
```cypher
DESCRIBE USER <user_name>
DESC USER <user_name>
```

### 关键特性
- 显示用户信息（用户名、锁定状态、创建/最近登录时间）
- 显示该用户在各空间的角色分配（每分配一行）
- 准入：仅允许本人或 Admin/God；GUEST 不可枚举他人信息

### 输出列

| 列 | 类型 | 说明 |
|---|---|---|
| `username` | string | 用户名 |
| `locked` | bool | 账户是否锁定 |
| `created_at` | bigint | 创建时间（毫秒时间戳） |
| `last_login_at` | bigint | 最近登录时间（毫秒时间戳，无登录时为空） |
| `space_name` | string | 空间名（无分配时为空） |
| `role` | string | 该空间上的角色（无分配时为空） |

无任何角色分配的用户输出一行且 `space_name` / `role` 为空；
空间已删除但角色映射残留时 `space_name` 显示数字 id；不存在的用户报错。

### 示例
```cypher
DESCRIBE USER alice
DESC USER bob
```

---

## 8. SHOW USERS - 显示用户列表

### 功能
显示所有用户列表。仅 Admin/God 可执行。

### 语法结构
```cypher
SHOW USERS
```

### 示例
```cypher
SHOW USERS
```

---

## 9. SHOW ROLES - 显示角色信息

### 功能
显示全部 用户→空间→角色 分配。仅 Admin/God 可执行。

### 语法结构
```cypher
SHOW ROLES [IN <space_name>]
```

### 输出列

| 列 | 类型 | 说明 |
|---|---|---|
| `username` | string | 用户名 |
| `space_name` | string | 空间名 |
| `role` | string | 该用户在该空间的角色 |

带 `IN <space_name>` 时按空间名过滤；无分配时返回空结果；
空间不存在（含从未创建）同样返回空结果而非报错。

### 示例
```cypher
SHOW ROLES
SHOW ROLES IN test_space
```

---

## 权限矩阵

判定依据是解析后的语句类别与变体，不是语句文本前缀；粘连词形（如 `CREATEUSER`）、多空格、
大小写变体与规范拼写走同一门禁。所有行列均按**当前会话空间**判定，会话空间无角色时回退到全局
角色；因此只在目标空间持有角色的用户，需先 `USE` 该空间再执行带空间限定的语句。

| 操作 | GOD | ADMIN | DBA | USER | GUEST |
|------|-----|-------|-----|------|-------|
| 创建/删除Space | ✓ | ✓ | ✗ | ✗ | ✗ |
| 创建/删除用户 | ✓ | ✓ | ✗ | ✗ | ✗ |
| 授权/撤销角色 | ✓ | ✓ | ✓* | ✗ | ✗ |
| 创建/删除标签 | ✓ | ✓ | ✓ | ✗ | ✗ |
| 创建/删除边类型 | ✓ | ✓ | ✓ | ✗ | ✗ |
| 修改标签/边类型注释（`ALTER ... COMMENT`） | ✓ | ✓ | ✓ | ✗ | ✗ |
| 插入数据 | ✓ | ✓ | ✓ | ✓ | ✗ |
| 更新数据 | ✓ | ✓ | ✓ | ✓ | ✗ |
| 删除数据 | ✓ | ✓ | ✓ | ✓ | ✗ |
| 查询数据 | ✓ | ✓ | ✓ | ✓ | ✓ |
| 创建索引 | ✓ | ✓ | ✓ | ✗ | ✗ |
| `PURGE EXPIRED`（维护） | ✓ | ✓ | ✓ | ✗ | ✗ |
| `CHECKPOINT`（维护） | ✓ | ✓ | ✓ | ✓ | ✗ |
| 配置语句（`SHOW/UPDATE CONFIGS`） | ✓ | ✓ | ✗ | ✗ | ✗ |
| `SHOW USERS` / `SHOW ROLES` / `DESCRIBE` 他人 | ✓ | ✓ | ✗ | ✗ | ✗ |
| `DESCRIBE` 本人 | ✓ | ✓ | ✓ | ✓ | ✓ |

* `✓*`：DBA 仅可授予/撤销 USER/GUEST；ADMIN 可授予 DBA/USER/GUEST；
GOD 可授予除 GOD 外的所有角色。层级约束见 §5（`RoleType::can_grant`）。

### 权限模型分工

查询通道按上表判定（语句类别推导，`GraphService::permission_for_stmt`）；
HTTP 探测与管理接口走 `PermissionChecker`（`OperationType` 体系，
`http::handlers::authz` 经同一推导消费）。两者以同一角色能力表
（`RoleType::has_permission` / `can_grant`）为底，因此判定结果一致。
`WriteRole` 层级校验、`ReadUser` 的 self-or-admin、`ChangePassword` 的
self-or-god 等语义仅保留在 checker 内，查询通道不经过它们。
