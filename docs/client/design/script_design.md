# 脚本执行设计方案

## 1. 概述

### 1.1 目标

为 GraphDB CLI 提供完善的脚本执行功能，支持从文件读取并批量执行 GQL 语句，包括事务控制、错误处理、条件执行、输出控制和脚本嵌套，参考 psql 的脚本执行机制。

### 1.2 参考实现

- **psql**：`psql -f script.sql` 或 `\i script.sql` 执行脚本，支持 `\i` 嵌套、`ON_ERROR_STOP` 控制、`\q` 退出脚本、`-1`/`--single-transaction` 单事务模式
- **usql**：`\i [-raw|-exec] FILE` 执行脚本，`-raw` 模式直接发送原始内容，`-exec` 模式逐条执行
- **MySQL CLI**：`source file.sql` 或 `mysql < file.sql`，`--force` 忽略错误继续执行

## 2. 现状分析

### 2.1 Phase 1 已实现

当前脚本执行功能（`src/command/executor.rs`）：

```rust
async fn execute_script(&mut self, path: &str, session_mgr: &mut SessionManager) -> Result<()> {
    let content = fs::read_to_string(path)
        .map_err(|_| CliError::ScriptNotFound(path.to_string()))?;

    let commands = self.parse_script(&content);

    for cmd_str in commands {
        let cmd_str = cmd_str.trim();
        if cmd_str.is_empty() || cmd_str.starts_with("--") || cmd_str.starts_with("//") {
            continue;
        }

        let command = crate::command::parser::parse_command(cmd_str);
        match self.execute(command, session_mgr).await {
            Ok(should_continue) => {
                if !should_continue { break; }
            }
            Err(e) => {
                self.write_output(&self.formatter.format_error(&e.to_string()))?;
            }
        }
    }

    Ok(())
}
```

命令行入口（`src/main.rs`）：

```rust
if let Some(ref file) = cli.file {
    let command = Command::MetaCommand(MetaCommand::ExecuteScript { path: file.clone() });
    // ...
}
```

### 2.2 不足之处

| 问题 | 说明 |
|------|------|
| 脚本解析粗糙 | 仅按分号分割，不支持多行语句 |
| 无错误控制 | 出错后继续执行，无 `ON_ERROR_STOP` |
| 无事务支持 | 不支持单事务模式执行脚本 |
| 无脚本嵌套 | `\i` 在脚本中不支持递归调用 |
| 无条件执行 | 不支持 `\if`/`\elif`/`\else`/`\endif` |
| 无输出控制 | 不支持 `\o` 重定向脚本输出 |
| 无行号追踪 | 报错时不显示脚本中的行号 |
| 无 `-1` 单事务模式 | 命令行不支持单事务执行 |
| 无原始模式 | 不支持将整个脚本作为单条语句发送 |
| 无脚本参数 | 不支持向脚本传递参数 |
| 注释处理不完善 | 仅跳过 `--` 和 `//` 开头的行 |

## 3. 功能设计

### 3.1 脚本解析器

#### 3.1.1 解析策略

脚本解析需要正确处理多行语句、注释和字符串中的分号：

