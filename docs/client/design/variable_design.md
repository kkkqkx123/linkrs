# 变量管理设计方案

## 1. 概述

### 1.1 目标

为 GraphDB CLI 提供完整的变量管理系统，支持用户自定义变量、变量替换、特殊内置变量、变量持久化和环境变量集成，参考 psql 的变量机制。

### 1.2 参考实现

- **psql**：`\set [name [value]]` 设置变量，`\unset name` 删除变量，`:name` 引用变量，`:'name'` 带引号引用，`:"name"` 双引号引用
- **psql 特殊变量**：`ON_ERROR_STOP`、`ECHO`、`ECHO_HIDDEN`、`QUIET`、`SINGLELINE`、`PROMPT1/2/3`、`COMP_KEYWORD_CASE` 等
- **usql**：`\set name value`、`\unset name`、`:name`、`:variable` 替换，`-v name=value` 命令行设置

## 2. 现状分析

### 2.1 Phase 1 已实现

当前 `Session`（`src/session/manager.rs`）中的变量功能：

```rust
pub struct Session {
    pub variables: HashMap<String, String>,
    // ...
}

impl Session {
    pub fn set_variable(&mut self, name: String, value: String) { ... }
    pub fn get_variable(&self, name: &str) -> Option<&String> { ... }
    pub fn remove_variable(&mut self, name: &str) { ... }
    pub fn substitute_variables(&self, input: &str) -> String { ... }
}
```

`MetaCommand` 中的变量命令：

```rust
Set { name: String, value: Option<String> },
Unset { name: String },
ShowVariables,
```

### 2.2 不足之处

| 问题 | 说明 |
|------|------|
| 无特殊内置变量 | 缺少 `ON_ERROR_STOP`、`ECHO` 等控制变量 |
| 替换语法单一 | 仅支持 `:name`，不支持 `:'name'` 和 `:"name"` |
| 无类型检查 | 所有变量都是字符串，无法验证值的有效性 |
| 无持久化 | 变量仅在会话内有效，退出后丢失 |
| 无命令行预设 | 不支持 `-v name=value` |
| 无环境变量集成 | 不支持从环境变量读取 |
| 无变量作用域 | 所有变量全局共享 |
| `\set` 无参数时行为不明确 | psql 中 `\set` 显示所有变量 |
| 替换时机不正确 | 应在查询执行前替换，而非在所有输入上替换 |

## 3. 功能设计

### 3.1 变量类型

#### 3.1.1 用户变量

用户通过 `\set` 命令自定义的变量，可在查询中通过 `:name` 引用。

```sql
\set limit 10
\set name 'Alice'
MATCH (p:person) WHERE p.name == :name RETURN p LIMIT :limit;
-- 替换后: MATCH (p:person) WHERE p.name == 'Alice' RETURN p LIMIT 10;
```

#### 3.1.2 特殊内置变量

控制 CLI 行为的特殊变量，具有预定义的语义：

| 变量名 | 类型 | 默认值 | 说明 |
|--------|------|--------|------|
| `ON_ERROR_STOP` | bool | `off` | 遇到错误时停止执行（脚本模式） |
| `ECHO` | enum | `none` | 回显模式：`none`/`queries`/`all` |
| `ECHO_HIDDEN` | bool | `off` | 回显内部查询（元命令生成的查询） |
| `QUIET` | bool | `off` | 静默模式，不显示欢迎信息和行数统计 |
| `TIMING` | bool | `off` | 显示查询执行时间 |
| `SINGLELINE` | bool | `off` | 单行模式，回车即执行 |
| `SYNTAX_HL` | bool | `on` | 是否启用语法高亮 |
| `PROMPT1` | string | 见提示符设计 | 主提示符模板 |
| `PROMPT2` | string | 见提示符设计 | 续行提示符模板 |
| `HISTSIZE` | number | `5000` | 历史记录最大条数 |
| `EDITOR` | string | 环境变量 | 外部编辑器命令 |
| `FORMAT` | enum | `table` | 默认输出格式 |
| `COMP_KEYWORD_CASE` | enum | `upper` | 补全关键字大小写：`upper`/`lower`/`preserve` |
| `AUTOCOMMIT` | bool | `on` | 自动提交模式 |
| `VERBOSITY` | enum | `default` | 错误信息详细度：`default`/`terse`/`verbose` |

