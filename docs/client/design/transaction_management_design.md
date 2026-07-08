# 事务管理设计方案

## 1. 概述

### 1.1 目标

为 GraphDB CLI 提供事务管理功能，支持显式事务控制、自动提交模式切换、事务状态提示，确保数据操作的原子性和一致性。

### 1.2 参考实现

- **psql**：`BEGIN`、`COMMIT`、`ROLLBACK`，`\set AUTOCOMMIT` 变量
- **MySQL**：`START TRANSACTION`、`COMMIT`、`ROLLBACK`，`autocommit` 变量
- **neo4j-cli**：`:begin`、`:commit`、`:rollback` 命令

## 2. 功能需求

### 2.1 核心功能

| 功能              | 说明                                       |
| ----------------- | ------------------------------------------ |
| 显式事务控制      | 支持 `BEGIN`、`COMMIT`、`ROLLBACK` 命令    |
| 自动提交模式      | 支持自动提交和手动提交模式切换              |
| 事务状态提示      | 在提示符中显示当前事务状态                  |
| 事务隔离级别      | 支持设置事务隔离级别                        |
| 保存点            | 支持 `SAVEPOINT` 和 `ROLLBACK TO`          |
| 事务超时          | 支持设置事务超时时间                        |
| 退出保护          | 有未提交事务时警告用户                      |

### 2.2 元命令

| 命令                           | 说明                           |
| ------------------------------ | ------------------------------ |
| `\begin`                       | 开始一个新事务                 |
| `\commit`                      | 提交当前事务                   |
| `\rollback`                    | 回滚当前事务                   |
| `\autocommit [on\|off]`        | 切换自动提交模式               |
| `\isolation [level]`           | 设置或显示隔离级别             |
| `\savepoint <name>`            | 创建保存点                     |
| `\rollback_to <name>`          | 回滚到保存点                   |
| `\release <name>`              | 释放保存点                     |
| `\txstatus`                    | 显示事务状态                   |

### 2.3 事务隔离级别

| 隔离级别           | 说明                                       |
| ------------------ | ------------------------------------------ |
| READ UNCOMMITTED   | 读未提交，可能读到脏数据                   |
| READ COMMITTED     | 读已提交，避免脏读                         |
| REPEATABLE READ    | 可重复读，避免不可重复读                   |
| SERIALIZABLE       | 可串行化，最高隔离级别                     |

## 3. 架构设计

### 3.1 模块结构

```
src/
├── transaction/
│   ├── mod.rs              # 模块导出
│   ├── manager.rs          # 事务管理器
│   ├── state.rs            # 事务状态
│   └── isolation.rs        # 隔离级别定义
└── command/
    └── executor.rs         # 集成事务命令
```

### 3.2 核心数据结构

```rust
pub struct TransactionManager {
    state: TransactionState,
    autocommit: bool,
    isolation_level: IsolationLevel,
    savepoints: Vec<Savepoint>,
    started_at: Option<std::time::Instant>,
    timeout: Option<std::time::Duration>,
    query_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TransactionState {
    Idle,
    Active {
        id: String,
        space: String,
    },
    Failed {
        id: String,
        error: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IsolationLevel {
    ReadUncommitted,
    ReadCommitted,
    RepeatableRead,
    Serializable,
}

#[derive(Debug, Clone)]
pub struct Savepoint {
    pub name: String,
    pub created_at: std::time::Instant,
    pub query_count: usize,
}

#[derive(Debug, Clone)]
pub struct TransactionInfo {
    pub state: TransactionState,
    pub autocommit: bool,
    pub isolation_level: IsolationLevel,
    pub duration_ms: Option<u64>,
    pub query_count: usize,
    pub savepoints: Vec<String>,
}
```

### 3.3 事务管理器实现

