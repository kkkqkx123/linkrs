# GraphDB 配置参考手册

## 概述

本文档详细说明 GraphDB 的所有配置项，包括默认值、实际效果和配置建议。

---

## 配置文件结构

GraphDB 使用 TOML 格式的配置文件，默认配置文件为 `config.toml`。配置文件包含以下主要部分：

- `[database]` - 数据库基础配置
- `[transaction]` - 事务管理配置
- `[log]` - 日志配置
- `[auth]` - 认证授权配置
- `[bootstrap]` - 初始化配置
- `[optimizer]` - 查询优化器配置
- `[optimizer.rules]` - 优化器规则配置
- `[vector]` - 向量检索配置（启用 `qdrant` feature 时可用）
- `[monitoring]` - 监控配置

---

## 1. 数据库配置 [database]

### 1.1 host
- **类型**: String
- **默认值**: `"127.0.0.1"`
- **说明**: 数据库服务监听的主机地址
- **实际效果**: 控制 GraphDB 服务绑定的 IP 地址
- **配置建议**: 
  - 本地开发: `127.0.0.1`
  - 局域网访问: `0.0.0.0` 或具体内网 IP
  - 生产环境: 根据网络架构配置

### 1.2 port
- **类型**: u16
- **默认值**: `9758`
- **说明**: 数据库服务监听的端口号
- **实际效果**: 客户端通过此端口连接到 GraphDB 服务
- **配置建议**: 确保端口未被其他服务占用

### 1.3 storage_path
- **类型**: String
- **默认值**: `"data/graphdb"`
- **说明**: 数据存储路径
- **实际效果**: 
  - 相对路径: 相对于可执行文件所在目录
  - 绝对路径: 直接使用指定路径
  - 支持 `~` 展开为用户主目录
- **配置建议**: 
  - 确保目录有足够的磁盘空间
  - 生产环境建议使用独立的数据盘

### 1.4 max_connections
- **类型**: usize
- **默认值**: `10`
- **说明**: 最大客户端连接数
- **实际效果**: 限制同时连接到数据库的客户端数量
- **配置建议**: 
  - 根据服务器资源和并发需求调整
  - 建议值: 10-100

---

## 2. 事务配置 [transaction]

### 2.1 default_timeout
- **类型**: u64
- **默认值**: `30`（秒）
- **说明**: 默认事务超时时间
- **实际效果**: 
  - 事务超过此时间未提交将自动中止
  - 会话空闲超时时间为该值的 10 倍
- **配置建议**: 
  - 短事务场景: 10-30 秒
  - 复杂查询场景: 60-300 秒

### 2.2 max_concurrent_transactions
- **类型**: usize
- **默认值**: `1000`
- **说明**: 最大并发事务数
- **实际效果**: 限制同时执行的事务数量，超出限制将返回错误
- **配置建议**: 
  - 根据服务器内存和 CPU 资源调整
  - 建议值: 100-5000

### 2.3 enable_2pc
- **类型**: bool
- **默认值**: `false`
- **说明**: 是否启用两阶段提交（2PC）
- **实际效果**: 
  - `true`: 启用分布式事务支持
  - `false`: 使用本地事务
- **配置建议**: 
  - 单节点部署: `false`
  - 分布式部署: `true`

### 2.4 auto_cleanup
- **类型**: bool
- **默认值**: `true`
- **说明**: 是否自动清理过期事务
- **实际效果**: 
  - `true`: 后台线程定期清理超时事务
  - `false`: 需要手动清理
- **配置建议**: 生产环境建议启用

### 2.5 cleanup_interval
- **类型**: u64
- **默认值**: `10`（秒）
- **说明**: 清理任务执行间隔
- **实际效果**: 控制后台清理线程的检查频率
- **配置建议**: 
  - 高频事务场景: 5-10 秒
  - 低频事务场景: 30-60 秒

---

## 3. 日志配置 [log]

### 3.1 level
- **类型**: String
- **默认值**: `"info"`
- **可选值**: `"trace"`, `"debug"`, `"info"`, `"warn"`, `"error"`
- **说明**: 日志输出级别
- **实际效果**: 只输出该级别及以上的日志
- **配置建议**: 
  - 开发环境: `debug`
  - 生产环境: `info` 或 `warn`

