# 多行编辑设计方案

## 1. 概述

### 1.1 目标

为 GraphDB CLI 提供完善的多行编辑支持，包括语句完整性检测、续行提示符、外部编辑器集成和查询缓冲区管理，参考 psql 的编辑体验。

### 1.2 参考实现

- **psql**：`\e` 打开外部编辑器编辑查询缓冲区，`\p` 显示缓冲区内容，`\r` 重置缓冲区，`\w` 写入文件
- **usql**：`\e [-raw|-exec] [FILE] [LINE]` 编辑查询/原始/执行缓冲区，`\p` 显示，`\w` 写入，`\r` 重置
- **rustyline**：支持多行输入（通过 `Validator` trait），内置 Emacs/Vi 编辑模式

## 2. 现状分析

### 2.1 Phase 1 已实现

当前 `main.rs` 中的多行编辑逻辑：

```rust
// 语句完整性检测
if !is_statement_complete(&full_input) {
    // 读取续行
    match input_handler.read_line(&cont_prompt)? {
        Some(next_line) => {
            full_input.push(' ');
            full_input.push_str(&next_line);
        }
        None => break,
    }
}
```

`is_statement_complete()` 函数（`src/input/handler.rs`）：
- 检查分号结尾
- 检查括号匹配（圆括号、花括号、方括号）
- 检查字符串引号匹配
- 元命令（`\` 开头）视为完整

### 2.2 不足之处

| 问题 | 说明 |
|------|------|
| 无外部编辑器支持 | 无法用 vim/vscode 编辑复杂查询 |
| 无查询缓冲区管理 | 无法查看、保存、重置当前缓冲区 |
| 语句检测不完善 | 不支持 `--` 注释跳过分号检测 |
| 无缩进辅助 | 多行输入无自动缩进 |
| 无语法高亮 | 输入时无关键字高亮 |
| 续行仅追加 | 无法编辑已输入的前面的行 |
| 无 `\e` 命令 | psql 用户习惯的编辑器集成缺失 |

## 3. 功能设计

### 3.1 查询缓冲区

#### 3.1.1 缓冲区模型

```rust
pub struct QueryBuffer {
    lines: Vec<String>,
    original_query: Option<String>,
}

impl QueryBuffer {
    pub fn new() -> Self {
        Self {
            lines: Vec::new(),
            original_query: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.lines.iter().all(|l| l.trim().is_empty())
    }

    pub fn content(&self) -> String {
        self.lines.join("\n")
    }

    pub fn add_line(&mut self, line: &str) {
        self.lines.push(line.to_string());
    }

    pub fn reset(&mut self) {
        self.lines.clear();
        self.original_query = None;
    }

    pub fn set_content(&mut self, content: &str) {
        self.lines = content.lines().map(String::from).collect();
    }

    pub fn is_complete(&self) -> bool {
        is_statement_complete(&self.content())
    }
}
```

#### 3.1.2 缓冲区操作命令

| 命令 | 功能 | psql 等价 | 示例 |
|------|------|-----------|------|
| `\e` | 用外部编辑器编辑缓冲区 | `\e` | `\e` |
| `\e <file>` | 编辑指定文件 | `\e file` | `\e query.gql` |
| `\p` | 显示缓冲区内容 | `\p` | `\p` |
| `\r` | 重置（清空）缓冲区 | `\r` | `\r` |
| `\w <file>` | 将缓冲区写入文件 | `\w file` | `\w my_query.gql` |

### 3.2 语句完整性检测增强

#### 3.2.1 当前检测逻辑的问题

1. **注释中的分号**：`-- this; comment` 中的分号不应视为语句结束
2. **多行字符串**：跨行字符串中的分号不应视为结束
3. **GQL 语句无分号**：某些 GQL 语句可能不需要分号（如 `SHOW SPACES`）

#### 3.2.2 增强检测

```rust
pub struct StatementParser {
    in_single_line_comment: bool,
    in_multi_line_comment: bool,
    in_single_quote: bool,
    in_double_quote: bool,
    in_backtick: bool,
    paren_depth: i32,
    brace_depth: i32,
    bracket_depth: i32,
}

impl StatementParser {
    pub fn new() -> Self {
        Self {
            in_single_line_comment: false,
            in_multi_line_comment: false,
            in_single_quote: false,
            in_double_quote: false,
            in_backtick: false,
            paren_depth: 0,
            brace_depth: 0,
            bracket_depth: 0,
        }
    }