```rust
impl TransactionManager {
    pub fn new() -> Self {
        Self {
            state: TransactionState::Idle,
            autocommit: true,
            isolation_level: IsolationLevel::ReadCommitted,
            savepoints: Vec::new(),
            started_at: None,
            timeout: None,
            query_count: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        matches!(self.state, TransactionState::Active { .. })
    }

    pub fn is_failed(&self) -> bool {
        matches!(self.state, TransactionState::Failed { .. })
    }

    pub fn autocommit(&self) -> bool {
        self.autocommit
    }

    pub fn set_autocommit(&mut self, value: bool) {
        self.autocommit = value;
    }

    pub fn isolation_level(&self) -> IsolationLevel {
        self.isolation_level
    }

    pub fn set_isolation_level(&mut self, level: IsolationLevel) {
        self.isolation_level = level;
    }

    pub fn begin(&mut self, session: &mut SessionManager) -> Result<()> {
        if self.is_active() {
            return Err(CliError::TransactionAlreadyActive);
        }

        let tx_id = session.execute_query("BEGIN TRANSACTION").await?;
        
        self.state = TransactionState::Active {
            id: format!("tx_{}", uuid::Uuid::new_v4()),
            space: session.current_space().unwrap_or("").to_string(),
        };
        self.started_at = Some(std::time::Instant::now());
        self.query_count = 0;
        self.savepoints.clear();

        Ok(())
    }

    pub async fn commit(&mut self, session: &mut SessionManager) -> Result<()> {
        if !self.is_active() {
            return Err(CliError::NoActiveTransaction);
        }

        session.execute_query("COMMIT").await?;
        
        self.state = TransactionState::Idle;
        self.started_at = None;
        self.savepoints.clear();

        Ok(())
    }

    pub async fn rollback(&mut self, session: &mut SessionManager) -> Result<()> {
        if !self.is_active() {
            return Err(CliError::NoActiveTransaction);
        }

        session.execute_query("ROLLBACK").await?;
        
        self.state = TransactionState::Idle;
        self.started_at = None;
        self.savepoints.clear();

        Ok(())
    }

    pub async fn create_savepoint(
        &mut self,
        name: &str,
        session: &mut SessionManager,
    ) -> Result<()> {
        if !self.is_active() {
            return Err(CliError::NoActiveTransaction);
        }

        session.execute_query(&format!("SAVEPOINT {}", name)).await?;
        
        self.savepoints.push(Savepoint {
            name: name.to_string(),
            created_at: std::time::Instant::now(),
            query_count: self.query_count,
        });

        Ok(())
    }

    pub async fn rollback_to_savepoint(
        &mut self,
        name: &str,
        session: &mut SessionManager,
    ) -> Result<()> {
        if !self.is_active() {
            return Err(CliError::NoActiveTransaction);
        }

        let pos = self.savepoints.iter()
            .position(|s| s.name == name)
            .ok_or_else(|| CliError::SavepointNotFound(name.to_string()))?;

        session.execute_query(&format!("ROLLBACK TO SAVEPOINT {}", name)).await?;
        
        let savepoint = &self.savepoints[pos];
        self.query_count = savepoint.query_count;
        self.savepoints.truncate(pos + 1);

        Ok(())
    }

    pub async fn release_savepoint(
        &mut self,
        name: &str,
        session: &mut SessionManager,
    ) -> Result<()> {
        if !self.is_active() {
            return Err(CliError::NoActiveTransaction);
        }

        let pos = self.savepoints.iter()
            .position(|s| s.name == name)
            .ok_or_else(|| CliError::SavepointNotFound(name.to_string()))?;

        session.execute_query(&format!("RELEASE SAVEPOINT {}", name)).await?;
        
        self.savepoints.remove(pos);

        Ok(())
    }

    pub fn record_query(&mut self) {
        self.query_count += 1;
    }

    pub fn mark_failed(&mut self, error: String) {
        if let TransactionState::Active { id, space } = &self.state {
            self.state = TransactionState::Failed {
                id: id.clone(),
                error,
            };
        }
    }

    pub fn check_timeout(&self) -> Result<()> {
        if let (Some(started), Some(timeout)) = (self.started_at, self.timeout) {
            if started.elapsed() > timeout {
                return Err(CliError::TransactionTimeout);
            }
        }
        Ok(())
    }

    pub fn info(&self) -> TransactionInfo {
        TransactionInfo {
            state: self.state.clone(),
            autocommit: self.autocommit,
            isolation_level: self.isolation_level,
            duration_ms: self.started_at.map(|t| t.elapsed().as_millis() as u64),
            query_count: self.query_count,
            savepoints: self.savepoints.iter().map(|s| s.name.clone()).collect(),
        }
    }
}
```