### 3.2 dir
- **类型**: String
- **默认值**: `"logs"`
- **说明**: 日志文件存储目录
- **实际效果**: 日志文件将存储在此目录下
- **配置建议**: 确保目录有写入权限和足够空间

### 3.3 file
- **类型**: String
- **默认值**: `"graphdb"`
- **说明**: 日志文件基础名称
- **实际效果**: 生成的日志文件名为 `graphdb.YYYY-MM-DD.N.log`
- **配置建议**: 根据部署环境自定义

### 3.4 max_file_size
- **类型**: u64
- **默认值**: `104857600`（100MB）
- **说明**: 单个日志文件最大大小（字节）
- **实际效果**: 超过此大小将自动创建新日志文件
- **配置建议**: 
  - 磁盘充足: 100-500MB
  - 磁盘紧张: 10-50MB

### 3.5 max_files
- **类型**: usize
- **默认值**: `5`
- **说明**: 保留的日志文件最大数量
- **实际效果**: 超过此数量的旧日志文件将被删除
- **配置建议**: 
  - 高流量场景: 10-30
  - 低流量场景: 3-5

---

## 4. 认证授权配置 [auth]

### 4.1 enable_authorize
- **类型**: bool
- **默认值**: `true`
- **说明**: 是否启用授权检查
- **实际效果**: 
  - `true`: 所有操作需要权限验证
  - `false`: 跳过所有权限检查（不安全）
- **配置建议**: 
  - 生产环境: `true`
  - 本地开发: 可设为 `false` 方便测试

### 4.2 failed_login_attempts
- **类型**: u32
- **默认值**: `5`
- **说明**: 登录失败次数限制（0表示不限制）
- **实际效果**: 超过此次数将锁定账户
- **配置建议**: 
  - 安全要求高: 3-5
  - 宽松环境: 0（不限制）或 10

### 4.3 session_idle_timeout_secs
- **类型**: u64
- **默认值**: `3600`（1小时）
- **说明**: 会话空闲超时时间（秒）
- **实际效果**: 超过此时间未活动的会话将被关闭
- **配置建议**: 
  - 交互式使用: 1800-3600 秒
  - 批处理场景: 86400 秒（1天）或更长

### 4.4 default_username
- **类型**: String
- **默认值**: `"root"`
- **说明**: 默认管理员用户名
- **实际效果**: 首次启动时创建的默认用户
- **配置建议**: 生产环境建议修改

### 4.5 default_password
- **类型**: String
- **默认值**: `"root"`
- **说明**: 默认管理员密码
- **实际效果**: 首次启动时默认用户的密码
- **配置建议**: **生产环境必须修改**

### 4.6 force_change_default_password
- **类型**: bool
- **默认值**: `true`
- **说明**: 是否强制修改默认密码
- **实际效果**: 
  - `true`: 首次登录必须修改密码
  - `false`: 允许使用默认密码
- **配置建议**: 生产环境建议启用

---

## 5. 初始化配置 [bootstrap]

### 5.1 auto_create_default_space
- **类型**: bool
- **默认值**: `true`
- **说明**: 是否自动创建默认图空间
- **实际效果**: 
  - `true`: 启动时自动创建默认 Space
  - `false`: 需要手动创建
- **配置建议**: 首次部署建议启用

### 5.2 default_space_name
- **类型**: String
- **默认值**: `"default"`
- **说明**: 默认图空间名称
- **实际效果**: 自动创建的 Space 的名称
- **配置建议**: 根据业务需求命名

### 5.3 single_user_mode
- **类型**: bool
- **默认值**: `false`
- **说明**: 单用户模式
- **实际效果**: 
  - `true`: 跳过认证，始终使用默认用户
  - `false`: 正常认证流程
- **配置建议**: 
  - 个人使用: `true`
  - 多用户环境: `false`

---

## 6. 优化器配置 [optimizer]

### 6.1 max_iteration_rounds
- **类型**: usize
- **默认值**: `5`
- **说明**: 查询优化最大迭代轮数
- **实际效果**: 控制查询计划的优化深度
- **配置建议**: 
  - 复杂查询: 5-10
  - 简单查询: 3-5

