# 历史记录设计方案

## 1. 概述

### 1.1 目标

为 GraphDB CLI 提供完善的命令历史管理功能，包括持久化存储、增量搜索、历史导航、去重和限制，参考 psql 和 rustyline 的最佳实践。

### 1.2 参考实现

- **psql**：基于 libedit/readline 的历史管理，支持 Ctrl+R 增量搜索、上下箭头导航、`~/.psql_history` 持久化
- **rustyline**：内置 `DefaultHistory`，支持 `Ctrl+R`/`Ctrl+S` 搜索、历史限制、文件持久化
- **usql**：自动保存命令历史，启动时加载

## 2. 现状分析

### 2.1 Phase 1 已实现

当前 `InputHandler`（`src/input/handler.rs`）中的历史功能：

```rust
// 创建 Editor 时设置自动添加历史
editor.set_auto_add_history(true);

// 启动时加载历史
let _ = editor.load_history(&history_path);

// 退出时保存历史
pub fn save_history(&mut self) {
    let _ = self.editor.save_history(&history_path);
}
```

### 2.2 不足之处

| 问题 | 说明 |
|------|------|
| 无历史限制 | 历史文件无限增长 |
| 无去重 | 相同命令重复出现在历史中 |
| 无 Ctrl+R 搜索提示 | rustyline 内置但无 UI 提示 |
| 无会话隔离 | 所有会话共享同一历史文件 |
| 无按 Space 过滤 | 无法按当前 Space 过滤历史 |
| 保存时机单一 | 仅在退出时保存，异常退出丢失历史 |
| 无历史管理命令 | 无 `\history` 等查看/管理命令 |

## 3. 功能设计

### 3.1 历史持久化

#### 3.1.1 存储位置

```
~/.graphdb/
├── cli_history          # 全局历史文件
├── cli_history.lock     # 文件锁（多实例互斥）
└── history/
    ├── default.hist     # 默认 Space 历史
    └── mygraph.hist     # mygraph Space 专属历史
```

#### 3.1.2 历史文件格式

采用简洁的文本格式，每行一条命令，以时间戳前缀标记：

```
#1700000000|MATCH (p:person) RETURN p.name LIMIT 10;
#1700000005|SHOW SPACES;
#1700000010|INSERT VERTEX person(name, age) VALUES "p1":("Alice", 30);
```

**格式说明**：
- `#` 开头的行为带时间戳的历史条目
- `#<unix_timestamp>|<command>` 格式
- 不以 `#` 开头的行为无时间戳的旧格式（向后兼容）
- 空行忽略

#### 3.1.3 保存策略

```rust
pub enum HistorySavePolicy {
    OnExit,       // 退出时保存（当前实现）
    Incremental,  // 每条命令执行后立即保存
    Timed,        // 定时保存（每 N 秒）
}
```

**推荐策略**：`Incremental`（增量保存）

- 每条命令执行后立即追加到历史文件
- 避免异常退出导致历史丢失
- 使用追加模式写入，性能开销极小

### 3.2 历史限制

#### 3.2.1 内存限制

```rust
const DEFAULT_MAX_HISTORY_SIZE: usize = 5000;
const MAX_HISTORY_SIZE: usize = 100000;
```

- 默认保留最近 5000 条历史
- 可通过配置文件调整
- 超出限制时自动淘汰最旧的条目

#### 3.2.2 文件限制

```rust
const DEFAULT_MAX_HISTORY_FILE_SIZE: usize = 10 * 1024 * 1024; // 10MB
```

- 历史文件超过限制时自动截断
- 保留最近的条目，删除最旧的

#### 3.2.3 去重策略

```rust
pub enum HistoryDedupPolicy {
    None,                    // 不去重
    Consecutive,             // 去除连续重复（推荐）
    Global,                  // 全局去重（保留最新位置）
}
```

**推荐策略**：`Consecutive`（连续去重）

- 连续输入相同命令时只保留一条
- 非连续的相同命令保留（因为可能在不同上下文中有意义）
- 添加新条目前检查最后一条是否相同

### 3.3 历史搜索

#### 3.3.1 增量搜索（Ctrl+R / Ctrl+S）

rustyline 内置支持 `Ctrl+R`（反向搜索）和 `Ctrl+S`（正向搜索），无需额外实现。

**增强**：在搜索提示中显示匹配数量

```
(reverse-i-search)`MATCH': MATCH (p:person) RETURN p.name LIMIT 10;
```

#### 3.3.2 历史搜索命令

新增 `\history` 元命令，支持搜索和浏览历史：

```
\history                      # 显示最近 20 条历史
\history 50                   # 显示最近 50 条历史
\history search MATCH         # 搜索包含 MATCH 的历史
\history clear                # 清空历史
```

**输出格式**：

```
  ID  | Time                | Command
------+---------------------+-------------------------------------------
  1   | 2024-01-15 10:00:00 | MATCH (p:person) RETURN p.name LIMIT 10;
  2   | 2024-01-15 10:00:05 | SHOW SPACES;
  3   | 2024-01-15 10:00:10 | INSERT VERTEX person(name, age) VALUES ...
```

#### 3.3.3 历史重执行

```
\history exec 3               # 重新执行第 3 条历史命令
\history edit 3               # 将第 3 条历史命令加载到编辑缓冲区
```

### 3.4 多会话历史

#### 3.4.1 问题

多个 CLI 实例同时运行时，历史文件可能冲突：
- 实例 A 和 B 同时启动，都读取了相同的历史
- A 执行命令后保存，覆盖了 B 的历史

#### 3.4.2 解决方案

**方案：文件锁 + 合并写入**

```rust
pub struct HistoryManager {
    history_path: PathBuf,
    max_size: usize,
    dedup: HistoryDedupPolicy,
}