## 4. 提示符扩展

### 4.1 事务状态提示

```rust
impl Session {
    pub fn prompt(&self, tx_manager: &TransactionManager) -> String {
        let mut prompt = String::new();
        
        prompt.push_str("graphdb");
        
        if let Some(space) = &self.current_space {
            prompt.push_str(&format!("({})", space));
        }
        
        if tx_manager.is_active() {
            prompt.push_str("*");
        }
        
        if !tx_manager.autocommit() {
            prompt.push_str("!");
        }
        
        prompt.push_str("=# ");
        prompt
    }
}
```

**提示符示例**：

| 状态                           | 提示符                |
| ------------------------------ | --------------------- |
| 无事务，自动提交               | `graphdb(test)=# `    |
| 事务激活                       | `graphdb(test)*=# `   |
| 非自动提交模式                 | `graphdb(test)!=# `   |
| 事务激活且非自动提交           | `graphdb(test)*!=# `  |
| 事务失败                       | `graphdb(test)!># `   |

## 5. 自动提交模式

### 5.1 自动提交逻辑

```rust
impl CommandExecutor {
    async fn execute_query(
        &mut self,
        query: &str,
        session_mgr: &mut SessionManager,
    ) -> Result<bool> {
        let query = query.trim();
        
        if query.is_empty() {
            return Ok(true);
        }

        if !self.conditional_stack.is_active() {
            return Ok(true);
        }

        let is_transactional = self.is_transactional_query(query);
        
        if self.tx_manager.autocommit() && is_transactional {
            self.tx_manager.begin(session_mgr).await?;
        }

        match session_mgr.execute_query(query).await {
            Ok(result) => {
                self.tx_manager.record_query();
                
                if self.tx_manager.autocommit() && is_transactional {
                    self.tx_manager.commit(session_mgr).await?;
                }
                
                let output = self.formatter.format_result(&result);
                self.write_output(&output)?;
            }
            Err(e) => {
                if self.tx_manager.is_active() {
                    self.tx_manager.mark_failed(e.to_string());
                }
                return Err(e);
            }
        }

        Ok(true)
    }

    fn is_transactional_query(&self, query: &str) -> bool {
        let upper = query.trim_start().to_uppercase();
        
        upper.starts_with("INSERT") ||
        upper.starts_with("UPDATE") ||
        upper.starts_with("DELETE") ||
        upper.starts_with("CREATE") ||
        upper.starts_with("ALTER") ||
        upper.starts_with("DROP")
    }
}
```

### 5.2 自动提交切换

```rust
MetaCommand::Autocommit { value } => {
    if let Some(v) = value {
        let enabled = match v.to_lowercase().as_str() {
            "on" | "true" | "1" => true,
            "off" | "false" | "0" => false,
            _ => return Err(CliError::InvalidValue(format!("Invalid autocommit value: {}", v))),
        };
        
        if !enabled && self.tx_manager.is_active() {
            return Err(CliError::TransactionAlreadyActive);
        }
        
        self.tx_manager.set_autocommit(enabled);
        self.write_output(&format!(
            "Autocommit {}.",
            if enabled { "enabled" } else { "disabled" }
        ))?;
    } else {
        self.write_output(&format!(
            "Autocommit is {}.",
            if self.tx_manager.autocommit() { "on" } else { "off" }
        ))?;
    }
    Ok(true)
}
```

## 6. 事务隔离级别

### 6.1 隔离级别设置

