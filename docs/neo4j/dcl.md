# Neo4j Cypher DCL - 数据控制语言

## 概述

DCL（Data Control Language）用于管理数据库访问权限和安全性，包括用户管理、角色管理和权限控制。Neo4j 的 DCL 功能主要在 Enterprise Edition 中提供。

---

## 1. 用户管理

### 1.1 CREATE USER - 创建用户

```cypher
// 创建用户（需要设置初始密码）
CREATE USER alice SET PASSWORD 'securePassword123'

// 创建用户并设置必须更改密码
CREATE USER bob SET PASSWORD 'tempPassword' SET PASSWORD CHANGE REQUIRED

// 创建用户并设置为非活动状态
CREATE USER charlie SET PASSWORD 'password' SET ACCOUNT DISABLED

// 创建用户并设置过期时间
CREATE USER dave SET PASSWORD 'password' SET ACCOUNT EXPIRE '2026-12-31T23:59:59Z'
```

### 1.2 ALTER USER - 修改用户

```cypher
// 修改用户密码
ALTER USER alice SET PASSWORD 'newSecurePassword456'

// 强制用户更改密码
ALTER USER bob SET PASSWORD CHANGE REQUIRED

// 启用/禁用用户账户
ALTER USER charlie SET ACCOUNT ACTIVE
ALTER USER charlie SET ACCOUNT DISABLED

// 修改用户过期时间
ALTER USER dave SET ACCOUNT EXPIRE '2027-12-31T23:59:59Z'

// 移除账户过期
ALTER USER dave SET ACCOUNT NO EXPIRE

// 修改用户显示名称
ALTER USER alice SET DISPLAY NAME 'Alice Smith'

// 设置用户邮箱
ALTER USER alice SET EMAIL 'alice@example.com'
```

### 1.3 DROP USER - 删除用户

```cypher
// 删除用户
DROP USER alice

// 如果存在则删除
DROP USER alice IF EXISTS
```

### 1.4 SHOW USERS - 显示用户

```cypher
// 显示所有用户
SHOW USERS

// 显示特定用户
SHOW USER alice

// 显示当前用户
SHOW CURRENT USER

// 显示用户详情
SHOW USERS YIELD user, roles, suspended, homeDatabase
RETURN *

// 显示用户的角色
SHOW USER alice ROLES
```

---

## 2. 角色管理

### 2.1 CREATE ROLE - 创建角色

```cypher
// 创建角色
CREATE ROLE developer

// 创建角色并描述
CREATE ROLE admin DESCRIPTION 'Database administrators'
```

### 2.2 ALTER ROLE - 修改角色

```cypher
// 修改角色描述
ALTER ROLE admin SET DESCRIPTION 'Full database administrators'
```

### 2.3 DROP ROLE - 删除角色

```cypher
// 删除角色
DROP ROLE developer

// 如果存在则删除
DROP ROLE developer IF EXISTS
```

### 2.4 SHOW ROLES - 显示角色

```cypher
// 显示所有角色
SHOW ROLES

// 显示特定角色
SHOW ROLE admin

// 显示角色的权限
SHOW ROLE admin PRIVILEGES

// 显示角色的用户
SHOW ROLE admin USERS
```

### 2.5 角色分配

```cypher
// 给用户分配角色
GRANT ROLE developer TO alice

// 给用户分配多个角色
GRANT ROLE developer, analyst TO bob

// 从用户移除角色
REVOKE ROLE developer FROM alice

// 设置用户的主要角色
SET USER alice DEFAULT ROLE developer
```

---

## 3. 权限管理

### 3.1 GRANT - 授权权限

#### 3.1.1 数据库级别权限

```cypher
// 授予数据库访问权限
GRANT ACCESS ON DATABASE neo4j TO developer

// 授予数据库读取权限
GRANT READ ON DATABASE neo4j TO analyst

// 授予数据库写入权限
GRANT WRITE ON DATABASE neo4j TO developer

// 授予数据库完全权限
GRANT ALL ON DATABASE neo4j TO admin
```

