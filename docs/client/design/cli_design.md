# GraphDB CLI 客户端设计方案

## 1. 概述

### 1.1 设计目标

为 GraphDB 设计一个类似 PostgreSQL `psql` 的命令行交互式客户端工具，提供以下核心能力：

- 交互式图查询语言（GQL）执行
- 数据库管理和元数据查看
- 友好的用户体验（自动补全、历史记录、格式化输出）
- 脚本执行和批处理支持
- 连接管理和会话控制

### 1.2 设计原则

1. **用户友好**：提供直观的交互界面和清晰的帮助信息
2. **功能完整**：覆盖数据库管理、查询执行、结果查看等核心场景
3. **扩展性强**：模块化设计，便于添加新功能
4. **性能优先**：快速响应，支持大数据集展示
5. **兼容性**：与现有 GraphDB API 层无缝集成

## 2. 架构设计

### 2.1 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│                      GraphDB CLI Client                      │
├─────────────────────────────────────────────────────────────┤
│                                                               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │
│  │   Input      │  │   Command    │  │   Output     │      │
│  │   Handler    │  │   Processor  │  │   Formatter  │      │
│  └──────────────┘  └──────────────┘  └──────────────┘      │
│         │                  │                  │              │
│         └──────────────────┴──────────────────┘              │
│                            │                                  │
│                   ┌────────▼────────┐                        │
│                   │  Session Manager │                        │
│                   └────────┬────────┘                        │
│                            │                                  │
│                   ┌────────▼────────┐                        │
│                   │   API Client    │                        │
│                   │  (HTTP/Embedded)│                        │
│                   └────────┬────────┘                        │
│                            │                                  │
└────────────────────────────┼──────────────────────────────────┘
                             │
                    ┌────────▼────────┐
                    │  GraphDB Server │
                    │   (HTTP API)    │
                    └─────────────────┘
```

### 2.2 核心模块

#### 2.2.1 输入处理模块（Input Handler）

**职责**：
- 读取用户输入（单行/多行）
- 处理特殊字符和转义序列
- 管理输入缓冲区
- 支持多行查询编辑

**技术选型**：
- 使用 `rustyline` crate 提供行编辑功能
- 支持历史记录和搜索
- 支持 Tab 自动补全

**关键功能**：
```rust
pub trait InputHandler {
    fn read_line(&mut self, prompt: &str) -> Result<String>;
    fn load_history(&mut self, path: &Path) -> Result<()>;
    fn save_history(&mut self, path: &Path) -> Result<()>;
    fn set_completer(&mut self, completer: Box<dyn Completer>);
}
```

#### 2.2.2 命令处理模块（Command Processor）

**职责**：
- 解析用户输入（GQL 查询或元命令）
- 区分查询语句和元命令
- 执行相应的处理逻辑

**命令分类**：

1. **GQL 查询语句**
   - DQL: MATCH, GO, LOOKUP, FETCH
   - DML: INSERT, UPDATE, DELETE
   - DDL: CREATE, ALTER, DROP
   - DCL: GRANT, REVOKE

2. **元命令（Meta-commands）**
   - 连接管理: `\connect`, `\disconnect`
   - 对象查看: `\show_spaces`, `\show_tags`, `\show_edges`
   - 格式控制: `\format`, `\pager`
   - 变量管理: `\set`, `\unset`
   - 脚本执行: `\i`, `\ir`
   - 帮助信息: `\?`, `\help`
   - 退出: `\q`, `\quit`

**关键接口**：
```rust
pub enum Command {
    Query(String),
    MetaCommand(MetaCommand),
}

pub enum MetaCommand {
    Connect { space: String },
    ShowSpaces,
    ShowTags { pattern: Option<String> },
    ShowEdges { pattern: Option<String> },
    Format { format: OutputFormat },
    Set { name: String, value: String },
    ExecuteScript { path: String },
    Help { topic: Option<String> },
    Quit,
}