```rust
pub struct ScriptParser {
    statement_parser: StatementParser,
}

impl ScriptParser {
    pub fn parse(&self, content: &str) -> Vec<ParsedStatement> {
        let mut statements = Vec::new();
        let mut current = String::new();
        let mut line_number = 1;
        let mut start_line = 1;
        let mut parser = StatementParser::new();

        for line in content.lines() {
            let trimmed = line.trim();

            // 空行
            if trimmed.is_empty() {
                if !current.is_empty() {
                    current.push('\n');
                }
                line_number += 1;
                continue;
            }

            // 单行注释
            if trimmed.starts_with("--") || trimmed.starts_with("//") {
                line_number += 1;
                continue;
            }

            // 元命令（独立行，以 \ 开头）
            if trimmed.starts_with('\\') {
                // 先保存当前语句
                if !current.trim().is_empty() {
                    statements.push(ParsedStatement {
                        content: current.trim().to_string(),
                        start_line,
                        end_line: line_number - 1,
                        kind: StatementKind::Query,
                    });
                    current.clear();
                }

                statements.push(ParsedStatement {
                    content: trimmed.to_string(),
                    start_line: line_number,
                    end_line: line_number,
                    kind: StatementKind::MetaCommand,
                });

                line_number += 1;
                start_line = line_number;
                continue;
            }

            // 累积语句行
            if current.is_empty() {
                start_line = line_number;
            } else {
                current.push('\n');
            }
            current.push_str(line);

            // 检查语句是否完整
            for ch in line.chars() {
                parser.feed(ch, None);
            }

            if parser.is_balanced() && line.trim().ends_with(';') {
                statements.push(ParsedStatement {
                    content: current.trim().to_string(),
                    start_line,
                    end_line: line_number,
                    kind: StatementKind::Query,
                });
                current.clear();
                parser = StatementParser::new();
                start_line = line_number + 1;
            }

            line_number += 1;
        }

        // 处理最后一条未以分号结尾的语句
        if !current.trim().is_empty() {
            statements.push(ParsedStatement {
                content: current.trim().to_string(),
                start_line,
                end_line: line_number - 1,
                kind: StatementKind::Query,
            });
        }

        statements
    }
}

pub struct ParsedStatement {
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    pub kind: StatementKind,
}

pub enum StatementKind {
    Query,
    MetaCommand,
}
```

#### 3.1.2 解析示例

输入脚本：

```gql
-- Create space
CREATE SPACE mygraph (vid_type=FIXED_STRING(32));

USE mygraph;

-- Create tag
CREATE TAG person (
    name STRING DEFAULT "",
    age INT,
    -- email is optional
    email STRING NULL
);

INSERT VERTEX person(name, age)
VALUES "p1":("Alice", 30);
```

解析结果：

| # | 类型 | 行号 | 内容 |
|---|------|------|------|
| 1 | Query | 2 | `CREATE SPACE mygraph (vid_type=FIXED_STRING(32));` |
| 2 | Query | 4 | `USE mygraph;` |
| 3 | Query | 7-11 | `CREATE TAG person (...)` |
| 4 | Query | 13-14 | `INSERT VERTEX person(name, age) VALUES "p1":("Alice", 30);` |

### 3.2 错误处理

#### 3.2.1 ON_ERROR_STOP 模式

```rust
pub enum ErrorMode {
    Continue,   // 出错继续执行（默认）
    Stop,       // 出错立即停止
}
```

**行为差异**：

| 模式 | 出错时行为 | 事务影响 |
|------|-----------|----------|
| `Continue` | 打印错误，继续下一条 | 无影响 |
| `Stop` | 打印错误，停止执行 | 回滚当前事务（如有） |

#### 3.2.2 错误信息增强

脚本模式下错误信息包含文件名和行号：

```
ERROR: Syntax error near "INSERTT" at script.gql:13
LINE: INSERTT VERTEX person(name, age) VALUES "p1":("Alice", 30);
```

```rust
struct ScriptError {
    message: String,
    file: String,
    line: usize,
    statement: String,
}

impl std::fmt::Display for ScriptError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "ERROR: {} at {}:{}\nLINE: {}",
            self.message, self.file, self.line, self.statement
        )
    }
}
```

### 3.3 事务控制

#### 3.3.1 单事务模式

```bash
graphdb-cli -1 -f init_script.gql
graphdb-cli --single-transaction -f init_script.gql
```

**行为**：
1. 执行前发送 `BEGIN`
2. 执行脚本中的所有语句
3. 全部成功则 `COMMIT`
4. 任一失败则 `ROLLBACK`（仅在 `ON_ERROR_STOP` 模式下）

```rust
async fn execute_script_in_transaction(
    &mut self,
    path: &str,
    session_mgr: &mut SessionManager,
) -> Result<()> {
    session_mgr.execute_query("BEGIN").await?;

    match self.execute_script(path, session_mgr).await {
        Ok(()) => {
            session_mgr.execute_query("COMMIT").await?;
            Ok(())
        }
        Err(e) => {
            let _ = session_mgr.execute_query("ROLLBACK").await;
            Err(e)
        }
    }
}
```

#### 3.3.2 脚本内事务

脚本中可以显式使用事务语句：

