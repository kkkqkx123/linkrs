# GraphDB CLI 实现路线图

## 1. 项目阶段划分

### 阶段概览

```
Phase 1: MVP (最小可用产品) ────────> Phase 2: 用户体验优化 ────────> Phase 3: 实用增强
   │                                    │                               │
   ├─ 基础框架                         ├─ 自动补全                     ├─ 性能分析
   ├─ 连接管理                         ├─ 历史记录                     ├─ 数据导入导出
   ├─ 查询执行                         ├─ 多行编辑                     └─ 事务管理
   ├─ 基本元命令                       ├─ 变量管理
   └─ 输出格式化                       └─ 脚本执行
```

## 2. Phase 1: MVP（最小可用产品）

**目标**：实现基本的交互式查询功能，能够连接服务器并执行查询。

**预计时间**：2-3 周

### 2.1 项目初始化（第 1-2 天）

#### 任务列表

- [ ] 创建项目结构
  ```bash
  cargo new graphdb-cli --name graphdb_cli
  cd graphdb-cli
  ```

- [ ] 配置 Cargo.toml
  ```toml
  [package]
  name = "graphdb-cli"
  version = "0.1.0"
  edition = "2021"

  [dependencies]
  tokio = { version = "1.48", features = ["full"] }
  clap = { version = "4.5", features = ["derive"] }
  rustyline = "13.0"
  reqwest = { version = "0.11", features = ["json"] }
  serde = { version = "1.0", features = ["derive"] }
  serde_json = "1.0"
  tabled = "0.14"
  colored = "2.1"
  thiserror = "2.0"
  anyhow = "1.0"
  ```

- [ ] 创建基本目录结构
  ```
  src/
  ├── main.rs
  ├── cli.rs
  ├── lib.rs
  ├── input/
  ├── command/
  ├── output/
  ├── session/
  ├── client/
  └── utils/
  ```

### 2.2 命令行参数解析（第 3-4 天）

#### 实现内容

- [ ] 使用 clap 定义命令行参数
  ```rust
  use clap::Parser;

  #[derive(Parser)]
  #[clap(version = "0.1.0", author = "GraphDB Contributors")]
  struct Cli {
      #[clap(short, long, default_value = "127.0.0.1")]
      host: String,
      
      #[clap(short, long, default_value = "8080")]
      port: u16,
      
      #[clap(short, long, default_value = "root")]
      user: String,
      
      #[clap(short = 'W', long)]
      password: bool,
      
      #[clap(short, long)]
      database: Option<String>,
      
      #[clap(short, long)]
      command: Option<String>,
      
      #[clap(short, long)]
      file: Option<String>,
  }
  ```

- [ ] 实现参数验证和默认值处理

### 2.3 HTTP 客户端实现（第 5-7 天）

#### 实现内容

- [ ] 创建 HTTP 客户端模块
  ```rust
  pub struct HttpClient {
      client: reqwest::Client,
      base_url: String,
  }

  impl HttpClient {
      pub fn new(host: &str, port: u16) -> Self {
          let base_url = format!("http://{}:{}", host, port);
          Self {
              client: reqwest::Client::new(),
              base_url,
          }
      }

      pub async fn execute_query(&self, query: &str, session_id: &str) -> Result<QueryResult> {
          let url = format!("{}/api/v1/query", self.base_url);
          let request = QueryRequest {
              session_id: session_id.to_string(),
              query: query.to_string(),
          };
          
          let response = self.client
              .post(&url)
              .json(&request)
              .send()
              .await?;
          
          let result = response.json::<QueryResult>().await?;
          Ok(result)
      }
  }
  ```

- [ ] 实现认证逻辑
- [ ] 实现错误处理和重试机制

### 2.4 会话管理（第 8-10 天）

#### 实现内容