#### 3.1.2 图级别权限

```cypher
// 授予图遍历权限
GRANT TRAVERSE ON GRAPH neo4j TO reader

// 授予图读取权限
GRANT READ ON GRAPH neo4j TO analyst

// 授予图写入权限
GRANT WRITE ON GRAPH neo4j TO developer

// 授予图模式权限
GRANT MATCH ON GRAPH neo4j TO analyst

// 授予图完全权限
GRANT ALL ON GRAPH neo4j TO admin
```

#### 3.1.3 标签级别权限

```cypher
// 授予特定标签的读取权限
GRANT READ {*} ON GRAPH neo4j FOR (n:Person) TO hr_role

// 授予特定标签的遍历权限
GRANT TRAVERSE ON GRAPH neo4j FOR (n:Employee) TO manager_role

// 授予特定标签的写入权限
GRANT WRITE ON GRAPH neo4j FOR (n:Customer) TO sales_role
```

#### 3.1.4 关系类型权限

```cypher
// 授予特定关系类型的权限
GRANT READ ON GRAPH neo4j FOR ()-[r:FRIENDS_WITH]-() TO analyst

GRANT TRAVERSE ON GRAPH neo4j FOR ()-[r:MANAGES]-() TO manager_role
```

#### 3.1.5 属性级别权限

```cypher
// 授予读取特定属性的权限
GRANT READ (p.name) ON GRAPH neo4j FOR (p:Person) TO reader

// 授予读取多个属性的权限
GRANT READ (p.name, p.email) ON GRAPH neo4j FOR (p:Person) TO hr_role

// 授予所有属性的读取权限
GRANT READ {*} ON GRAPH neo4j FOR (p:Person) TO analyst
```

#### 3.1.6 基于条件的权限

```cypher
// 基于节点属性值的权限
GRANT READ {*} ON GRAPH * FOR (n) WHERE n.securityLevel > 3 TO regularUsers

// 基于空值的权限
GRANT TRAVERSE ON GRAPH * FOR (n:Email) WHERE n.classification IS NULL TO regularUsers

// 拒绝访问特定分类的数据
DENY MATCH {*} ON GRAPH * FOR (n) WHERE n.classification <> 'UNCLASSIFIED' TO regularUsers
```

#### 3.1.7 过程执行权限

```cypher
// 授予执行存储过程的权限
GRANT EXECUTE ON PROCEDURE db.labels TO analyst
GRANT EXECUTE ON PROCEDURE apoc.* TO developer

// 授予执行所有内置过程的权限
GRANT EXECUTE ON BUILT IN PROCEDURE * TO admin
```

#### 3.1.8 函数调用权限

```cypher
// 授予调用函数的权限
GRANT CALL ON FUNCTION * TO developer
```

#### 3.1.9 LOAD 权限

```cypher
// 授予数据加载权限
GRANT LOAD ON ALL DATA TO etl_role

// 授予特定 CIDR 范围的 LOAD 权限
GRANT LOAD ON CIDR "127.0.0.1/32" TO etl_role
```

### 3.2 DENY - 拒绝权限

```cypher
// 拒绝数据库访问
DENY ACCESS ON DATABASE neo4j TO guest

// 拒绝图遍历
DENY TRAVERSE ON GRAPH neo4j TO external_user

// 拒绝特定标签的访问
DENY MATCH ON GRAPH neo4j FOR (n:Sensitive) TO regularUsers

// 拒绝属性访问
DENY READ (p.ssn) ON GRAPH neo4j FOR (p:Person) TO all_users

// 拒绝过程执行
DENY EXECUTE ON PROCEDURE apoc.export.* TO regularUsers

// 拒绝 LOAD 操作
DENY LOAD ON ALL DATA TO readonly_role

// 拒绝特定 CIDR 范围的访问
DENY LOAD ON CIDR "::1/128" TO restricted_role
```