```gql
BEGIN;
INSERT VERTEX person(name) VALUES "p1":("Alice");
INSERT VERTEX person(name) VALUES "p2":("Bob");
COMMIT;
```

### 3.4 脚本嵌套

#### 3.4.1 嵌套执行

脚本中可以使用 `\i` 命令嵌套执行另一个脚本：

```gql
-- main.gql
CREATE SPACE mygraph;
USE mygraph;
\i tags.gql
\i edges.gql
\i data.gql
```

#### 3.4.2 嵌套深度限制

```rust
const MAX_SCRIPT_DEPTH: usize = 16;

struct ScriptExecutionContext {
    depth: usize,
    call_stack: Vec<String>,
}

impl ScriptExecutionContext {
    fn enter_script(&mut self, path: &str) -> Result<()> {
        if self.depth >= MAX_SCRIPT_DEPTH {
            return Err(CliError::Other(format!(
                "Script nesting too deep (max {}): {}",
                MAX_SCRIPT_DEPTH, path
            )));
        }

        if self.call_stack.contains(&path.to_string()) {
            return Err(CliError::Other(format!(
                "Circular script reference detected: {}",
                path
            )));
        }

        self.depth += 1;
        self.call_stack.push(path.to_string());
        Ok(())
    }

    fn exit_script(&mut self) {
        self.depth -= 1;
        self.call_stack.pop();
    }
}
```

#### 3.4.3 脚本搜索路径

```rust
fn resolve_script_path(path: &str, search_paths: &[PathBuf]) -> Result<PathBuf> {
    // 1. 绝对路径
    if Path::new(path).is_absolute() {
        if Path::new(path).exists() {
            return Ok(PathBuf::from(path));
        }
        return Err(CliError::ScriptNotFound(path.to_string()));
    }

    // 2. 相对于当前脚本目录
    if let Some(current_script) = search_paths.last() {
        let resolved = current_script.join(path);
        if resolved.exists() {
            return Ok(resolved);
        }
    }

    // 3. 相对于工作目录
    if Path::new(path).exists() {
        return Ok(PathBuf::from(path));
    }

    // 4. 搜索路径
    for dir in search_paths {
        let resolved = dir.join(path);
        if resolved.exists() {
            return Ok(resolved);
        }
    }

    Err(CliError::ScriptNotFound(path.to_string()))
}
```

### 3.5 条件执行

#### 3.5.1 `\if`/`\elif`/`\else`/`\endif`

参考 psql 的条件执行语法：

```gql
\set mode production

\if :mode == production
    CREATE SPACE prod_graph (vid_type=FIXED_STRING(64));
\elif :mode == test
    CREATE SPACE test_graph (vid_type=FIXED_STRING(32));
\else
    CREATE SPACE dev_graph (vid_type=FIXED_STRING(16));
\endif
```

#### 3.5.2 条件求值

```rust
enum ConditionExpr {
    Equals { var: String, value: String },
    NotEquals { var: String, value: String },
    IsSet { var: String },
    IsNotSet { var: String },
}

impl ConditionExpr {
    fn evaluate(&self, variables: &HashMap<String, String>) -> bool {
        match self {
            ConditionExpr::Equals { var, value } => {
                variables.get(var).map(|v| v == value).unwrap_or(false)
            }
            ConditionExpr::NotEquals { var, value } => {
                variables.get(var).map(|v| v != value).unwrap_or(true)
            }
            ConditionExpr::IsSet { var } => variables.contains_key(var),
            ConditionExpr::IsNotSet { var } => !variables.contains_key(var),
        }
    }
}
```

#### 3.5.3 条件栈

```rust
struct ConditionalStack {
    stack: Vec<ConditionalState>,
}

struct ConditionalState {
    condition_met: bool,     // 当前分支条件是否满足
    any_branch_taken: bool,  // 是否已有分支被执行
    in_active_branch: bool,  // 当前是否在活跃分支中
}

impl ConditionalStack {
    fn is_active(&self) -> bool {
        self.stack.iter().all(|s| s.in_active_branch)
    }
}
```

### 3.6 输出重定向

#### 3.6.1 `\o` 命令