- [ ] 创建会话管理器
  ```rust
  pub struct SessionManager {
      client: HttpClient,
      session: Option<Session>,
  }

  pub struct Session {
      pub session_id: String,
      pub username: String,
      pub current_space: Option<String>,
      pub connected: bool,
  }

  impl SessionManager {
      pub async fn connect(&mut self, username: &str, password: &str) -> Result<()> {
          // 实现连接逻辑
      }

      pub async fn switch_space(&mut self, space: &str) -> Result<()> {
          // 实现切换 Space 逻辑
      }

      pub async fn execute_query(&mut self, query: &str) -> Result<QueryResult> {
          // 实现查询执行逻辑
      }
  }
  ```

- [ ] 实现会话状态维护
- [ ] 实现连接池（可选）

### 2.5 输入处理（第 11-13 天）

#### 实现内容

- [ ] 集成 rustyline
  ```rust
  use rustyline::Editor;

  pub struct InputHandler {
      editor: Editor<()>,
  }

  impl InputHandler {
      pub fn new() -> Result<Self> {
          let mut editor = Editor::new()?;
          Ok(Self { editor })
      }

      pub fn read_line(&mut self, prompt: &str) -> Result<String> {
          let line = self.editor.readline(prompt)?;
          self.editor.add_history_entry(line.as_str());
          Ok(line)
      }
  }
  ```

- [ ] 实现提示符生成
- [ ] 实现多行输入检测

### 2.6 输出格式化（第 14-16 天）

#### 实现内容

- [ ] 实现表格格式化
  ```rust
  use tabled::{Table, Tabled, settings::Style};

  pub fn format_table(result: &QueryResult) -> String {
      if result.rows.is_empty() {
          return "(0 rows)".to_string();
      }

      let headers = &result.columns;
      let rows: Vec<Vec<String>> = result.rows.iter()
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
  ```

- [ ] 实现 JSON 格式化
- [ ] 实现 CSV 格式化
- [ ] 实现分页器支持

### 2.7 基本元命令（第 17-19 天）

#### 实现内容

- [ ] 实现命令解析器
  ```rust
  pub enum Command {
      Query(String),
      MetaCommand(MetaCommand),
  }

  pub enum MetaCommand {
      Quit,
      Help,
      ShowSpaces,
      ShowTags { pattern: Option<String> },
      Format { format: OutputFormat },
      Connect { space: String },
  }

  pub fn parse_command(input: &str) -> Result<Command> {
      if input.starts_with('\\') {
          parse_meta_command(input)
      } else {
          Ok(Command::Query(input.to_string()))
      }
  }
  ```

- [ ] 实现基本元命令
  - `\q` - 退出
  - `\?` - 帮助
  - `\show_spaces` - 列出图空间
  - `\show_tags` - 列出 Tag
  - `\show_edges` - 列出 Edge
  - `\format` - 设置输出格式
  - `\connect` - 连接到图空间

### 2.8 主循环和集成（第 20-21 天）

#### 实现内容

- [ ] 实现主循环
  ```rust
  pub async fn run_repl(cli: Cli) -> Result<()> {
      let mut session_manager = SessionManager::new(&cli.host, cli.port);
      session_manager.connect(&cli.user, &get_password()?).await?;
      
      let mut input_handler = InputHandler::new()?;
      let output_formatter = OutputFormatter::new();
      
      loop {
          let prompt = generate_prompt(&session_manager);
          let line = input_handler.read_line(&prompt)?;
          
          let command = parse_command(&line)?;
          match execute_command(command, &mut session_manager, &output_formatter).await {
              Ok(should_continue) => if !should_continue { break; },
              Err(e) => eprintln!("Error: {}", e),
          }
      }
      
      Ok(())
  }
  ```

- [ ] 集成所有模块
- [ ] 端到端测试

### 2.9 测试和文档（第 22-23 天）

#### 实现内容

- [ ] 编写单元测试
- [ ] 编写集成测试
- [ ] 编写用户文档
- [ ] 编写 README

## 3. Phase 2: 用户体验优化

**目标**：提升用户体验，添加自动补全、历史记录等功能。

**预计时间**：2-3 周