### 3.3 REVOKE - 撤销权限

```cypher
// 撤销已授予的权限
REVOKE GRANT ACCESS ON DATABASE neo4j FROM guest

// 撤销已拒绝的权限
REVOKE DENY TRAVERSE ON GRAPH neo4j FROM external_user

// 撤销角色
REVOKE ROLE developer FROM alice

// 仅撤销不可变权限
REVOKE IMMUTABLE GRANT ACCESS ON DATABASE neo4j FROM guest

// 撤销所有权限
REVOKE ALL PRIVILEGES FROM alice
```

---

## 4. 权限查询

### 4.1 SHOW PRIVILEGES - 显示权限

```cypher
// 显示所有权限
SHOW PRIVILEGES

// 显示特定用户的权限
SHOW USER alice PRIVILEGES

// 显示特定角色的权限
SHOW ROLE developer PRIVILEGES

// 显示当前用户的权限
SHOW CURRENT USER PRIVILEGES

// 按条件过滤权限
SHOW PRIVILEGES
YIELD grantee, action, access, segment
WHERE grantee = 'alice'
RETURN *
```

### 4.2 SHOW SUPPORTED PRIVILEGES - 显示支持的权限

```cypher
// 显示所有支持的权限类型
SHOW SUPPORTED PRIVILEGES

// 显示支持的权限详情
SHOW SUPPORTED PRIVILEGES
YIELD privilege, description
RETURN *
```

---

## 5. 安全策略

### 5.1 密码策略

```cypher
// 设置全局密码策略
ALTER PASSWORD POLICY DEFAULT
SET MIN LENGTH 12
SET MAX LENGTH 128
SET REQUIRE DIGIT true
SET REQUIRE SPECIAL CHARACTER true
SET MAX AGE 90 DAYS
SET MAX HISTORY 5

// 为用户设置特定密码策略
ALTER USER alice SET PASSWORD POLICY strict_policy
```

### 5.2 登录策略

```cypher
// 设置登录失败锁定
ALTER USER alice SET FAILED LOGIN ATTEMPTS 5 LOCKOUT 30 MINUTES

// 设置登录时间限制
ALTER USER bob SET ALLOWED LOGIN HOURS '09:00-18:00'

// 设置允许的登录来源
ALTER USER charlie SET ALLOWED LOGIN FROM '192.168.1.0/24'
```

---

## 6. 审计和监控

### 6.1 审计日志

```cypher
// 启用审计日志（需要在配置文件中设置）
// dbms.security.auth_enabled=true
// dbms.security.procedures.unrestricted=audit.*

// 查询审计日志（需要审计插件）
CALL audit.log()
YIELD action, user, timestamp
RETURN *
```

### 6.2 安全监控

```cypher
// 显示登录历史
SHOW LOGIN HISTORY FOR USER alice

// 显示失败的登录尝试
SHOW FAILED LOGIN ATTEMPTS

// 显示当前活动会话
SHOW SESSIONS
```

---

## 7. 多租户管理

### 7.1 数据库隔离

```cypher
// 为不同租户创建独立数据库
CREATE DATABASE tenant1_db
CREATE DATABASE tenant2_db

// 创建租户管理员角色
CREATE ROLE tenant1_admin
CREATE ROLE tenant2_admin

// 授予租户管理员完全权限
GRANT ALL ON DATABASE tenant1_db TO tenant1_admin
GRANT ALL ON DATABASE tenant2_db TO tenant2_admin

// 创建租户用户角色
CREATE ROLE tenant1_user
CREATE ROLE tenant2_user

// 授予租户用户读取权限
GRANT READ ON DATABASE tenant1_db TO tenant1_user
GRANT READ ON DATABASE tenant2_db TO tenant2_user
```

### 7.2 跨数据库访问控制

```cypher
// 允许跨数据库访问
GRANT ACCESS ON DATABASE tenant1_db TO shared_service_role
GRANT ACCESS ON DATABASE tenant2_db TO shared_service_role

// 限制跨数据库写入
DENY WRITE ON DATABASE tenant1_db TO readonly_integration
DENY WRITE ON DATABASE tenant2_db TO readonly_integration
```