pub trait CommandProcessor {
    fn parse(&self, input: &str) -> Result<Command>;
    fn execute(&mut self, command: Command) -> Result<()>;
}
```

#### 2.2.3 输出格式化模块（Output Formatter）

**职责**：
- 格式化查询结果
- 支持多种输出格式
- 处理分页显示
- 支持结果导出

**支持的输出格式**：

1. **表格格式（Table）**：默认格式，对齐显示
   ```
   ┌─────────┬──────┬─────────┐
   │ name    │ age  │ email   │
   ├─────────┼──────┼─────────┤
   │ Alice   │ 30   │ a@b.com │
   │ Bob     │ 25   │ b@c.com │
   └─────────┴──────┴─────────┘
   ```

2. **垂直格式（Vertical）**：适合宽表
   ```
   -[ RECORD 1 ]-
   name  | Alice
   age   | 30
   email | a@b.com
   -[ RECORD 2 ]-
   name  | Bob
   age   | 25
   email | b@c.com
   ```

3. **CSV 格式**：便于导入导出
   ```
   name,age,email
   Alice,30,a@b.com
   Bob,25,b@c.com
   ```

4. **JSON 格式**：结构化输出
   ```json
   [
     {"name": "Alice", "age": 30, "email": "a@b.com"},
     {"name": "Bob", "age": 25, "email": "b@c.com"}
   ]
   ```

5. **HTML 格式**：网页展示

**关键接口**：
```rust
pub enum OutputFormat {
    Table,
    Vertical,
    CSV,
    JSON,
    HTML,
}

pub trait OutputFormatter {
    fn format(&self, result: &QueryResult, format: OutputFormat) -> String;
    fn format_error(&self, error: &Error) -> String;
    fn set_pager(&mut self, pager: Option<String>);
}
```

#### 2.2.4 会话管理模块（Session Manager）

**职责**：
- 管理与服务器的连接
- 维护会话状态
- 处理认证和授权
- 管理当前 Space（图空间）

**会话状态**：
```rust
pub struct Session {
    pub session_id: String,
    pub username: String,
    pub current_space: Option<String>,
    pub connection_info: ConnectionInfo,
    pub variables: HashMap<String, String>,
    pub created_at: DateTime<Utc>,
}

pub struct ConnectionInfo {
    pub host: String,
    pub port: u16,
    pub connected: bool,
}
```

**关键接口**：
```rust
pub trait SessionManager {
    fn connect(&mut self, host: &str, port: u16, username: &str, password: &str) -> Result<Session>;
    fn disconnect(&mut self) -> Result<()>;
    fn switch_space(&mut self, space: &str) -> Result<()>;
    fn get_current_space(&self) -> Option<&str>;
    fn set_variable(&mut self, name: String, value: String);
    fn get_variable(&self, name: &str) -> Option<&String>;
}
```

#### 2.2.5 API 客户端模块（API Client）

**职责**：
- 与 GraphDB 服务器通信
- 支持多种连接方式（HTTP、Embedded）
- 处理请求和响应
- 错误处理和重试

**连接方式**：

1. **HTTP 连接**：通过 HTTP API 连接服务器
   - 适合远程连接
   - 支持认证
   - 支持并发请求

2. **Embedded 连接**：直接嵌入数据库实例
   - 适合本地单机使用
   - 无网络开销
   - 更高性能

**关键接口**：
```rust
pub enum ConnectionMode {
    HTTP { base_url: String },
    Embedded { data_path: String },
}

pub trait GraphDBClient {
    async fn execute_query(&self, query: &str, session: &Session) -> Result<QueryResult>;
    async fn execute_batch(&self, queries: Vec<&str>, session: &Session) -> Result<Vec<QueryResult>>;
    async fn get_schema(&self, space: &str) -> Result<SchemaInfo>;
    async fn health_check(&self) -> Result<bool>;
}
```

### 2.3 自动补全模块（Auto-completion）

**职责**：
- 提供上下文相关的自动补全
- 补全关键字、函数名、对象名

**补全类型**：

1. **关键字补全**：MATCH, GO, LOOKUP, CREATE 等
2. **函数名补全**：内置函数（count, sum, avg 等）
3. **对象名补全**：Space、Tag、Edge、属性名
4. **变量补全**：用户定义的变量

**实现方式**：
```rust
pub struct Completer {
    keywords: Vec<String>,
    functions: Vec<String>,
    schema_cache: Arc<RwLock<SchemaCache>>,
}