### 6.2 max_exploration_rounds
- **类型**: usize
- **默认值**: `128`
- **说明**: 查询计划最大探索轮数
- **实际效果**: 限制优化器探索的候选计划数量
- **配置建议**: 
  - 高性能要求: 256-512
  - 快速响应: 64-128

### 6.3 enable_cost_model
- **类型**: bool
- **默认值**: `true`
- **说明**: 是否启用代价模型
- **实际效果**: 
  - `true`: 基于代价选择最优计划
  - `false`: 使用启发式规则
- **配置建议**: 建议启用以获得更好的查询性能

### 6.4 enable_multi_plan
- **类型**: bool
- **默认值**: `true`
- **说明**: 是否启用多计划候选
- **实际效果**: 
  - `true`: 生成多个候选计划进行比较
  - `false`: 只生成一个计划
- **配置建议**: 建议启用

### 6.5 enable_property_pruning
- **类型**: bool
- **默认值**: `true`
- **说明**: 是否启用属性剪枝
- **实际效果**: 自动移除查询中不需要的属性访问
- **配置建议**: 建议启用以减少 IO

### 6.6 enable_adaptive_iteration
- **类型**: bool
- **默认值**: `true`
- **说明**: 是否启用自适应迭代
- **实际效果**: 根据查询复杂度动态调整迭代次数
- **配置建议**: 建议启用

### 6.7 stable_threshold
- **类型**: usize
- **默认值**: `2`
- **说明**: 稳定阈值
- **实际效果**: 连续多轮优化没有改进时停止优化
- **配置建议**: 默认值即可

### 6.8 min_iteration_rounds
- **类型**: usize
- **默认值**: `1`
- **说明**: 最小迭代轮数
- **实际效果**: 至少执行这么多轮优化
- **配置建议**: 默认值即可

---

## 7. 优化器规则配置 [optimizer.rules]

### 7.1 disabled_rules
- **类型**: Vec<String>
- **默认值**: `[]`
- **说明**: 禁用的优化规则列表
- **实际效果**: 列出的规则将不会被执行
- **配置建议**: 仅在规则导致问题时禁用

### 7.2 enabled_rules
- **类型**: Vec<String>
- **默认值**: `[]`
- **说明**: 显式启用的优化规则列表
- **实际效果**: 优先级高于默认规则集
- **配置建议**: 高级调优时使用

**可用规则列表**:
- `FilterPushDownRule` - 谓词下推
- `PredicatePushDownRule` - 谓词下推
- `RemoveUselessNodeRule` - 移除无用节点
- `MergeFiltersRule` - 合并过滤条件
- `LimitPushDownRule` - LIMIT 下推

---

## 8. 向量检索配置 [vector]

> 仅在启用 `qdrant` feature 时可用。当前引擎类型为 `Qdrant`，配置会传递给 `vector-client::VectorClientConfig`。

### 8.1 enabled
- **类型**: bool
- **默认值**: `false`
- **说明**: 是否启用向量检索
- **实际效果**: 
  - `true`: GraphDB 启动时创建向量客户端并加载向量索引协调器
  - `false`: 跳过向量相关初始化
- **配置建议**: 只有需要向量索引时才启用

### 8.2 engine
- **类型**: String
- **默认值**: `"Qdrant"`
- **说明**: 向量引擎类型
- **实际效果**: 当前仅支持 `Qdrant`
- **配置建议**: 保持默认值

### 8.3 connection.host
- **类型**: String
- **默认值**: `"localhost"`
- **说明**: Qdrant 服务主机名或 IP
- **实际效果**: 用于构建 gRPC / HTTP 连接地址
- **配置建议**: 本地开发使用 `localhost`，生产环境使用实际服务地址

### 8.4 connection.port
- **类型**: u16
- **默认值**: `6333`
- **说明**: Qdrant gRPC 端口
- **实际效果**: GraphDB 通过该端口连接 Qdrant 的 gRPC 接口
- **配置建议**: 确保端口与 Qdrant 实际配置一致