#### 3.1.3 环境变量

从系统环境变量读取，以 `ENV_` 前缀访问：

```sql
-- 环境变量 HOME 可通过 :ENV_HOME 引用
\set output_dir :ENV_HOME/graphdb_output
```

### 3.2 变量替换语法

#### 3.2.1 替换模式

| 语法 | 行为 | 示例 |
|------|------|------|
| `:name` | 直接替换为变量值 | `:limit` → `10` |
| `:'name'` | 替换为单引号包围的值 | `:'name'` → `'Alice'` |
| `:"name"` | 替换为双引号包围的值 | `:"name"` → `"Alice"` |
| `::name` | 不替换，输出字面 `:name` | `::limit` → `:limit` |

#### 3.2.2 替换规则

```rust
pub struct VariableSubstitutor {
    variables: HashMap<String, String>,
}

impl VariableSubstitutor {
    pub fn substitute(&self, input: &str) -> Result<String> {
        let mut result = String::with_capacity(input.len());
        let chars: Vec<char> = input.chars().collect();
        let mut i = 0;

        while i < chars.len() {
            if chars[i] == ':' && i + 1 < chars.len() {
                // :: → 字面冒号，不替换
                if chars[i + 1] == ':' {
                    result.push(':');
                    i += 2;
                    continue;
                }

                // :'name' → 单引号包围
                if chars[i + 1] == '\'' {
                    let (replaced, new_i) = self.substitute_quoted(&chars, i + 2, '\'')?;
                    result.push_str(&replaced);
                    i = new_i;
                    continue;
                }

                // :"name" → 双引号包围
                if chars[i + 1] == '"' {
                    let (replaced, new_i) = self.substitute_quoted(&chars, i + 2, '"')?;
                    result.push_str(&replaced);
                    i = new_i;
                    continue;
                }

                // :name → 直接替换
                if is_var_start_char(chars[i + 1]) {
                    let (replaced, new_i) = self.substitute_plain(&chars, i + 1)?;
                    result.push_str(&replaced);
                    i = new_i;
                    continue;
                }
            }

            result.push(chars[i]);
            i += 1;
        }

        Ok(result)
    }

    fn substitute_plain(&self, chars: &[char], start: usize) -> Result<(String, usize)> {
        let mut end = start;
        while end < chars.len() && is_var_char(chars[end]) {
            end += 1;
        }

        let var_name: String = chars[start..end].iter().collect();
        let value = self.variables.get(&var_name).ok_or_else(|| {
            CliError::Other(format!("Undefined variable: :{}", var_name))
        })?;

        Ok((value.clone(), end))
    }

    fn substitute_quoted(
        &self,
        chars: &[char],
        start: usize,
        quote: char,
    ) -> Result<(String, usize)> {
        let mut end = start;
        while end < chars.len() && chars[end] != quote {
            end += 1;
        }

        if end >= chars.len() {
            return Err(CliError::Other("Unterminated quoted variable reference".to_string()));
        }

        let var_name: String = chars[start..end].iter().collect();
        let value = self.variables.get(&var_name).ok_or_else(|| {
            CliError::Other(format!("Undefined variable: :{}", var_name))
        })?;

        Ok((format!("{}{}{}", quote, value, quote), end + 1))
    }
}

fn is_var_start_char(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_var_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}
```

#### 3.2.3 替换时机

变量替换仅在以下时机执行：

1. **查询执行前**：GQL 查询发送到服务器前替换
2. **脚本执行前**：脚本中每条语句执行前替换
3. **元命令参数**：部分元命令参数替换（如 `\connect :space_name`）

**不替换的场景**：
- 元命令名本身（`\set` 中的变量名不替换）
- 字符串字面量内部（可选，通过配置控制）
- 注释内部

### 3.3 变量设置命令

#### 3.3.1 `\set` 命令