impl rustyline::completion::Completer for Completer {
    fn complete(&self, line: &str, pos: usize) -> rustyline::Result<(usize, Vec<String>)> {
        // 实现补全逻辑
    }
}
```

## 3. 功能设计

### 3.1 连接管理

#### 3.1.1 启动连接

**命令行参数**：
```bash
graphdb-cli [options] [space_name]

Options:
  -h, --host <host>          Server host (default: 127.0.0.1)
  -p, --port <port>          Server port (default: 8080)
  -u, --user <username>      Username (default: root)
  -W, --password             Prompt for password
  -d, --database <space>     Space name to connect
  -f, --file <file>          Execute commands from file
  -c, --command <command>    Execute single command
  -o, --output <file>        Output file
  -q, --quiet                Quiet mode
  --format <format>          Output format (table, csv, json, etc.)
  --no-readline              Disable readline features
```

**示例**：
```bash
# 交互式连接
graphdb-cli -h 192.168.1.100 -p 8080 -u admin -W

# 执行单个查询
graphdb-cli -c "MATCH (p:Person) RETURN p.name LIMIT 10"

# 执行脚本文件
graphdb-cli -f queries.gql

# 指定输出格式
graphdb-cli --format json -c "SHOW SPACES"
```

#### 3.1.2 会话内连接

**元命令**：
```
\connect [space_name]          # 连接到指定 Space
\disconnect                    # 断开当前连接
\conninfo                      # 显示连接信息
```

### 3.2 查询执行

#### 3.2.1 交互式查询

**基本流程**：
1. 用户输入查询语句
2. CLI 解析并发送到服务器
3. 接收结果并格式化显示
4. 显示执行统计信息

**提示符设计**：
```
# 未连接状态
graphdb=#

# 已连接但未选择 Space
graphdb(admin)=#

# 已选择 Space
graphdb(admin:mygraph)=#

# 多行输入模式
graphdb(admin:mygraph)->

# 事务模式
graphdb(admin:mygraph[txn])=#
```

#### 3.2.2 多行查询

**支持方式**：
- 自动检测：检测语句是否完整（括号匹配、分号结尾）
- 显式模式：使用 `\e` 进入编辑器

**示例**：
```
graphdb(admin:mygraph)=# MATCH (p:Person)
graphdb(admin:mygraph)-# WHERE p.age > 25
graphdb(admin:mygraph)-# RETURN p.name, p.age
graphdb(admin:mygraph)-# ORDER BY p.age DESC
graphdb(admin:mygraph)-# LIMIT 10;
```

#### 3.2.3 查询结果处理

**结果展示**：
- 自动分页（使用 pager）
- 支持结果集导航
- 支持结果导出

**统计信息**：
```
Execution time: 15ms
Rows returned: 100
Rows scanned: 1000
Cache hit: true
```

### 3.3 元命令设计

#### 3.3.1 对象查看命令

| 命令 | 功能 | 示例 |
|------|------|------|
| `\show_spaces` 或 `\l` | 列出所有图空间 | `\show_spaces` |
| `\show_tags [pattern]` 或 `\dt` | 列出所有 Tag | `\show_tags person*` |
| `\show_edges [pattern]` 或 `\de` | 列出所有 Edge | `\show_edges` |
| `\show_indexes [pattern]` 或 `\di` | 列出所有索引 | `\show_indexes` |
| `\show_users` | 列出所有用户 | `\show_users` |
| `\describe <object>` 或 `\d` | 查看对象详情 | `\describe person` |

**示例输出**：
```
graphdb(admin:mygraph)=# \show_tags

┌─────────────┬──────────┬─────────────┐
│ Tag Name    │ Fields   │ Comment     │
├─────────────┼──────────┼─────────────┤
│ person      │ 5        │ Person tag  │
│ post        │ 3        │ Post tag    │
│ comment     │ 2        │ Comment tag │
└─────────────┴──────────┴─────────────┘