### 8.5 connection.http_port
- **类型**: Option\<u16\>
- **默认值**: `null`
- **说明**: Qdrant HTTP 端口（可选）
- **实际效果**: 某些调试或兼容场景下用于构造 HTTP 地址
- **配置建议**: 如果部署同时开放 HTTP 与 gRPC，建议显式指定

### 8.6 connection.use_tls
- **类型**: bool
- **默认值**: `false`
- **说明**: 是否使用 TLS
- **实际效果**: `true` 时请求使用 HTTPS / TLS 连接
- **配置建议**: 仅在远程部署且网关或服务端启用 TLS 时打开

### 8.7 connection.api_key
- **类型**: Option\<String\>
- **默认值**: `null`
- **说明**: Qdrant API Key
- **实际效果**: 若服务端启用认证，则请求会附带 API Key
- **配置建议**: 生产环境按需配置

### 8.8 connection.connect_timeout_secs
- **类型**: u64
- **默认值**: `5`
- **说明**: 连接超时时间（秒）
- **实际效果**: 超过该时间仍未建立连接则返回错误
- **配置建议**: 本地部署可保持默认，跨机房部署可适当增大

### 8.9 timeout.request_timeout_secs
- **类型**: u64
- **默认值**: `30`
- **说明**: 通用请求超时时间（秒）
- **实际效果**: 影响客户端大多数请求
- **配置建议**: 一般场景保持默认即可

### 8.10 timeout.search_timeout_secs
- **类型**: u64
- **默认值**: `60`
- **说明**: 搜索请求超时时间（秒）
- **实际效果**: 影响搜索类请求的等待上限
- **配置建议**: 大集合或慢磁盘场景可适当提高

### 8.11 timeout.upsert_timeout_secs
- **类型**: u64
- **默认值**: `30`
- **说明**: 写入请求超时时间（秒）
- **实际效果**: 影响 upsert / 批量写入请求的等待上限
- **配置建议**: 批量导入场景可适当提高

### 8.12 retry.max_retries
- **类型**: usize
- **默认值**: `3`
- **说明**: 最大重试次数
- **实际效果**: 请求失败时自动重试的上限
- **配置建议**: 网络不稳定场景可适当提高，但不宜过大

### 8.13 retry.initial_delay_ms
- **类型**: u64
- **默认值**: `100`
- **说明**: 首次重试等待时间（毫秒）
- **实际效果**: 控制重试退避起点
- **配置建议**: 保持默认值通常足够

### 8.14 retry.max_delay_ms
- **类型**: u64
- **默认值**: `5000`
- **说明**: 重试最大等待时间（毫秒）
- **实际效果**: 退避不会超过该值
- **配置建议**: 保持默认值即可

### 8.15 retry.multiplier
- **类型**: f64
- **默认值**: `2.0`
- **说明**: 重试退避倍率
- **实际效果**: 每次重试等待时间按该倍率增长
- **配置建议**: 一般保持默认值

### 8.16 embedding（嵌入模型配置）

> 可选配置。若未配置，向量检索使用原始向量；若配置，GraphDB 会自动调用嵌入服务将文本转为向量后写入 Qdrant。

#### 8.16.1 embedding.base_url
- **类型**: String
- **默认值**: `"http://localhost:11434/api/embeddings"`
- **说明**: 嵌入 API 端点 URL
- **实际效果**: GraphDB 通过该 URL 调用嵌入服务生成向量
- **配置建议**: 
  - Ollama 本地: `http://localhost:11434/api/embeddings`
  - OpenAI: `https://api.openai.com/v1/embeddings`
  - 其他兼容端点: 按服务商提供地址配置

#### 8.16.2 embedding.api_key
- **类型**: Option\<String\>
- **默认值**: `null`
- **说明**: 嵌入 API 密钥（可选，部分服务需要）
- **实际效果**: 若有值，每次请求会附带 Authorization header
- **配置建议**: OpenAI、Azure 等服务需要配置，Ollama 本地可省略

#### 8.16.3 embedding.model
- **类型**: String
- **默认值**: `"all-minilm"`
- **说明**: 嵌入模型名称
- **实际效果**: 传递给嵌入 API 的 model 参数
- **配置建议**:
  - Ollama: `all-minilm`、`nomic-embed-text`、`llama3` 等
  - OpenAI: `text-embedding-3-small`、`text-embedding-3-large`、`text-embedding-ada-002`