```
\set                          # 显示所有变量
\set name                     # 设置变量为空字符串
\set name value               # 设置变量值
\set name 'value with space'  # 值含空格时用引号
\set name "value with space"  # 双引号也支持
\set name :other_var          # 引用另一个变量的值
\set name :ENV_HOME/path      # 混合引用和字面量
```

#### 3.3.2 `\unset` 命令

```
\unset name                   # 删除变量
```

#### 3.3.3 `\show_variables` 命令

```
\show_variables               # 显示所有变量（同 \set 无参数）
\show_variables name          # 显示指定变量的值
```

#### 3.3.4 命令行预设

```bash
graphdb-cli -v limit=10 -v name=Alice
graphdb-cli --variable=limit=10 --variable=name=Alice
```

```rust
// cli.rs
#[derive(Parser)]
pub struct Cli {
    // ... 已有参数 ...

    #[arg(short = 'v', long = "variable", value_name = "NAME=VALUE")]
    pub variables: Vec<String>,
}
```

### 3.4 特殊变量行为

#### 3.4.1 ON_ERROR_STOP

```rust
async fn execute_script(&mut self, path: &str, session: &mut SessionManager) -> Result<()> {
    let commands = self.parse_script(&content);

    for cmd_str in commands {
        let command = parse_command(cmd_str);
        match self.execute(command, session).await {
            Ok(_) => {}
            Err(e) => {
                self.write_output(&self.formatter.format_error(&e.to_string()))?;
                if session.get_special_var("ON_ERROR_STOP") == Some(&"on".to_string()) {
                    return Err(e);
                }
            }
        }
    }
    Ok(())
}
```

#### 3.4.2 ECHO

```rust
enum EchoMode {
    None,      // 不回显
    Queries,   // 回显查询语句
    All,       // 回显所有（包括元命令生成的内部查询）
}

fn should_echo(session: &Session, is_internal: bool) -> bool {
    match session.get_echo_mode() {
        EchoMode::None => false,
        EchoMode::Queries => !is_internal,
        EchoMode::All => true,
    }
}
```

#### 3.4.3 TIMING

```rust
async fn execute_query(&mut self, query: &str, session: &mut SessionManager) -> Result<bool> {
    let show_timing = session.get_special_var("TIMING") == Some(&"on".to_string());

    let start = std::time::Instant::now();
    let result = session.execute_query(query).await;
    let elapsed = start.elapsed();

    match result {
        Ok(query_result) => {
            self.write_output(&self.formatter.format_result(&query_result))?;
            if show_timing {
                self.write_output(&format!("Time: {:.3}ms\n", elapsed.as_secs_f64() * 1000.0))?;
            }
        }
        Err(e) => { ... }
    }

    Ok(true)
}
```

### 3.5 变量持久化

#### 3.5.1 存储位置

```
~/.graphdb/
├── cli_variables.toml    # 用户变量持久化文件
└── config.toml           # CLI 配置文件（含特殊变量默认值）
```

#### 3.5.2 文件格式

```toml
# cli_variables.toml
[variables]
limit = "10"
space = "mygraph"
output_dir = "/tmp/graphdb_output"

[special]
ON_ERROR_STOP = "on"
ECHO = "queries"
TIMING = "on"
FORMAT = "table"
```

#### 3.5.3 加载与保存

```rust
impl VariableStore {
    pub fn load() -> Result<Self> {
        let path = Self::variables_path();
        if !path.exists() {
            return Ok(Self::default());
        }

        let content = std::fs::read_to_string(&path)?;
        let config: VariablesConfig = toml::from_str(&content)?;
        Ok(Self::from_config(config))
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::variables_path();
        let config = self.to_config();
        let content = toml::to_string_pretty(&config)?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        std::fs::write(&path, content)?;
        Ok(())
    }
}
```

**保存时机**：
- `\set` 和 `\unset` 命令执行后自动保存
- 退出时统一保存

### 3.6 变量与补全联动

变量名可通过 `:` 前缀触发补全（见 [补全设计](completion_design.md)）：

- 输入 `:` 后按 Tab → 显示所有变量名
- 输入 `:li` 后按 Tab → 补全为 `:limit`

变量值在补全描述中显示：