graphdb(admin:mygraph)=# \describe person

Tag: person
┌─────────────┬──────────┬─────────┬─────────┐
│ Field Name  │ Type     │ Nullable│ Default │
├─────────────┼──────────┼─────────┼─────────┤
│ name        │ string   │ NO      │ -       │
│ age         │ int      │ YES     │ -       │
│ email       │ string   │ YES     │ -       │
│ created_at  │ datetime │ YES     │ NOW()   │
│ status      │ bool     │ YES     │ true    │
└─────────────┴──────────┴─────────┴─────────┘
```

#### 3.3.2 格式控制命令

| 命令 | 功能 | 示例 |
|------|------|------|
| `\format <format>` | 设置输出格式 | `\format json` |
| `\pager [command]` | 设置分页器 | `\pager less` |
| `\t` | 切换只显示行模式 | `\t` |
| `\x` | 切换扩展显示模式 | `\x` |

#### 3.3.3 变量管理命令

| 命令 | 功能 | 示例 |
|------|------|------|
| `\set [name [value]]` | 设置或显示变量 | `\set limit 10` |
| `\unset <name>` | 删除变量 | `\unset limit` |

**变量使用**：
```
graphdb(admin:mygraph)=# \set limit 10
graphdb(admin:mygraph)=# MATCH (p:Person) RETURN p.name LIMIT :limit;
```

#### 3.3.4 脚本执行命令

| 命令 | 功能 | 示例 |
|------|------|------|
| `\i <file>` | 执行脚本文件 | `\i queries.gql` |
| `\ir <file>` | 执行脚本文件（相对路径） | `\ir ./test.gql` |
| `\o [file]` | 输出重定向 | `\o result.txt` |
| `\! <command>` | 执行 shell 命令 | `\! ls -la` |

#### 3.3.5 帮助命令

| 命令 | 功能 | 示例 |
|------|------|------|
| `\?` | 显示元命令帮助 | `\?` |
| `\help [command]` | 显示 GQL 命令帮助 | `\help MATCH` |
| `\copyright` | 显示版权信息 | `\copyright` |
| `\version` | 显示版本信息 | `\version` |

### 3.4 事务管理

#### 3.4.1 事务命令

```
\begin              # 开始事务
\commit             # 提交事务
\rollback           # 回滚事务
\status             # 查看事务状态
```

#### 3.4.2 事务模式

**自动提交模式**（默认）：
- 每条语句自动提交
- 适合简单查询

**手动提交模式**：
```
graphdb(admin:mygraph)=# \begin
graphdb(admin:mygraph[txn])=# INSERT VERTEX person(name, age) VALUES "p1":("Alice", 30);
graphdb(admin:mygraph[txn])=# INSERT EDGE follow(degree) VALUES "p1"->"p2":(90);
graphdb(admin:mygraph[txn])=# \commit
Transaction committed successfully.
graphdb(admin:mygraph)=#
```

### 3.5 性能分析

#### 3.5.1 执行计划查看

```
graphdb(admin:mygraph)=# EXPLAIN MATCH (p:Person)-[:FRIEND]->(f) RETURN p, f;

Query Plan:
┌─────────────────────────────────────────────────────────────┐
│ Operator        │ Rows  │ Cost  │ Time    │ Details        │
├─────────────────┼───────┼───────┼─────────┼────────────────┤
│ Project         │ 100   │ 150   │ 0.5ms   │ p, f           │
│ Filter          │ 100   │ 100   │ 0.3ms   │ f.age > 25     │
│ Expand(All)     │ 1000  │ 80    │ 2.0ms   │ FRIEND         │
│ IndexScan       │ 1     │ 10    │ 0.1ms   │ person(name)   │
└─────────────────┴───────┴───────┴─────────┴────────────────┘
```

#### 3.5.2 性能分析命令

```
\timing            # 显示执行时间
\profile <query>   # 显示详细性能分析
```

### 3.6 配置管理

#### 3.6.1 配置文件

**位置**：`~/.graphdb/cli.toml`

**配置项**：
```toml
[connection]
default_host = "127.0.0.1"
default_port = 8080
default_user = "root"

