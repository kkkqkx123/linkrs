# Linkrs CLI 操作清单

本文档整理 Linkrs CLI（`crates/linkrs-cli`）当前支持的完整操作，包括命令行启动参数、交互式 REPL 元命令（`\` 命令）以及可在 REPL/脚本中执行的 GQL 语句。

## 1. 启动与命令行参数

二进制：`linkrs-cli`（crate `linkrs_cli`）。

| 参数 | 短选项 | 说明 | 默认值 |
|------|--------|------|--------|
| `--host` | `-h` | 服务器主机 | `127.0.0.1` |
| `--port` | `-p` | 服务器端口 | `8080` |
| `--user` | `-u` | 认证用户名 | `root` |
| `--password` | `-W` | 交互式提示输入密码 | `false` |
| `--database` | `-d` | 启动后切换到指定空间 | 无 |
| `--command` | `-c` | 执行单条命令并退出 | 无 |
| `--file` | `-f` | 从文件执行命令 | 无 |
| `--output` | `-o` | 查询结果输出文件 | 无 |
| `--format` |  | 输出格式：`table`/`csv`/`json`/`vertical`/`html` | `table` |
| `--quiet` | `-q` | 静默模式，抑制非必要输出 | `false` |
| `--single-transaction` | `-1` | 在单个事务中执行命令 | `false` |
| `--force` |  | 出错后继续处理 | `false` |
| `--variable` | `-v` | 启动前设置变量，格式 `NAME=VALUE`（可重复） | 无 |

示例：

```shell
linkrs-cli --host localhost --port 9758 --user root -W -d my_space
linkrs-cli -c "SHOW SPACES;"
linkrs-cli -f script.gql --force
```

REPL 内还支持直接输入 `exit` / `quit` / `\q` 退出。

## 1.1 服务管理子命令（顶层）

`linkrs-cli` 现支持顶层 `service` 子命令管理本地 `linkrs-server` 进程：

```shell
linkrs-cli service start [--bin PATH] [--config PATH] [--data-dir PATH] [--http-port PORT] [--grpc-port PORT] [--foreground]
linkrs-cli service stop
linkrs-cli service restart
linkrs-cli service status
```

说明：
- 默认后台启动，PID 与日志写入 `~/.linkrs/`。
- `--foreground` 以前台方式运行。
- 可通过 `LINKRS_SERVER_BIN` 指定服务二进制路径。

## 2. 服务端二进制（`linkrs-server`）

根 crate 提供的 `linkrs-server` 二进制支持以下子命令：

| 子命令 | 参数 | 说明 |
|--------|------|------|
| `serve` | `--config <path>` | 加载配置文件 |
|  | `--data-dir <path>` | 数据目录 |
|  | `--http-port <port>` | HTTP 端口 |
|  | `--grpc-port <port>` | gRPC 端口 |
|  | `--log-level <level>` | 日志级别 |
| `query` | `--query <gql>` / `-q` | 直接执行一条查询 |

## 3. 交互式 REPL 元命令（`\` 命令）

所有元命令均以反斜杠 `\` 开头；命令名大小写不敏感。别名见括号。

### 3.1 连接与会话

| 命令 | 说明 |
|------|------|
| `\login <user> [password]` | 登录服务器（密码省略时交互式提示） |
| `\passwd` / `\password` | 修改当前用户密码 |
| `\whoami` | 显示当前用户名、会话与角色 |
| `\show_users` / `\du` | 列出所有用户（管理员） |
| `\users <subcommand>` | 用户管理，见 3.1.1 |
| `\sessions [list\|show\|kill]` | 服务器会话管理（管理员），见 3.1.2 |
| `\connect <space>` / `\c` | 切换到指定图空间 |
| `\disconnect` | 断开当前会话 |
| `\conninfo` | 显示连接信息 |

#### 3.1.1 `\users` 子命令

`list | create <user> <password> | drop <user> | reset-password <user> | enable <user> | disable <user> | grant <user> <role> <space> | revoke <user> <space>`

别名：`list/ls`、`create/add`、`drop/remove/rm`、`reset-password/passwd/reset`、`enable`、`disable`、`grant`、`revoke`。

#### 3.1.2 `\sessions` 子命令

`list`（默认）、`show <id>` / `get <id>`、`kill <id>` / `terminate <id>`。

### 3.2 对象查看

| 命令 | 说明 |
|------|------|
| `\show_spaces` / `\l` | 列出所有图空间 |
| `\show_tags [pattern]` / `\dt` | 列出所有 Tag（顶点类型） |
| `\show_edges [pattern]` / `\de` | 列出所有边类型 |
| `\show_functions` / `\df` | 列出所有函数 |
| `\functions <subcommand>` | 函数管理，见 3.2.1 |
| `\schema <subcommand>` | Schema 版本历史，见 3.2.2 |
| `\describe <tag>` / `\d` | 描述 Tag 结构 |
| `\describe_edge <edge>` | 描述边类型结构 |

#### 3.2.1 `\functions` 子命令

`list | info <name> | register <name> <impl_path> | unregister <name>`

别名：`list/ls`、`info/show/describe`、`register/add`、`unregister/remove/rm/drop`。

#### 3.2.2 `\schema` 子命令

- `\schema versions <space> <label> [edge]`（别名 `history`）
- `\schema changes <space> <label> <from> <to> [edge]`
- `\schema breaking <space> <label> <from> <to> [edge]`（别名 `breaking-changes`）

### 3.3 输出格式

| 命令 | 说明 |
|------|------|
| `\format <fmt>` | 设置输出格式：`table`/`csv`/`json`/`vertical`/`html` |
| `\pager [cmd]` | 设置或关闭分页器 |
| `\timing` | 切换执行时间显示 |
| `\x` | 切换展开/垂直显示 |

### 3.4 变量

| 命令 | 说明 |
|------|------|
| `\set [name [value]]` | 设置变量或显示变量（无参数显示全部） |
| `\unset <name>` | 删除变量 |

变量可在查询中使用：`:name`（值）、`:'name'`（字符串）。特殊变量：`ON_ERROR_STOP`、`ECHO`、`TIMING`、`EDITOR`、`FORMAT`、`HISTSIZE`、`AUTOCOMMIT`。

### 3.5 脚本与 I/O

| 命令 | 说明 |
|------|------|
| `\i <file>` | 从文件执行命令（支持变量替换） |
| `\ir <file>` | 从文件执行命令（原始模式，无替换） |
| `\o [file]` | 重定向输出到文件（无参数关闭） |
| `\! <command>` | 执行 Shell 命令 |
| `\import <fmt> <file> <tag\|edge> <name> [batch_size]` | 导入数据（格式 `csv`/`json`/`jsonl`） |
| `\export <fmt> <file> <query> [--stream] [--chunk-size n]` | 导出查询结果（格式 `csv`/`json`/`jsonl`） |
| `\copy <target> from\|to <file> [--stream] [--chunk-size n]` | 在文件与表之间复制数据 |
| `\export-space <space> <path> [--format csv\|json\|jsonl] [--tags t1,t2] [--edges e1,e2]` | 导出整个空间数据 |
| `\export-schema <path> [--format json\|yaml]` | 导出 Schema 定义 |
| `\import-schema <file>` | 从文件导入 Schema 定义 |
| `\batch <subcommand>` | 服务器批处理任务，见 3.5.1 |
| `\fulltext <subcommand>` | 全文索引管理与搜索，见 3.5.2 |
| `\vector <subcommand>` | 向量索引管理与搜索，见 3.5.3 |
| `\validate <query>` | 校验 GQL 语句但不执行 |
| `\sync` / `\outbox <subcommand>` | 同步/Outbox 管理，见 3.5.4 |
| `\explain [analyze] <query>` | 解释查询计划 |
| `\profile <query>` | 查询性能分析 |

#### 3.5.1 `\batch` 子命令

`create <space_id> <vertex|edge|mixed> [batch_size] | add <batch_id> <json_file> | execute <batch_id> | status <batch_id> | cancel <batch_id> | delete <batch_id>`

别名：`execute/exec`、`delete/remove/rm`。

#### 3.5.2 `\fulltext` 子命令

`search <space_id> <tag> <field> <query> [limit] | indexes | inconsistent | rebuild <space_id> <tag> <field> | status <rebuild_id> | clear <space_id> <tag> <field>`

别名：`indexes/list`。

#### 3.5.3 `\vector` 子命令

`create <space_id> <tag> <field> <vector_size> | indexes | info <space_id> <tag> <field> | drop <space_id> <tag> <field> | search <space_id> <tag> <field> <v1,v2,...> [limit] | get <space_id> <tag> <field> <point_id> | scroll <space_id> <tag> <field> [limit] [offset] | count <space_id> <tag> <field> | set-payload <space_id> <tag> <field> <id1,id2> <json> | merge-payload <space_id> <tag> <field> <id1,id2> <json> | delete-payload <space_id> <tag> <field> <id1,id2> <key1,key2> | rebuild <space_id> <tag> <field> | status <rebuild_id> | clear <space_id> <tag> <field>`

别名：`indexes/list`。

#### 3.5.4 `\sync` / `\outbox` 子命令

`status | diagnostics | dead-letters [target] [index_id] [generation] [limit] [offset] | requeue [target] [index_id] [generation] [limit] | retry | degraded [target] [index_id] [generation] | clear <target> <index_id> <generation> <start_lsn> <end_lsn> | retention-status | retention-run [grace_lsn_distance] [max_age_ms]`

别名：`diagnostics/diag`、`dead-letters/dead_letters/dlq`、`degraded/degraded-ranges/ranges`、`retention-status/retention_status`、`retention-run/retention_run/retention`。

### 3.6 查询缓冲区

| 命令 | 说明 |
|------|------|
| `\e [file] [+line]` / `\edit` | 在外部编辑器中编辑查询缓冲区 |
| `\p` | 打印当前查询缓冲区 |
| `\r` | 清空查询缓冲区 |
| `\w <file>` | 将查询缓冲区写入文件 |

### 3.7 历史

| 命令 | 说明 |
|------|------|
| `\history [N]` | 显示最近 N 条历史（默认 20） |
| `\history search <pattern>` | 搜索历史 |
| `\history exec <id>` | 重新执行指定历史条目 |
| `\history clear` | 清空历史 |

历史保存在 `~/.linkrs/cli_history`。

### 3.8 事务

| 命令 | 说明 |
|------|------|
| `\begin` | 开始事务 |
| `\commit` | 提交当前事务 |
| `\rollback` | 回滚当前事务 |
| `\rollback to <savepoint>` | 回滚到保存点 |
| `\autocommit [on\|off]` | 自动提交模式 |
| `\isolation [level]` | 设置事务隔离级别 |
| `\savepoint <name>` | 创建保存点 |
| `\release <name>` | 释放保存点 |
| `\txstatus` | 显示事务状态 |

### 3.9 流式与游标

| 命令 | 说明 |
|------|------|
| `\stream <query>` | 以流式模式（SSE）执行查询 |
| `\cursor open <query>` | 打开前向游标 |
| `\cursor fetch [id] [n]` | 获取下一页（默认 500 行） |
| `\cursor close [id]` | 关闭游标 |

### 3.10 可观测性

| 命令 | 说明 |
|------|------|
| `\statistics [target]` | 显示统计信息（`queries`/`database`/`system`/`overview`/`session`/`search`/`freeze`/`migration`/`profile <trace_id>`；`freeze trigger`/`freeze run`） |
| `\status` | 数据库概览（`\statistics overview` 的别名） |

### 3.11 配置

| 命令 | 说明 |
|------|------|
| `\config` / `\config show [section]` | 显示配置（全部或某节） |
| `\config get <section> <key>` | 获取单个配置项 |
| `\config set <section> <key> <value>` | 更新配置项 |
| `\config reset <section> <key>` | 重置配置项为默认值 |

### 3.12 条件执行

| 命令 | 说明 |
|------|------|
| `\if <condition>` | 开始条件块 |
| `\elif <condition>` | 否则如果分支 |
| `\else` | 否则分支 |
| `\endif` | 结束条件块 |

条件：`VAR`、`?VAR`、`!?VAR`、`VAR == VALUE`、`VAR != VALUE`。

### 3.13 通用

| 命令 | 说明 |
|------|------|
| `\help [topic]` / `\?` | 显示帮助（GQL 主题：`match`、`go`、`insert`、`create`、`show`、`use`、`export`、`import`、`copy`、`export-space`、`export-schema`、`import-schema`、`variables`、`if`、`history`、`stream`、`cursor`、`statistics`、`config`、`edit`、`fulltext`、`vector`） |
| `\version` | 显示版本信息 |
| `\copyright` | 显示版权信息 |
| `\q` / `\quit` | 退出 CLI |
| `\q!` | 强制退出（回滚活跃事务） |

## 4. GQL 语句（可在 CLI 中执行）

以下为查询引擎支持的语句关键字，按类别列出。GQL 关键字大小写不敏感，语句以分号 `;` 结束（多行输入会自动等待补全）。

### 4.1 数据查询（DQL）

`MATCH`、`GO`、`FETCH`、`LOOKUP`、`SUBGRAPH`、`FIND PATH`、`GROUP BY`、`UNWIND`、`RETURN`、`WITH`、`YIELD`、`WHERE`（`FILTER`）、`COLLECT`、`LET`（`AssignVariable`）、`LOAD FROM`、`CALL`（`InQueryCall`）。

### 4.2 数据写入（DML）

`INSERT`、`UPDATE` / `UPSERT`、`DELETE`、`MERGE`、`SET`、`REMOVE`、`COPY`。

### 4.3 Schema（DDL）

`CREATE`、`DROP`、`ALTER`、`DESC`、`SHOW CREATE`、`CREATE FULLTEXT INDEX`、`DROP FULLTEXT INDEX`、`ALTER FULLTEXT INDEX`、`CREATE VECTOR INDEX`、`DROP VECTOR INDEX`、`CREATE TYPE`、`DROP TYPE`、`CREATE MACRO`、`DROP MACRO`、`CLEAR SPACE`、`PURGE EXPIRED`。

### 4.4 权限控制（DCL）

`CREATE USER`、`ALTER USER`、`DROP USER`、`CHANGE PASSWORD`、`GRANT`、`REVOKE`、`DESCRIBE USER`、`SHOW USERS`、`SHOW ROLES`。

### 4.5 管理（Admin）

`USE`、`SHOW`、`EXPLAIN`、`PROFILE`、`ANALYZE`、`SHOW SESSIONS`、`SHOW QUERIES`、`KILL QUERY`、`SHOW CONFIGS`、`UPDATE CONFIGS`、`CHECKPOINT`、`EXPORT DATABASE`、`IMPORT DATABASE`、`LOAD EXTENSION` / `INSTALL EXTENSION` / `UNINSTALL EXTENSION` / `UPDATE EXTENSION`、`ATTACH DATABASE`、`DETACH DATABASE`。

### 4.6 搜索（Search）

`SEARCH`、`LOOKUP FULLTEXT`、`MATCH FULLTEXT`、`SEARCH VECTOR`、`LOOKUP VECTOR`、`MATCH VECTOR`、`SHOW FULLTEXT INDEX`、`DESCRIBE FULLTEXT INDEX`、`SHOW VECTOR INDEX`、`DESCRIBE VECTOR INDEX`。

### 4.7 事务

`BEGIN TRANSACTION`（可选 `READ ONLY` / `READ WRITE`）、`COMMIT TRANSACTION`、`ROLLBACK TRANSACTION`（可选 `TO SAVEPOINT`）、`SAVEPOINT`、`RELEASE SAVEPOINT`。

### 4.8 组合与控制流

`PIPE`（`|`）、集合操作（`SET OPERATION`）、`ASSIGNMENT`。

## 5. 输出格式

支持：`table`（默认）、`csv`、`json`、`vertical`、`html`。可通过 `--format`、`\format` 或 `\set FORMAT` 设置。

## 6. 脚本与条件执行

- 脚本文件通过 `\i` / `\ir` 或 `-f` 执行。
- 支持 `\if` / `\elif` / `\else` / `\endif` 条件块。
- 支持 `\set` 变量与 `:var` / `:'var'` 替换。
- 支持 `\o` 输出重定向与 `\!` Shell 命令。

## 7. 常用示例

```sql
SHOW SPACES;
USE my_graph;
SHOW TAGS;
MATCH (p:person) RETURN p.name, p.age LIMIT 10;
GO 1 STEPS FROM "person1" OVER friend YIELD friend.name;
INSERT VERTEX person(name, age) VALUES "p1":("Alice", 30);
CREATE SPACE my_space (vid_type=STRING);
```

```shell
\connect my_graph
\format json
\timing
\export csv out.csv 'MATCH (p:person) RETURN p.name, p.age'
\import csv data.csv tag person 100
\history
\q
```