#### 8.16.4 embedding.timeout_secs
- **类型**: u64
- **默认值**: `30`
- **说明**: 嵌入请求超时时间（秒）
- **实际效果**: 超过该时间未返回响应则请求失败
- **配置建议**: 大模型或网络延迟高时可适当提高到 60-120

#### 8.16.5 embedding.dimension
- **类型**: Option\<usize\>
- **默认值**: `null`
- **说明**: 期望的向量维度（可选，不设置则自动检测）
- **实际效果**: 
  - `Some(768)`: 强制返回的向量维度必须匹配，不匹配则报错
  - `null`: 自动检测并适配
- **配置建议**: 推荐显式设置以提高健壮性（例如 OpenAI `text-embedding-3-small` 为 1536）

#### 8.16.6 embedding.preprocessor（文本预处理器）

> 可选配置。用于在生成嵌入前对文本进行预处理（如添加前缀、模板等）。不配置则不做处理。

| 类型 | 说明 | 额外字段 |
|------|------|---------|
| `none` | 无预处理（默认） | 无 |
| `prefix` | 为文本添加固定前缀 | `prefix` (String) |
| `template` | 使用模板替换 `{{text}}` 占位符 | `template` (String) |
| `nomic` | Nomic-Embed 任务类型前缀 | `task_type` (String) |
| `stella` | Stella 任务类型前缀 | `task_type` (String) |

**Nomic task_type 可选值**:
- `search_query` — 搜索查询
- `search_document` — 搜索文档
- `classification` — 分类
- `clustering` — 聚类

**Stella task_type 可选值**:
- `s2p_query` — 句子到段落查询
- `s2s_document` — 句子到句子文档
- `p2p_query` — 段落到段落查询
- `p2p_document` — 段落到段落文档

**预处理效果示例**:

| 配置 | 输入 | 输出 |
|------|------|------|
| `type = "none"` | `"hello"` | `"hello"` |
| `type = "prefix", prefix = "query: "` | `"rust"` | `"query: rust"` |
| `type = "template", template = "classify: {{text}}"` | `"hello"` | `"classify: hello"` |
| `type = "nomic", task_type = "search_query"` | `"rust"` | `"search_query: rust"` |
| `type = "stella", task_type = "s2p_query"` | `"query"` | `"Instruct: Given a web search query, retrieve relevant passages. Query: query"` |

---

## 9. 监控配置 [monitoring]

### 9.1 enabled
- **类型**: bool
- **默认值**: `true`
- **说明**: 是否启用监控
- **实际效果**: 
  - `true`: 收集查询统计信息
  - `false`: 关闭监控功能
- **配置建议**: 生产环境建议启用

### 9.2 memory_cache_size
- **类型**: usize
- **默认值**: `1000`
- **说明**: 内存缓存大小（保留最近N条查询）
- **实际效果**: 控制内存中保留的查询历史数量
- **配置建议**: 
  - 内存充足: 5000-10000
  - 内存紧张: 100-500

### 9.3 slow_query_threshold_ms
- **类型**: u64
- **默认值**: `1000`（1秒）
- **说明**: 慢查询阈值（毫秒）
- **实际效果**: 超过此时间的查询将被记录为慢查询
- **配置建议**: 
  - 高性能要求: 100-500ms
  - 一般场景: 1000-3000ms

### 9.4 slow_query_log_dir
- **类型**: String
- **默认值**: `"logs/slow_queries"`
- **说明**: 慢查询日志目录
- **实际效果**: 慢查询日志文件存储位置
- **配置建议**: 确保目录有写入权限

### 9.5 slow_query_log_retention_days
- **类型**: u32
- **默认值**: `7`
- **说明**: 慢查询日志保留天数
- **实际效果**: 超过此天数的慢查询日志将被删除
- **配置建议**: 
  - 长期分析: 30-90 天
  - 短期分析: 3-7 天

---

## 配置示例

### 开发环境配置