impl HistoryManager {
    pub fn load(&self) -> Result<Vec<HistoryEntry>> {
        let file = OpenOptions::new()
            .read(true)
            .open(&self.history_path)?;
        // 读取并解析
    }

    pub fn append(&self, entry: &HistoryEntry) -> Result<()> {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.history_path)?;
        writeln!(file, "#{}|{}", entry.timestamp, entry.command)?;
        Ok(())
    }

    pub fn compact(&self, entries: &[HistoryEntry]) -> Result<()> {
        // 去重 + 截断后重写文件
    }
}
```

**流程**：
1. 启动时加载历史文件
2. 每条命令执行后追加写入（append mode）
3. 退出时执行 compact（去重 + 截断）
4. 使用文件锁防止并发写入冲突

### 3.5 按 Space 过滤

#### 3.5.1 设计

历史条目可选关联 Space 信息：

```
#1700000000|[mygraph]|MATCH (p:person) RETURN p.name LIMIT 10;
#1700000005|[default]|SHOW SPACES;
#1700000010|[mygraph]|INSERT VERTEX person(name, age) VALUES "p1":("Alice", 30);
```

#### 3.5.2 过滤行为

- `Ctrl+R` 搜索时默认搜索所有历史
- 配置 `history.space_filter = true` 时，仅搜索当前 Space 的历史
- `\history` 命令默认显示所有历史，加 `--space` 参数过滤

## 4. 模块结构

### 4.1 文件组织

```
src/input/
├── mod.rs              # 模块导出
├── handler.rs          # 输入处理（已有，需修改）
└── history.rs          # 历史管理（新增）
```

### 4.2 核心类型

```rust
pub struct HistoryEntry {
    pub id: usize,
    pub timestamp: u64,
    pub space: Option<String>,
    pub command: String,
}

pub struct HistoryManager {
    history_path: PathBuf,
    max_size: usize,
    dedup: HistoryDedupPolicy,
    save_policy: HistorySavePolicy,
    entries: Vec<HistoryEntry>,
    next_id: usize,
}
```

### 4.3 与 InputHandler 的集成

```rust
pub struct InputHandler {
    editor: Editor<GraphDBCompleter, DefaultHistory>,
    history_mgr: HistoryManager,
}

impl InputHandler {
    pub fn new(config: &HistoryConfig) -> Result<Self> {
        let mut history_mgr = HistoryManager::new(config);
        history_mgr.load()?;

        let mut editor = Editor::new()?;
        editor.set_helper(Some(GraphDBCompleter::new()));
        editor.set_auto_add_history(false); // 改为手动管理

        // 加载历史到 editor
        for entry in history_mgr.entries() {
            editor.add_history_entry(&entry.command);
        }

        Ok(Self { editor, history_mgr })
    }

    pub fn add_history(&mut self, command: &str, space: Option<&str>) {
        // 去重检查
        if self.history_mgr.should_skip(command) {
            return;
        }

        self.editor.add_history_entry(command);
        self.history_mgr.add_entry(command, space);

        if self.history_mgr.save_policy() == HistorySavePolicy::Incremental {
            let _ = self.history_mgr.append_last();
        }
    }
}
```

## 5. 配置项

```toml
[history]
file = "~/.graphdb/cli_history"     # 历史文件路径
max_size = 5000                      # 最大历史条数
dedup = "consecutive"                # 去重策略: none, consecutive, global
save_policy = "incremental"          # 保存策略: on_exit, incremental, timed
space_filter = false                 # 是否按 Space 过滤
```

## 6. 实现步骤

### Step 1: 新增 HistoryManager（2 天）

- 实现 `HistoryEntry` 和 `HistoryManager`
- 实现文件加载、追加写入、compact
- 实现去重逻辑
- 实现文件锁

### Step 2: 集成到 InputHandler（1 天）

- 修改 `InputHandler` 使用 `HistoryManager`
- 关闭 `auto_add_history`，改为手动调用
- 在 REPL 循环中调用 `add_history()`

### Step 3: 实现历史管理命令（1 天）

- 新增 `\history` 元命令
- 实现显示、搜索、清空、重执行功能
- 在 `MetaCommand` 枚举中添加变体

### Step 4: 按 Space 过滤（1 天）

- 历史条目关联 Space 信息
- 实现过滤逻辑
- 配置项支持

### Step 5: 测试（1 天）

- 历史持久化测试
- 去重测试
- 多实例并发测试
- 大量历史性能测试

## 7. 测试用例

### 7.1 基本功能

| 操作 | 预期结果 |
|------|----------|
| 执行命令后退出重启 | 历史保留，可上下箭头浏览 |
| 连续执行相同命令 | 历史中只保留一条 |
| 历史达到上限 | 自动淘汰最旧的条目 |

### 7.2 搜索功能

| 操作 | 预期结果 |
|------|----------|
| `Ctrl+R` 输入 `MATCH` | 显示包含 MATCH 的最近历史 |
| `\history search INSERT` | 列出所有包含 INSERT 的历史 |
| `\history 5` | 显示最近 5 条历史 |

### 7.3 管理功能

| 操作 | 预期结果 |
|------|----------|
| `\history clear` | 清空所有历史 |
| `\history exec 3` | 重新执行第 3 条历史命令 |
| `\history edit 3` | 将第 3 条历史加载到编辑缓冲区 |