```
\o                    # 重置输出到 stdout
\o results.txt        # 输出重定向到文件
\o >> results.txt     # 追加模式重定向
\o |less              # 通过管道输出到外部命令
```

#### 3.6.2 实现

```rust
pub enum OutputTarget {
    Stdout,
    File { path: PathBuf, append: bool },
    Pipe { command: String },
}

pub struct OutputRedirector {
    target: OutputTarget,
    file: Option<std::fs::File>,
    child: Option<std::process::Child>,
}

impl OutputRedirector {
    pub fn redirect_to_file(&mut self, path: &str, append: bool) -> Result<()> {
        let path = PathBuf::from(path);
        let file = OpenOptions::new()
            .create(true)
            .append(append)
            .write(true)
            .open(&path)?;

        self.target = OutputTarget::File { path, append };
        self.file = Some(file);
        self.child = None;
        Ok(())
    }

    pub fn reset(&mut self) {
        self.target = OutputTarget::Stdout;
        self.file = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
    }

    pub fn write(&mut self, content: &str) -> std::io::Result<()> {
        match &mut self.target {
            OutputTarget::Stdout => {
                print!("{}", content);
                std::io::stdout().flush()
            }
            OutputTarget::File { .. } => {
                if let Some(ref mut file) = self.file {
                    file.write_all(content.as_bytes())?;
                    file.flush()
                } else {
                    Ok(())
                }
            }
            OutputTarget::Pipe { .. } => {
                if let Some(ref mut child) = self.child {
                    if let Some(ref mut stdin) = child.stdin {
                        stdin.write_all(content.as_bytes())?;
                        stdin.flush()
                    } else {
                        Ok(())
                    }
                } else {
                    Ok(())
                }
            }
        }
    }
}
```

### 3.7 脚本参数

#### 3.7.1 传递参数

```bash
graphdb-cli -f init.gql -v space=mygraph -v vid_type=FIXED_STRING\(32\)
```

脚本中使用变量引用：

```gql
CREATE SPACE :space (vid_type=:vid_type);
USE :space;
```

#### 3.7.2 位置参数

```bash
graphdb-cli -f init.gql -v 1=mygraph -v 2=FIXED_STRING\(32\)
```

脚本中通过 `:1`、`:2` 引用位置参数。

### 3.8 原始模式

#### 3.8.1 设计

某些场景下需要将整个脚本内容作为一条语句发送（如服务端支持批量执行）：

```
\ir script.gql     # 原始模式执行，整个文件作为一条语句
```

```rust
async fn execute_script_raw(&mut self, path: &str, session: &mut SessionManager) -> Result<()> {
    let content = fs::read_to_string(path)
        .map_err(|_| CliError::ScriptNotFound(path.to_string()))?;

    let substituted = session.substitute_variables(&content);
    let result = session.execute_query(&substituted).await?;

    self.write_output(&self.formatter.format_result(&result))?;
    Ok(())
}
```

## 4. 命令行接口

### 4.1 新增参数

```rust
#[derive(Parser)]
pub struct Cli {
    // ... 已有参数 ...

    /// Execute SQL commands from a file, then exit
    #[arg(short = 'f', long = "file", value_name = "FILE")]
    pub file: Option<String>,

    /// Execute all commands in a single transaction
    #[arg(short = '1', long = "single-transaction")]
    pub single_transaction: bool,

    /// Continue processing after an error (overrides ON_ERROR_STOP)
    #[arg(long = "force")]
    pub force: bool,

    /// Set variable before execution (-v NAME=VALUE)
    #[arg(short = 'v', long = "variable", value_name = "NAME=VALUE")]
    pub variables: Vec<String>,

    /// Script search path
    #[arg(long = "search-path", value_name = "DIR")]
    pub search_path: Vec<String>,
}
```

### 4.2 元命令扩展

```rust
pub enum MetaCommand {
    // ... 已有变体 ...

    // 脚本执行增强
    ExecuteScript { path: String },         // \i - 逐条执行
    ExecuteScriptRaw { path: String },      // \ir - 原始模式执行

    // 条件执行
    If { condition: String },
    Elif { condition: String },
    Else,
    EndIf,

    // 输出重定向
    OutputRedirect { path: Option<String> },
}
```