    pub fn feed(&mut self, ch: char, prev_ch: Option<char>) {
        // 处理注释
        if !self.in_single_quote && !self.in_double_quote && !self.in_backtick {
            if !self.in_multi_line_comment {
                if ch == '-' && prev_ch == Some('-') {
                    self.in_single_line_comment = true;
                    return;
                }
                if ch == '*' && prev_ch == Some('/') {
                    self.in_multi_line_comment = true;
                    return;
                }
            } else {
                if ch == '/' && prev_ch == Some('*') {
                    self.in_multi_line_comment = false;
                    return;
                }
            }
        }

        if self.in_single_line_comment {
            if ch == '\n' {
                self.in_single_line_comment = false;
            }
            return;
        }

        if self.in_multi_line_comment {
            return;
        }

        // 处理引号
        match ch {
            '\'' if !self.in_double_quote && !self.in_backtick => {
                self.in_single_quote = !self.in_single_quote;
            }
            '"' if !self.in_single_quote && !self.in_backtick => {
                self.in_double_quote = !self.in_double_quote;
            }
            '`' if !self.in_single_quote && !self.in_double_quote => {
                self.in_backtick = !self.in_backtick;
            }
            '(' if !self.in_any_string() => self.paren_depth += 1,
            ')' if !self.in_any_string() => self.paren_depth -= 1,
            '{' if !self.in_any_string() => self.brace_depth += 1,
            '}' if !self.in_any_string() => self.brace_depth -= 1,
            '[' if !self.in_any_string() => self.bracket_depth += 1,
            ']' if !self.in_any_string() => self.bracket_depth -= 1,
            _ => {}
        }
    }

    pub fn is_balanced(&self) -> bool {
        !self.in_single_quote
            && !self.in_double_quote
            && !self.in_backtick
            && self.paren_depth <= 0
            && self.brace_depth <= 0
            && self.bracket_depth <= 0
    }