---

## 8. 最佳实践

### 8.1 最小权限原则

```cypher
// 1. 创建细粒度角色
CREATE ROLE read_only
CREATE ROLE data_entry
CREATE ROLE data_analyst
CREATE ROLE admin

// 2. 授予最小必要权限
GRANT READ ON GRAPH neo4j TO read_only
GRANT WRITE ON GRAPH neo4j FOR (n:InputData) TO data_entry
GRANT MATCH, READ ON GRAPH neo4j TO data_analyst
GRANT ALL ON DATABASE neo4j TO admin

// 3. 基于角色分配用户
GRANT ROLE read_only TO intern_user
GRANT ROLE data_entry TO entry_user
GRANT ROLE data_analyst TO analyst_user
GRANT ROLE admin TO admin_user
```

### 8.2 敏感数据保护

```cypher
// 1. 为敏感数据创建特殊标签
CREATE CONSTRAINT FOR (p:Person) REQUIRE p.ssn IS NOT NULL

// 2. 限制敏感属性访问
DENY READ (p.ssn, p.salary) ON GRAPH neo4j FOR (p:Person) TO regularUsers

// 3. 仅授权特定角色访问
GRANT READ {*} ON GRAPH neo4j FOR (p:Person) TO hr_admins
```

### 8.3 定期权限审查

```cypher
// 1. 定期查看权限分配
SHOW PRIVILEGES
YIELD grantee, action, access, segment
RETURN grantee, count(*) AS privilegeCount
ORDER BY privilegeCount DESC

// 2. 查看未使用的角色
SHOW ROLES
YIELD role
WHERE NOT EXISTS((role)<-[:HAS_ROLE]-())
RETURN role

// 3. 查看长期未登录的用户
SHOW USERS
YIELD user, suspended, lastLogin
WHERE lastLogin < datetime() - duration({days: 90})
RETURN user, lastLogin
```

---

## 9. 安全配置示例

### 9.1 开发环境配置

```cypher
// 创建开发团队角色
CREATE ROLE developer
CREATE ROLE tester

// 授予开发人员完全访问权限
GRANT ALL ON DATABASE neo4j TO developer

// 授予测试人员读写权限
GRANT READ, WRITE ON DATABASE neo4j TO tester

// 允许执行开发相关的过程
GRANT EXECUTE ON PROCEDURE apoc.* TO developer
```

### 9.2 生产环境配置

```cypher
// 创建生产环境角色
CREATE ROLE prod_admin
CREATE ROLE prod_developer
CREATE ROLE prod_readonly
CREATE ROLE prod_service

// 严格限制生产环境权限
GRANT ALL ON DATABASE prod TO prod_admin
GRANT READ ON DATABASE prod TO prod_readonly
GRANT MATCH ON GRAPH prod FOR (n:Config) TO prod_developer
GRANT EXECUTE ON PROCEDURE db.* TO prod_service

// 拒绝危险操作
DENY EXECUTE ON PROCEDURE apoc.export.* TO prod_developer
DENY DROP CONSTRAINT TO prod_developer
DENY DROP INDEX TO prod_developer
```

---

## 参考文档

- [Neo4j Cypher Manual 25 - Security](https://neo4j.com/docs/cypher-manual/25/security/)
- [Neo4j Operations Manual - Authentication and Authorization](https://neo4j.com/docs/operations-manual/current/authentication-authorization/)
- [Neo4j Cypher Manual 25 - GRANT](https://neo4j.com/docs/cypher-manual/25/clauses/grant/)
- [Neo4j Cypher Manual 25 - DENY](https://neo4j.com/docs/cypher-manual/25/clauses/deny/)
- [Neo4j Cypher Manual 25 - REVOKE](https://neo4j.com/docs/cypher-manual/25/clauses/revoke/)