```
:limit       = 10
:name        = Alice
:output_dir  = /tmp/graphdb_output
```

## 4. 模块结构

### 4.1 文件组织

```
src/session/
├── mod.rs              # 模块导出
├── manager.rs          # 会话管理（已有，需修改）
├── variables.rs        # 变量管理（新增）
└── special_vars.rs     # 特殊变量定义（新增）
```

### 4.2 核心类型

```rust
// variables.rs
pub struct VariableStore {
    user_vars: HashMap<String, String>,
    special_vars: HashMap<String, SpecialVariable>,
}

pub struct SpecialVariable {
    name: String,
    default_value: String,
    current_value: String,
    var_type: SpecialVarType,
    description: String,
}

pub enum SpecialVarType {
    Boolean,
    Enum { values: Vec<String> },
    Number { min: i64, max: i64 },
    String,
}

// special_vars.rs
pub fn define_special_variables() -> Vec<SpecialVariable> {
    vec![
        SpecialVariable::new("ON_ERROR_STOP", "off", SpecialVarType::Boolean,
            "Stop execution on error (script mode)"),
        SpecialVariable::new("ECHO", "none", SpecialVarType::Enum {
            values: vec!["none".into(), "queries".into(), "all".into()]
        }, "Echo mode"),
        // ...
    ]
}
```

### 4.3 Session 变更

```rust
pub struct Session {
    pub session_id: i64,
    pub username: String,
    pub current_space: Option<String>,
    pub host: String,
    pub port: u16,
    pub connected: bool,
    variable_store: VariableStore,  // 替换原来的 HashMap<String, String>
}
```

## 5. 实现步骤

### Step 1: 实现 VariableStore（2 天）

- 新增 `variables.rs`，实现 `VariableStore`
- 支持用户变量和特殊变量
- 实现变量替换（`:name`、`:'name'`、`:"name"`、`::name`）
- 实现环境变量集成

### Step 2: 实现特殊变量（1 天）

- 新增 `special_vars.rs`，定义所有特殊变量
- 实现类型检查和值验证
- 实现 `ON_ERROR_STOP`、`ECHO`、`TIMING` 等行为

### Step 3: 集成到 Session（1 天）

- 修改 `Session` 使用 `VariableStore`
- 修改 `\set`、`\unset`、`\show_variables` 命令实现
- 修改查询执行前的变量替换

### Step 4: 命令行预设（1 天）

- 在 `Cli` 中添加 `-v`/`--variable` 参数
- 启动时将预设变量注入 `VariableStore`

### Step 5: 变量持久化（1 天）

- 实现 `cli_variables.toml` 的加载和保存
- 集成到 `\set`/`\unset` 命令

### Step 6: 测试（1 天）

- 变量替换测试（各种语法）
- 特殊变量行为测试
- 持久化测试
- 命令行预设测试

## 6. 测试用例

### 6.1 变量替换

| 输入 | 变量 | 替换结果 |
|------|------|----------|
| `LIMIT :limit` | `limit=10` | `LIMIT 10` |
| `WHERE p.name == :'name'` | `name=Alice` | `WHERE p.name == 'Alice'` |
| `WHERE p.id == :"id"` | `id=p1` | `WHERE p.id == "p1"` |
| `::not_a_var` | - | `::not_a_var` → `:not_a_var` |
| `:undefined` | 未定义 | 报错：Undefined variable |

### 6.2 特殊变量

| 设置 | 行为 |
|------|------|
| `\set ON_ERROR_STOP on` | 脚本中遇到错误立即停止 |
| `\set ECHO queries` | 执行查询前打印查询语句 |
| `\set TIMING on` | 查询后显示执行时间 |
| `\set FORMAT csv` | 默认输出格式改为 CSV |

### 6.3 命令行预设

```bash
graphdb-cli -v limit=10 -v name=Alice
# 在 CLI 中：
# :limit → 10
# :name → Alice
```

### 6.4 变量持久化

| 操作 | 预期 |
|------|------|
| `\set myvar hello` → 退出 → 重启 | `:myvar` 仍为 `hello` |
| `\unset myvar` → 退出 → 重启 | `:myvar` 不存在 |