### 3.1 自动补全（第 1-5 天）

#### 实现内容

- [ ] 实现关键字补全
  ```rust
  const KEYWORDS: &[&str] = &[
      "MATCH", "GO", "LOOKUP", "FETCH", "INSERT", "UPDATE", "DELETE",
      "CREATE", "ALTER", "DROP", "GRANT", "REVOKE",
      "RETURN", "WHERE", "ORDER", "LIMIT", "SKIP",
      "AND", "OR", "NOT", "IN", "AS",
  ];

  impl Completer for GraphDBCompleter {
      fn complete(&self, line: &str, pos: usize) -> Result<(usize, Vec<Pair>)> {
          let last_word = get_last_word(&line[..pos]);
          let completions = KEYWORDS.iter()
              .filter(|k| k.starts_with(&last_word))
              .map(|k| Pair {
                  display: k.to_string(),
                  replacement: k[last_word.len()..].to_string(),
              })
              .collect();
          
          Ok((pos - last_word.len(), completions))
      }
  }
  ```

- [ ] 实现对象名补全（Tag、Edge、属性）
- [ ] 实现函数名补全
- [ ] 实现上下文感知补全

### 3.2 历史记录增强（第 6-8 天）

#### 实现内容

- [ ] 实现历史记录持久化
  ```rust
  impl InputHandler {
      pub fn load_history(&mut self, path: &Path) -> Result<()> {
          self.editor.load_history(path)?;
          Ok(())
      }

      pub fn save_history(&mut self, path: &Path) -> Result<()> {
          self.editor.save_history(path)?;
          Ok(())
      }
  }
  ```

- [ ] 实现历史记录搜索（Ctrl+R）
- [ ] 实现历史记录限制（最大条数）

### 3.3 多行编辑（第 9-12 天）

#### 实现内容

- [ ] 实现语句完整性检测
  ```rust
  fn is_statement_complete(input: &str) -> bool {
      let trimmed = input.trim();
      
      // 检查是否以分号结尾
      if !trimmed.ends_with(';') {
          return false;
      }
      
      // 检查括号是否匹配
      let mut paren_count = 0;
      for ch in trimmed.chars() {
          match ch {
              '(' => paren_count += 1,
              ')' => paren_count -= 1,
              _ => {}
          }
      }
      
      paren_count == 0
  }
  ```

- [ ] 实现多行提示符
- [ ] 实现编辑器集成（`\e` 命令）

### 3.4 变量管理（第 13-15 天）

#### 实现内容

- [ ] 实现变量存储
  ```rust
  pub struct VariableManager {
      variables: HashMap<String, String>,
  }

  impl VariableManager {
      pub fn set(&mut self, name: String, value: String) {
          self.variables.insert(name, value);
      }

      pub fn get(&self, name: &str) -> Option<&String> {
          self.variables.get(name)
      }

      pub fn unset(&mut self, name: &str) {
          self.variables.remove(name);
      }

      pub fn substitute(&self, input: &str) -> String {
          let mut result = input.to_string();
          for (name, value) in &self.variables {
              result = result.replace(&format!(":{}", name), value);
          }
          result
      }
  }
  ```

- [ ] 实现变量替换
- [ ] 实现内置变量（如 `:NOW`, `:USER` 等）

### 3.5 脚本执行（第 16-18 天）

#### 实现内容

- [ ] 实现文件执行
  ```rust
  pub async fn execute_file(path: &Path, session: &mut SessionManager) -> Result<()> {
      let content = fs::read_to_string(path)?;
      let commands = parse_commands(&content)?;
      
      for command in commands {
          match command {
              Command::Query(query) => {
                  let result = session.execute_query(&query).await?;
                  println!("{}", format_result(&result));
              }
              Command::MetaCommand(meta) => {
                  execute_meta_command(meta, session).await?;
              }
          }
      }
      
      Ok(())
  }
  ```

- [ ] 实现批处理模式
- [ ] 实现错误处理和继续执行选项

### 3.6 配置管理（第 19-21 天）

