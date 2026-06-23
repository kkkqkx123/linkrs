# GraphDB 数据控制语言 (DCL)

## 概述

数据控制语言 (DCL) 用于管理用户、角色和权限，控制对图数据库的访问。

---

## 1. CREATE USER - 创建用户

### 功能
创建数据库用户。

### 语法结构
```cypher
CREATE USER [IF NOT EXISTS] <user_name> WITH PASSWORD '<password>' [WITH ROLE <role>]
```

### 关键特性
- 支持IF NOT EXISTS
- 支持密码设置
- 支持角色分配

### 示例
```cypher
CREATE USER IF NOT EXISTS alice WITH PASSWORD 'secure_password'
CREATE USER bob WITH PASSWORD 'password123' WITH ROLE ADMIN
```

---

## 2. ALTER USER - 修改用户

### 功能
修改用户信息。

### 语法结构
```cypher
ALTER USER <user_name> [WITH PASSWORD '<new_password>'] [WITH ROLE <role>]
ALTER USER <user_name> SET ROLE = <role>
ALTER USER <user_name> SET LOCKED = <true|false>
```

### 关键特性
- 支持密码修改
- 支持角色修改
- 支持账户锁定/解锁

### 示例
```cypher
ALTER USER alice WITH PASSWORD 'new_secure_password'
ALTER USER bob SET ROLE = DBA
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
修改当前用户或其他用户密码。

### 语法结构
```cypher
CHANGE PASSWORD [<user_name>] '<old_password>' TO '<new_password>'
```

### 关键特性
- 需要验证旧密码
- 支持密码强度检查
- 管理员可修改其他用户密码

### 示例
```cypher
-- 修改当前用户密码
CHANGE PASSWORD 'old_pass' TO 'new_pass'

-- 管理员修改其他用户密码
CHANGE PASSWORD alice 'old_pass' TO 'new_pass'
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
- 撤销指定角色权限
- 不影响其他角色权限

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
- 显示用户信息
- 显示角色分配
- 显示权限详情

### 示例
```cypher
DESCRIBE USER alice
DESC USER bob
```

---

## 8. SHOW USERS - 显示用户列表

### 功能
显示所有用户列表。

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
显示角色信息或在指定空间的角色分配。

### 语法结构
```cypher
SHOW ROLES [IN <space_name>]
```

### 示例
```cypher
SHOW ROLES
SHOW ROLES IN test_space
```

---

## 权限矩阵

| 操作 | GOD | ADMIN | DBA | USER | GUEST |
|------|-----|-------|-----|------|-------|
| 创建/删除Space | ✓ | ✓ | ✗ | ✗ | ✗ |
| 创建/删除用户 | ✓ | ✓ | ✗ | ✗ | ✗ |
| 授权/撤销角色 | ✓ | ✓ | ✗ | ✗ | ✗ |
| 创建/删除标签 | ✓ | ✓ | ✓ | ✗ | ✗ |
| 创建/删除边类型 | ✓ | ✓ | ✓ | ✗ | ✗ |
| 插入数据 | ✓ | ✓ | ✓ | ✓ | ✗ |
| 更新数据 | ✓ | ✓ | ✓ | ✓ | ✗ |
| 删除数据 | ✓ | ✓ | ✓ | ✓ | ✗ |
| 查询数据 | ✓ | ✓ | ✓ | ✓ | ✓ |
| 创建索引 | ✓ | ✓ | ✓ | ✗ | ✗ |