```rust
impl IsolationLevel {
    pub fn as_str(&self) -> &'static str {
        match self {
            IsolationLevel::ReadUncommitted => "READ UNCOMMITTED",
            IsolationLevel::ReadCommitted => "READ COMMITTED",
            IsolationLevel::RepeatableRead => "REPEATABLE READ",
            IsolationLevel::Serializable => "SERIALIZABLE",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_uppercase().replace('_', " ").as_str() {
            "READ UNCOMMITTED" => Some(IsolationLevel::ReadUncommitted),
            "READ COMMITTED" => Some(IsolationLevel::ReadCommitted),
            "REPEATABLE READ" => Some(IsolationLevel::RepeatableRead),
            "SERIALIZABLE" => Some(IsolationLevel::Serializable),
            _ => None,
        }
    }
}

MetaCommand::Isolation { level } => {
    if let Some(l) = level {
        let isolation = IsolationLevel::from_str(&l)
            .ok_or_else(|| CliError::InvalidValue(format!("Invalid isolation level: {}", l)))?;
        
        if self.tx_manager.is_active() {
            return Err(CliError::TransactionAlreadyActive);
        }
        
        self.tx_manager.set_isolation_level(isolation);
        self.write_output(&format!(
            "Isolation level set to: {}",
            isolation.as_str()
        ))?;
    } else {
        self.write_output(&format!(
            "Current isolation level: {}",
            self.tx_manager.isolation_level().as_str()
        ))?;
    }
    Ok(true)
}
```

### 6.2 事务开始时应用隔离级别

```rust
pub async fn begin(&mut self, session: &mut SessionManager) -> Result<()> {
    if self.is_active() {
        return Err(CliError::TransactionAlreadyActive);
    }

    let query = format!(
        "BEGIN TRANSACTION ISOLATION LEVEL {}",
        self.isolation_level.as_str()
    );
    
    session.execute_query(&query).await?;
    
    self.state = TransactionState::Active {
        id: format!("tx_{}", uuid::Uuid::new_v4()),
        space: session.current_space().unwrap_or("").to_string(),
    };
    self.started_at = Some(std::time::Instant::now());
    self.query_count = 0;
    self.savepoints.clear();

    Ok(())
}
```

## 7. 事务状态显示

### 7.1 状态命令

```rust
MetaCommand::TxStatus => {
    let info = self.tx_manager.info();
    let output = self.format_transaction_status(&info);
    self.write_output(&output)?;
    Ok(true)
}

fn format_transaction_status(&self, info: &TransactionInfo) -> String {
    let mut output = String::new();
    
    output.push_str("─────────────────────────────────────────────────────────────\n");
    output.push_str("Transaction Status\n");
    output.push_str("─────────────────────────────────────────────────────────────\n");
    
    match &info.state {
        TransactionState::Idle => {
            output.push_str("State:           Idle\n");
        }
        TransactionState::Active { id, space } => {
            output.push_str(&format!("State:           Active\n"));
            output.push_str(&format!("Transaction ID:  {}\n", id));
            output.push_str(&format!("Space:           {}\n", space));
        }
        TransactionState::Failed { id, error } => {
            output.push_str(&format!("State:           Failed\n"));
            output.push_str(&format!("Transaction ID:  {}\n", id));
            output.push_str(&format!("Error:           {}\n", error));
        }
    }
    
    output.push_str(&format!("Autocommit:      {}\n", if info.autocommit { "on" } else { "off" }));
    output.push_str(&format!("Isolation:       {}\n", info.isolation_level.as_str()));
    
    if let Some(duration) = info.duration_ms {
        output.push_str(&format!("Duration:        {:.3} s\n", duration as f64 / 1000.0));
    }
    
    output.push_str(&format!("Queries:         {}\n", info.query_count));
    
    if !info.savepoints.is_empty() {
        output.push_str(&format!("Savepoints:      {}\n", info.savepoints.join(", ")));
    }
    
    output
}
```

## 8. 退出保护

### 8.1 退出检查