[output]
format = "table"
pager = "less"
max_rows = 1000
null_string = "NULL"

[editor]
command = "vim"
line_number_arg = "+"

[history]
file = "~/.graphdb/history"
max_size = 1000
```

#### 3.6.2 配置命令

```
\config [name [value]]    # 查看或设置配置
\config_file              # 显示配置文件路径
\reload                   # 重新加载配置
```

## 4. 技术实现

### 4.1 技术栈

#### 4.1.1 核心依赖

| Crate | 用途 | 版本 |
|-------|------|------|
| `tokio` | 异步运行时 | 1.x |
| `clap` | 命令行参数解析 | 4.x |
| `rustyline` | 行编辑和历史 | 13.x |
| `reqwest` | HTTP 客户端 | 0.11 |
| `serde` | 序列化/反序列化 | 1.x |
| `serde_json` | JSON 处理 | 1.x |
| `tabled` | 表格格式化 | 0.14 |
| `colored` | 终端颜色 | 2.x |
| `indicatif` | 进度条 | 0.17 |

#### 4.1.2 可选依赖

| Crate | 用途 | 版本 |
|-------|------|------|
| `csv` | CSV 文件处理 | 1.3 |
| `dialoguer` | 交互式提示 | 0.11 |

### 4.2 项目结构

```
graphdb-cli/
├── Cargo.toml
├── src/
│   ├── main.rs                 # 入口点
│   ├── cli.rs                  # CLI 定义
│   ├── lib.rs                  # 库入口
│   ├── input/
│   │   ├── mod.rs
│   │   ├── handler.rs          # 输入处理
│   │   ├── editor.rs           # 编辑器集成
│   │   └── history.rs          # 历史管理
│   ├── command/
│   │   ├── mod.rs
│   │   ├── parser.rs           # 命令解析
│   │   ├── meta_commands.rs    # 元命令实现
│   │   └── executor.rs         # 命令执行
│   ├── output/
│   │   ├── mod.rs
│   │   ├── formatter.rs        # 输出格式化
│   │   ├── table.rs            # 表格格式
│   │   ├── csv.rs              # CSV 格式
│   │   ├── json.rs             # JSON 格式
│   │   └── pager.rs            # 分页器
│   ├── session/
│   │   ├── mod.rs
│   │   ├── manager.rs          # 会话管理
│   │   └── state.rs            # 会话状态
│   ├── client/
│   │   ├── mod.rs
│   │   ├── http.rs             # HTTP 客户端
│   │   └── embedded.rs         # Embedded 客户端
│   ├── completion/
│   │   ├── mod.rs
│   │   ├── completer.rs        # 自动补全
│   │   └── schema_cache.rs     # Schema 缓存
│   ├── config/
│   │   ├── mod.rs
│   │   └── settings.rs         # 配置管理
│   └── utils/
│       ├── mod.rs
│       ├── error.rs            # 错误处理
│       └── display.rs          # 显示工具
└── tests/
    ├── integration_tests.rs
    └── fixtures/
```

### 4.3 核心实现示例

#### 4.3.1 主循环

```rust
use rustyline::error::ReadlineError;
use rustyline::Editor;

pub struct Repl {
    input_handler: InputHandler,
    command_processor: CommandProcessor,
    session_manager: SessionManager,
    output_formatter: OutputFormatter,
}

impl Repl {
    pub async fn run(&mut self) -> Result<()> {
        loop {
            let prompt = self.get_prompt();
            
            match self.input_handler.read_line(&prompt) {
                Ok(line) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    
                    let command = self.command_processor.parse(&line)?;
                    self.execute_command(command).await?;
                }
                Err(ReadlineError::Interrupted) => {
                    println!("^C");
                    continue;
                }
                Err(ReadlineError::Eof) => {
                    println!("\\q");
                    break;
                }
                Err(err) => {
                    eprintln!("Error: {}", err);
                    break;
                }
            }
        }
        