    fn in_any_string(&self) -> bool {
        self.in_single_quote || self.in_double_quote || self.in_backtick
    }
}
```

#### 3.2.3 特殊语句处理

某些 GQL 语句不需要分号结尾即可执行：

```rust
const AUTO_COMPLETE_STATEMENTS: &[&str] = &[
    "SHOW SPACES",
    "SHOW TAGS",
    "SHOW EDGES",
    "SHOW INDEXES",
    "SHOW USERS",
    "SHOW FUNCTIONS",
];
```

对于这些语句，即使没有分号也视为完整语句（仅在交互模式下）。

### 3.3 外部编辑器集成

#### 3.3.1 编辑器选择

```rust
pub fn get_editor_command() -> String {
    // 优先级：
    // 1. \set EDITOR 命令设置的变量
    // 2. EDITOR 环境变量
    // 3. VISUAL 环境变量
    // 4. 配置文件中的 editor.command
    // 5. 平台默认值

    std::env::var("EDITOR")
        .or_else(|_| std::env::var("VISUAL"))
        .unwrap_or_else(|_| {
            if cfg!(target_os = "windows") {
                "notepad".to_string()
            } else {
                "vi".to_string()
            }
        })
}
```

#### 3.3.2 编辑流程

```
1. 将当前查询缓冲区写入临时文件
2. 启动外部编辑器打开该文件
3. 等待编辑器退出
4. 读取文件内容，更新查询缓冲区
5. 如果内容以分号结尾，自动执行
6. 否则回到多行输入模式
```

```rust
pub fn edit_in_external_editor(buffer: &mut QueryBuffer) -> Result<bool> {
    let editor = get_editor_command();
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join("graphdb_query.gql");

    // 写入临时文件
    std::fs::write(&temp_file, buffer.content())?;

    // 启动编辑器
    let status = std::process::Command::new(&editor)
        .arg(&temp_file)
        .status()
        .map_err(|e| CliError::Other(format!("Failed to launch editor '{}': {}", editor, e)))?;

    if !status.success() {
        return Ok(false);
    }

    // 读取编辑后的内容
    let content = std::fs::read_to_string(&temp_file)?;
    buffer.set_content(&content);

    // 清理临时文件
    let _ = std::fs::remove_file(&temp_file);

    Ok(true)
}
```

#### 3.3.3 编辑器命令行参数

支持指定行号打开编辑器：

```
\e +10         # 在第 10 行打开编辑器
\e query.gql   # 编辑指定文件
\e +5 query.gql # 在第 5 行打开指定文件
```

```rust
fn parse_editor_args(arg: &str) -> (Option<String>, Option<usize>) {
    let parts: Vec<&str> = arg.split_whitespace().collect();
    let mut file = None;
    let mut line = None;

    for part in parts {
        if let Some(l) = part.strip_prefix('+') {
            line = l.parse().ok();
        } else {
            file = Some(part.to_string());
        }
    }

    (file, line)
}
```

### 3.4 语法高亮

#### 3.4.1 基于 rustyline Highlighter

```rust
impl Highlighter for GraphDBCompleter {
    fn highlight<'l>(&self, line: &'l str, pos: usize) -> Cow<'l, str> {
        // 简单的关键字高亮
        let mut result = String::new();
        let mut in_string = false;
        let mut word_start = 0;

        for (i, ch) in line.char_indices() {
            if ch == '\'' || ch == '"' {
                in_string = !in_string;
            }

            if !in_string && (ch.is_whitespace() || is_separator(ch)) {
                if word_start < i {
                    let word = &line[word_start..i];
                    if is_gql_keyword(word) {
                        result.push_str(&word.to_uppercase().blue().to_string());
                    } else {
                        result.push_str(word);
                    }
                }
                result.push(ch);
                word_start = i + ch.len_utf8();
            }
        }

        Cow::Owned(result)
    }
}
```

**高亮规则**：

| 类型 | 颜色 | 示例 |
|------|------|------|
| GQL 关键字 | 蓝色 | `MATCH`, `RETURN`, `WHERE` |
| 字符串 | 绿色 | `'Alice'`, `"hello"` |
| 数字 | 黄色 | `42`, `3.14` |
| 注释 | 暗灰色 | `-- comment` |
| 元命令 | 青色 | `\show_spaces` |
| 变量引用 | 品红 | `:limit` |

#### 3.4.2 可选开关

```toml
[editor]
syntax_highlight = true    # 是否启用语法高亮
```

通过 `\set SYNTAX_HL on|off` 动态切换。

### 3.5 续行提示符增强

#### 3.5.1 上下文感知提示符

```rust
pub fn continuation_prompt(&self, buffer: &QueryBuffer) -> String {
    let content = buffer.content();
    let last_line = content.lines().last().unwrap_or("");

    // 根据未闭合的括号类型显示不同提示
    if content.chars().filter(|&c| c == '(').count()
        > content.chars().filter(|&c| c == ')').count()
    {
        return "graphdb(> ".to_string();  // 未闭合圆括号
    }

    if content.chars().filter(|&c| c == '{').count()
        > content.chars().filter(|&c| c == '}').count()
    {
        return "graphdb{> ".to_string();  // 未闭合花括号
    }

    // 默认续行提示符
    "graphdb-> ".to_string()
}
```

#### 3.5.2 提示符类型

| 提示符 | 含义 | 示例 |
|--------|------|------|
| `graphdb(user:space)=# ` | 主提示符，等待新命令 | |
| `graphdb(user:space)-> ` | 续行提示符，语句未结束 | |
| `graphdb(user:space)(> ` | 圆括号内续行 | `MATCH (p:person` |
| `graphdb(user:space){> ` | 花括号内续行 | `SET p = {name: "Alice"` |