```rust
MetaCommand::Quit => {
    if self.tx_manager.is_active() {
        self.write_output("WARNING: There is an active transaction.")?;
        self.write_output("Use \\commit or \\rollback before quitting, or use \\q! to force quit.")?;
        return Ok(true);
    }
    
    if !self.tx_manager.autocommit() {
        self.write_output("WARNING: Autocommit is disabled. Uncommitted changes may be lost.")?;
    }
    
    self.write_output("Goodbye!")?;
    Ok(false)
}

MetaCommand::ForceQuit => {
    if self.tx_manager.is_active() {
        self.write_output("Rolling back active transaction...")?;
        self.tx_manager.rollback(session_mgr).await?;
    }
    
    self.write_output("Goodbye!")?;
    Ok(false)
}
```

## 9. 错误处理

### 9.1 错误类型

```rust
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("Transaction already active")]
    TransactionAlreadyActive,
    
    #[error("No active transaction")]
    NoActiveTransaction,
    
    #[error("Savepoint not found: {0}")]
    SavepointNotFound(String),
    
    #[error("Transaction timeout")]
    TransactionTimeout,
    
    #[error("Transaction is in failed state: {0}")]
    TransactionFailed(String),
    
    #[error("Cannot change autocommit while transaction is active")]
    CannotChangeAutocommit,
    
    #[error("Invalid value: {0}")]
    InvalidValue(String),
}
```

### 9.2 失败事务处理

```rust
async fn execute_query(&mut self, query: &str, session_mgr: &mut SessionManager) -> Result<bool> {
    if self.tx_manager.is_failed() {
        return Err(CliError::TransactionFailed(
            self.tx_manager.info().state.error_message().unwrap_or_default()
        ));
    }
    
    // ... 正常查询执行
}
```

## 10. HTTP 客户端扩展

### 10.1 事务 API

```rust
impl GraphDBHttpClient {
    pub async fn begin_transaction(
        &self,
        session_id: i64,
        isolation_level: Option<&str>,
    ) -> Result<String> {
        let url = format!("{}/transaction/begin", self.base_url);
        
        let mut body = serde_json::Map::new();
        body.insert("session_id".to_string(), json!(session_id));
        if let Some(level) = isolation_level {
            body.insert("isolation_level".to_string(), json!(level));
        }
        
        let response = self.client.post(&url).json(&body).send().await?;
        let result: TransactionResponse = response.json().await?;
        
        Ok(result.transaction_id)
    }

    pub async fn commit_transaction(&self, session_id: i64) -> Result<()> {
        let url = format!("{}/transaction/commit", self.base_url);
        
        let body = json!({ "session_id": session_id });
        self.client.post(&url).json(&body).send().await?;
        
        Ok(())
    }

    pub async fn rollback_transaction(&self, session_id: i64) -> Result<()> {
        let url = format!("{}/transaction/rollback", self.base_url);
        
        let body = json!({ "session_id": session_id });
        self.client.post(&url).json(&body).send().await?;
        
        Ok(())
    }

    pub async fn create_savepoint(
        &self,
        session_id: i64,
        name: &str,
    ) -> Result<()> {
        let url = format!("{}/transaction/savepoint", self.base_url);
        
        let body = json!({
            "session_id": session_id,
            "name": name,
            "action": "create"
        });
        self.client.post(&url).json(&body).send().await?;
        
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct TransactionResponse {
    transaction_id: String,
}
```

## 11. 命令解析器扩展

### 11.1 新增元命令