        Ok(())
    }
    
    async fn execute_command(&mut self, command: Command) -> Result<()> {
        match command {
            Command::Query(query) => {
                let result = self.session_manager.execute_query(&query).await?;
                let output = self.output_formatter.format(&result);
                println!("{}", output);
            }
            Command::MetaCommand(meta) => {
                self.execute_meta_command(meta).await?;
            }
        }
        Ok(())
    }
}
```

#### 4.3.2 自动补全

```rust
use rustyline::completion::{Completer, Pair};

pub struct GraphDBCompleter {
    keywords: Vec<String>,
    schema_cache: Arc<RwLock<SchemaCache>>,
}

impl Completer for GraphDBCompleter {
    type Candidate = Pair;

    fn complete(&self, line: &str, pos: usize) -> Result<(usize, Vec<Pair>)> {
        let line_to_cursor = &line[..pos];
        
        // 判断上下文
        if is_keyword_context(line_to_cursor) {
            return self.complete_keyword(line_to_cursor);
        }
        
        if is_identifier_context(line_to_cursor) {
            return self.complete_identifier(line_to_cursor);
        }
        
        Ok((pos, vec![]))
    }
}

impl GraphDBCompleter {
    fn complete_keyword(&self, line: &str) -> Result<(usize, Vec<Pair>)> {
        let last_word = get_last_word(line);
        let completions: Vec<Pair> = self.keywords
            .iter()
            .filter(|k| k.starts_with(&last_word))
            .map(|k| Pair {
                display: k.clone(),
                replacement: k[last_word.len()..].to_string(),
            })
            .collect();
        
        let start = line.len() - last_word.len();
        Ok((start, completions))
    }
    
    fn complete_identifier(&self, line: &str) -> Result<(usize, Vec<Pair>)> {
        let cache = self.schema_cache.read();
        let last_word = get_last_word(line);
        
        let completions: Vec<Pair> = cache
            .get_matching_objects(&last_word)
            .map(|name| Pair {
                display: name.clone(),
                replacement: name[last_word.len()..].to_string(),
            })
            .collect();
        
        let start = line.len() - last_word.len();
        Ok((start, completions))
    }
}
```

#### 4.3.3 输出格式化

```rust
use tabled::{Table, Tabled, settings::Style};

pub struct TableFormatter;

impl OutputFormatter for TableFormatter {
    fn format(&self, result: &QueryResult) -> String {
        if result.rows.is_empty() {
            return "(0 rows)".to_string();
        }
        
        let headers: Vec<String> = result.columns.clone();
        let rows: Vec<Vec<String>> = result.rows
            .iter()
            .map(|row| {
                headers.iter()
                    .map(|col| format_value(row.get(col)))
                    .collect()
            })
            .collect();
        
        let mut table = Table::new(rows);
        table.with(Style::rounded());
        
        format!("{}\n\n({} rows)", table, result.rows.len())
    }
}

fn format_value(value: Option<&Value>) -> String {
    match value {
        Some(Value::Null) => "NULL".to_string(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Int(i)) => i.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        Some(v) => format!("{:?}", v),
        None => "".to_string(),
    }
}
```

### 4.4 错误处理

#### 4.4.1 错误类型

```rust
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("Connection error: {0}")]
    Connection(String),
    
    #[error("Query execution error: {0}")]
    QueryExecution(String),
    
    #[error("Invalid command: {0}")]
    InvalidCommand(String),
    
    #[error("File I/O error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Configuration error: {0}")]
    Config(String),
    
    #[error("Authentication failed: {0}")]
    Authentication(String),
}
```

#### 4.4.2 错误显示

```rust
impl CliError {
    pub fn display(&self) -> String {
        match self {
            CliError::Connection(msg) => {
                format!("ERROR:  Connection failed\nDETAIL:  {}", msg)
            }
            CliError::QueryExecution(msg) => {
                format!("ERROR:  Query execution failed\nDETAIL:  {}", msg)
            }
            CliError::InvalidCommand(msg) => {
                format!("ERROR:  Invalid command\nDETAIL:  {}", msg)
            }
            _ => format!("ERROR:  {}", self),
        }
    }
}
```