## 4. 模块结构

### 4.1 文件组织

```
src/input/
├── mod.rs              # 模块导出
├── handler.rs          # 输入处理（已有，需修改）
├── history.rs          # 历史管理（Phase 2 新增）
├── buffer.rs           # 查询缓冲区（新增）
└── statement.rs        # 语句解析器（新增，从 handler.rs 拆出）
```

### 4.2 核心接口变更

```rust
// buffer.rs
pub struct QueryBuffer {
    lines: Vec<String>,
    original_query: Option<String>,
}

// statement.rs
pub struct StatementParser { ... }
pub fn is_statement_complete(input: &str) -> bool { ... }
pub fn detect_statement_type(input: &str) -> StatementType { ... }
```

### 4.3 MetaCommand 扩展

```rust
pub enum MetaCommand {
    // ... 已有变体 ...

    // 新增：查询缓冲区操作
    Edit { file: Option<String>, line: Option<usize> },
    PrintBuffer,
    ResetBuffer,
    WriteBuffer { file: String },
}
```

## 5. 实现步骤

### Step 1: 拆分语句解析器（1 天）

- 将 `is_statement_complete()` 从 `handler.rs` 拆到 `statement.rs`
- 实现 `StatementParser` 增强版
- 支持注释跳过、多行字符串

### Step 2: 实现查询缓冲区（1 天）

- 新增 `buffer.rs`，实现 `QueryBuffer`
- 修改 REPL 主循环使用 `QueryBuffer` 代替 `String`
- 支持缓冲区查看、重置、写入

### Step 3: 实现外部编辑器集成（2 天）

- 实现 `edit_in_external_editor()`
- 新增 `\e`、`\p`、`\r`、`\w` 元命令
- 处理临时文件和编辑器退出状态

### Step 4: 实现语法高亮（2 天）

- 实现 `Highlighter` trait
- 定义高亮规则（关键字、字符串、数字、注释）
- 支持动态开关

### Step 5: 增强续行提示符（1 天）

- 根据括号类型显示不同提示符
- 集成到 REPL 主循环

### Step 6: 测试（1 天）

- 语句完整性检测测试（含注释、多行字符串）
- 外部编辑器集成测试
- 缓冲区操作测试

## 6. 测试用例

### 6.1 语句完整性检测

| 输入 | 是否完整 | 说明 |
|------|----------|------|
| `MATCH (p:person) RETURN p;` | ✅ | 标准完整语句 |
| `MATCH (p:person)` | ❌ | 缺少分号 |
| `-- comment;` | ❌ | 分号在注释中 |
| `MATCH (p:person) -- ;\nRETURN p;` | ✅ | 注释后继续 |
| `INSERT VERTEX person(name) VALUES "p1":("Alice;Bob");` | ✅ | 分号在字符串中 |
| `\show_spaces` | ✅ | 元命令 |

### 6.2 外部编辑器

| 命令 | 行为 |
|------|------|
| `\e` | 打开编辑器编辑当前缓冲区 |
| `\e +10` | 在第 10 行打开编辑器 |
| `\e query.gql` | 编辑指定文件 |
| `\p` | 显示当前缓冲区内容 |
| `\r` | 清空当前缓冲区 |
| `\w out.gql` | 将缓冲区写入文件 |

### 6.3 续行提示符

| 输入 | 提示符 |
|------|--------|
| `MATCH (p:person)` | `graphdb(> ` |
| `SET p = {name: "Alice"` | `graphdb{> ` |
| `MATCH (p:person)` + Enter | `graphdb-> ` |