```rust
pub enum MetaCommand {
    Begin,
    Commit,
    Rollback,
    Autocommit { value: Option<String> },
    Isolation { level: Option<String> },
    Savepoint { name: String },
    RollbackTo { name: String },
    Release { name: String },
    TxStatus,
    ForceQuit,
}

fn parse_meta_command(input: &str) -> Result<Command> {
    let trimmed = input.trim_start_matches('\\');
    
    match trimmed.split_whitespace().next() {
        Some("begin") => Ok(Command::MetaCommand(MetaCommand::Begin)),
        Some("commit") => Ok(Command::MetaCommand(MetaCommand::Commit)),
        Some("rollback") => {
            let rest = trimmed.trim_start_matches("rollback").trim();
            if rest.starts_with("to") {
                let name = rest.trim_start_matches("to").trim().to_string();
                Ok(Command::MetaCommand(MetaCommand::RollbackTo { name }))
            } else {
                Ok(Command::MetaCommand(MetaCommand::Rollback))
            }
        }
        Some("autocommit") => {
            let value = trimmed.split_whitespace().nth(1).map(|s| s.to_string());
            Ok(Command::MetaCommand(MetaCommand::Autocommit { value }))
        }
        Some("isolation") => {
            let level = trimmed.split_whitespace().nth(1).map(|s| s.to_string());
            Ok(Command::MetaCommand(MetaCommand::Isolation { level }))
        }
        Some("savepoint") => {
            let name = trimmed.split_whitespace().nth(1)
                .ok_or_else(|| anyhow!("Savepoint name required"))?
                .to_string();
            Ok(Command::MetaCommand(MetaCommand::Savepoint { name }))
        }
        Some("release") => {
            let name = trimmed.split_whitespace().nth(1)
                .ok_or_else(|| anyhow!("Savepoint name required"))?
                .to_string();
            Ok(Command::MetaCommand(MetaCommand::Release { name }))
        }
        Some("txstatus") => Ok(Command::MetaCommand(MetaCommand::TxStatus)),
        Some("q!") => Ok(Command::MetaCommand(MetaCommand::ForceQuit)),
        _ => parse_other_meta_command(trimmed),
    }
}
```

## 12. 测试用例

### 12.1 基本事务控制

| 操作                           | 预期结果                       |
| ------------------------------ | ------------------------------ |
| `\begin`                       | 开始事务，提示符显示 `*`       |
| 执行 INSERT                    | 查询计数增加                   |
| `\commit`                      | 提交事务，提示符恢复正常       |
| `\begin` + `\rollback`         | 回滚事务                       |

### 12.2 自动提交模式

| 操作                           | 预期结果                       |
| ------------------------------ | ------------------------------ |
| `\autocommit off`              | 禁用自动提交                   |
| 执行 INSERT                    | 不自动提交                     |
| `\commit`                      | 手动提交                       |
| `\autocommit on`               | 启用自动提交                   |
| 执行 INSERT                    | 自动提交                       |

### 12.3 保存点

| 操作                           | 预期结果                       |
| ------------------------------ | ------------------------------ |
| `\begin`                       | 开始事务                       |
| INSERT 1                       | 插入数据                       |
| `\savepoint sp1`               | 创建保存点                     |
| INSERT 2                       | 插入更多数据                   |
| `\rollback to sp1`             | 回滚到保存点                   |
| `\commit`                      | 只有 INSERT 1 被提交           |

### 12.4 退出保护

| 操作                           | 预期结果                       |
| ------------------------------ | ------------------------------ |
| `\begin` + `\q`                | 显示警告，不退出               |
| `\begin` + `\q!`               | 强制退出，自动回滚             |

## 13. 实现步骤

### Step 1: 实现事务管理器（1 天）

- 定义数据结构
- 实现状态管理
- 实现保存点逻辑

### Step 2: 扩展提示符（0.5 天）

- 修改 `Session::prompt()`
- 显示事务状态

### Step 3: 实现自动提交模式（1 天）

- 实现自动提交逻辑
- 实现模式切换

### Step 4: 实现隔离级别（0.5 天）

- 定义隔离级别枚举
- 实现设置和显示

### Step 5: 实现元命令（1 天）

- 添加命令解析
- 集成到命令执行器
- 实现退出保护

### Step 6: 扩展 HTTP 客户端（1 天）

- 添加事务 API
- 处理服务端响应

### Step 7: 测试（0.5 天）

- 单元测试
- 集成测试
- 边界情况测试