## 5. 模块结构

### 5.1 文件组织

```
src/command/
├── mod.rs              # 模块导出
├── parser.rs           # 命令解析（已有，需扩展）
├── executor.rs         # 命令执行（已有，需重构）
├── script.rs           # 脚本解析与执行（新增）
└── condition.rs        # 条件执行（新增）
```

### 5.2 核心类型

```rust
// script.rs
pub struct ScriptParser { ... }
pub struct ParsedStatement { ... }
pub struct ScriptExecutionContext {
    depth: usize,
    call_stack: Vec<String>,
    conditional_stack: ConditionalStack,
    output_redirector: OutputRedirector,
}

// condition.rs
pub struct ConditionalStack { ... }
pub enum ConditionExpr { ... }
```

## 6. 实现步骤

### Step 1: 实现脚本解析器（2 天）

- 新增 `script.rs`，实现 `ScriptParser`
- 支持多行语句、注释跳过、行号追踪
- 替换现有的简单分号分割逻辑

### Step 2: 增强错误处理（1 天）

- 实现 `ON_ERROR_STOP` 支持
- 脚本错误信息包含文件名和行号
- 实现 `--force` 命令行参数

### Step 3: 实现事务控制（1 天）

- 实现 `-1`/`--single-transaction` 模式
- 脚本内事务语句处理

### Step 4: 实现脚本嵌套（1 天）

- 实现 `\i` 在脚本中的嵌套调用
- 深度限制和循环检测
- 脚本搜索路径

### Step 5: 实现条件执行（2 天）

- 新增 `condition.rs`
- 实现 `\if`/`\elif`/`\else`/`\endif`
- 条件求值和条件栈

### Step 6: 实现输出重定向（1 天）

- 实现 `OutputRedirector`
- 支持 `\o` 重定向到文件和管道

### Step 7: 原始模式和脚本参数（1 天）

- 实现 `\ir` 原始模式
- 实现 `-v` 命令行变量预设

### Step 8: 测试（1 天）

- 脚本解析测试
- 嵌套脚本测试
- 条件执行测试
- 事务控制测试
- 错误处理测试

## 7. 测试用例

### 7.1 脚本解析

| 脚本内容 | 解析结果 |
|----------|----------|
| 单行语句 + 分号 | 1 条 Query |
| 多行语句（跨行） | 1 条 Query |
| `-- comment` | 跳过 |
| `\show_spaces` | 1 条 MetaCommand |
| 混合查询和元命令 | 多条混合 |

### 7.2 错误处理

| 场景 | `ON_ERROR_STOP=off` | `ON_ERROR_STOP=on` |
|------|---------------------|---------------------|
| 第 2 条语句出错 | 打印错误，继续执行 | 打印错误，停止执行 |
| 脚本不存在 | 报错退出 | 报错退出 |

### 7.3 事务控制

| 场景 | 预期 |
|------|------|
| `-1 -f script.gql`，全部成功 | BEGIN → 执行 → COMMIT |
| `-1 -f script.gql`，第 3 条失败 | BEGIN → 执行 → ROLLBACK |
| 脚本内显式 `BEGIN/COMMIT` | 按脚本逻辑执行 |

### 7.4 嵌套脚本

| 场景 | 预期 |
|------|------|
| `main.gql` 中 `\i sub.gql` | 先执行 sub.gql，再继续 main.gql |
| 嵌套深度超过 16 | 报错：Script nesting too deep |
| 循环引用 A → B → A | 报错：Circular script reference |

### 7.5 条件执行

```gql
\set env test
\if :env == production
    CREATE SPACE prod;
\elif :env == test
    CREATE SPACE test;
\else
    CREATE SPACE dev;
\endif
```

| `env` 值 | 执行结果 |
|-----------|----------|
| `production` | `CREATE SPACE prod;` |
| `test` | `CREATE SPACE test;` |
| `dev` | `CREATE SPACE dev;` |

### 7.6 输出重定向

| 命令 | 行为 |
|------|------|
| `\o out.txt` | 后续输出写入 out.txt |
| `\o >> out.txt` | 后续输出追加到 out.txt |
| `\o` | 恢复输出到 stdout |