#### 实现内容

- [ ] 实现配置文件加载
  ```rust
  #[derive(Deserialize)]
  pub struct Config {
      pub connection: ConnectionConfig,
      pub output: OutputConfig,
      pub editor: EditorConfig,
  }

  impl Config {
      pub fn load() -> Result<Self> {
          let config_path = Self::get_config_path()?;
          if config_path.exists() {
              let content = fs::read_to_string(&config_path)?;
              let config: Config = toml::from_str(&content)?;
              Ok(config)
          } else {
              Ok(Self::default())
          }
      }
  }
  ```

- [ ] 实现配置命令（`\config`）
- [ ] 实现配置热加载

## 4. Phase 3: 实用增强

**目标**：添加性能分析、数据导入导出等实用功能。

**预计时间**：2-3 周

### 4.1 性能分析（第 1-5 天）

#### 实现内容

- [ ] 实现执行计划展示
  ```rust
  pub fn format_explain(result: &ExplainResult) -> String {
      let mut output = String::new();
      output.push_str("Query Plan:\n");
      output.push_str(&format_table(&result.plan_table));
      
      if let Some(stats) = &result.stats {
          output.push_str(&format!("\nExecution Statistics:\n"));
          output.push_str(&format!("  Total time: {}ms\n", stats.total_time_ms));
          output.push_str(&format!("  Rows processed: {}\n", stats.rows_processed));
      }
      
      output
  }
  ```

- [ ] 实现性能统计
- [ ] 实现 `\timing` 命令显示执行时间

### 4.2 数据导入导出（第 6-12 天）

#### 实现内容

- [ ] 实现 CSV 导入
  ```rust
  pub async fn import_csv(
      path: &Path,
      tag: &str,
      session: &mut SessionManager,
  ) -> Result<ImportStats> {
      let file = File::open(path)?;
      let reader = BufReader::new(file);
      let mut csv_reader = csv::Reader::from_reader(reader);
      
      let headers = csv_reader.headers()?.clone();
      let mut stats = ImportStats::default();
      
      for result in csv_reader.records() {
          let record = result?;
          let query = build_insert_query(tag, &headers, &record)?;
          session.execute_query(&query).await?;
          stats.rows_imported += 1;
      }
      
      Ok(stats)
  }
  ```

- [ ] 实现 CSV 导出
- [ ] 实现 JSON 导入导出
- [ ] 添加元命令 `\import` 和 `\export`

### 4.3 事务管理（第 13-17 天）

#### 实现内容

- [ ] 实现事务控制命令
  ```rust
  pub struct TransactionManager {
      in_transaction: bool,
      transaction_id: Option<String>,
  }

  impl TransactionManager {
      pub async fn begin(&mut self, session: &mut SessionManager) -> Result<()> {
          if self.in_transaction {
              return Err(CliError::TransactionAlreadyActive);
          }
          
          session.execute_query("BEGIN TRANSACTION").await?;
          self.in_transaction = true;
          Ok(())
      }

      pub async fn commit(&mut self, session: &mut SessionManager) -> Result<()> {
          if !self.in_transaction {
              return Err(CliError::NoActiveTransaction);
          }
          
          session.execute_query("COMMIT").await?;
          self.in_transaction = false;
          Ok(())
      }
  }
  ```

- [ ] 实现事务状态提示符
- [ ] 实现自动提交模式切换
- [ ] 添加元命令 `\begin`, `\commit`, `\rollback`

## 5. 测试计划

### 5.1 单元测试

每个模块都需要编写单元测试，测试覆盖率目标：70%

**关键测试点**：
- 命令解析
- 输出格式化
- 变量替换
- 自动补全

### 5.2 集成测试

**测试场景**：
- 连接管理
- 查询执行
- 事务处理
- 脚本执行

### 5.3 端到端测试

**测试流程**：
1. 启动 GraphDB 服务器
2. 启动 CLI 客户端
3. 执行完整的用户操作流程
4. 验证结果