```toml
[database]
host = "127.0.0.1"
port = 9758
storage_path = "data/graphdb"
max_connections = 10

[transaction]
default_timeout = 60
max_concurrent_transactions = 100
enable_2pc = false
auto_cleanup = true
cleanup_interval = 10

[log]
level = "debug"
dir = "logs"
file = "graphdb"
max_file_size = 52428800  # 50MB
max_files = 3

[auth]
enable_authorize = false
failed_login_attempts = 0
session_idle_timeout_secs = 7200
default_username = "root"
default_password = "root"
force_change_default_password = false

[bootstrap]
auto_create_default_space = true
default_space_name = "default"
single_user_mode = true

[vector]
enabled = true
engine = "Qdrant"

[vector.connection]
host = "localhost"
port = 6333
http_port = 6334
use_tls = false

[vector.timeout]
request_timeout_secs = 30
search_timeout_secs = 60
upsert_timeout_secs = 30

[vector.retry]
max_retries = 3
initial_delay_ms = 100
max_delay_ms = 5000
multiplier = 2.0

# 嵌入服务配置（可选）
# 使用 Ollama 本地嵌入模型
[vector.embedding]
base_url = "http://localhost:11434/api/embeddings"
model = "all-minilm"
timeout_secs = 30

# 文本预处理器（可选，不配置则无预处理）
# [vector.embedding.preprocessor]
# type = "prefix"
# prefix = "query: "

[optimizer]
max_iteration_rounds = 3
max_exploration_rounds = 64
enable_cost_model = true
enable_multi_plan = true
enable_property_pruning = true
enable_adaptive_iteration = true
stable_threshold = 2
min_iteration_rounds = 1

[monitoring]
enabled = true
memory_cache_size = 500
slow_query_threshold_ms = 500
slow_query_log_dir = "logs/slow_queries"
slow_query_log_retention_days = 3
```

### 生产环境配置

```toml
[database]
host = "0.0.0.0"
port = 9758
storage_path = "/var/lib/graphdb/data"
max_connections = 100

[transaction]
default_timeout = 30
max_concurrent_transactions = 2000
enable_2pc = false
auto_cleanup = true
cleanup_interval = 10

[log]
level = "info"
dir = "/var/log/graphdb"
file = "graphdb"
max_file_size = 524288000  # 500MB
max_files = 20

[auth]
enable_authorize = true
failed_login_attempts = 5
session_idle_timeout_secs = 3600
default_username = "admin"
default_password = "changeme"
force_change_default_password = true

[bootstrap]
auto_create_default_space = true
default_space_name = "production"
single_user_mode = false

[vector]
enabled = true
engine = "Qdrant"

[vector.connection]
host = "qdrant.example.com"
port = 6333
http_port = 6334
use_tls = true

[vector.timeout]
request_timeout_secs = 30
search_timeout_secs = 60
upsert_timeout_secs = 30

[vector.retry]
max_retries = 3
initial_delay_ms = 100
max_delay_ms = 5000
multiplier = 2.0

# 嵌入服务配置（可选）
# 使用 OpenAI 嵌入模型
[vector.embedding]
base_url = "https://api.openai.com/v1/embeddings"
api_key = "sk-xxx"
model = "text-embedding-3-small"
dimension = 1536
timeout_secs = 60

# 文本预处理器（可选）
# [vector.embedding.preprocessor]
# type = "none"

[optimizer]
max_iteration_rounds = 10
max_exploration_rounds = 256
enable_cost_model = true
enable_multi_plan = true
enable_property_pruning = true
enable_adaptive_iteration = true
stable_threshold = 2
min_iteration_rounds = 1

[monitoring]
enabled = true
memory_cache_size = 5000
slow_query_threshold_ms = 1000
slow_query_log_dir = "/var/log/graphdb/slow_queries"
slow_query_log_retention_days = 30
```

---

## 配置加载优先级

1. 配置文件中的显式配置
2. 配置文件中省略的字段使用默认值
3. 运行时可通过环境变量覆盖（未来支持）

---

## 配置验证

启动时会自动验证配置：
- 检查必需的配置项
- 验证配置值的合法性
- 检查路径的读写权限
- 验证端口是否可用

配置错误将导致启动失败并输出错误信息。
