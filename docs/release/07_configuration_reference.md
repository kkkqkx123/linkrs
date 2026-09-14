# GraphDB 配置参考手册

## 概述

本文档详细说明 GraphDB 的所有配置项，包括默认值、实际效果和配置建议。配置项与 `graphdb-config` crate 中的定义一一对应。

---

## 配置文件结构

GraphDB 使用 TOML 格式的配置文件，默认配置文件为 `config.toml`。配置文件包含以下主要部分：

- `[database]` - 数据库基础配置
- `[transaction]` - 事务管理配置
- `[log]` - 日志配置
- `[auth]` - 认证授权配置
- `[bootstrap]` - 初始化配置
- `[grpc]` - gRPC 服务配置
- `[optimizer]` / `[optimizer.rules]` - 查询优化器配置
- `[parallel]` - 查询内并行执行配置
- `[storage]` - 存储引擎配置
- `[query_resource]` - 查询资源限制配置
- `[columnar]` - 列式快速路径配置
- `[monitoring]` - 监控与慢查询日志配置
- `[vector]` - 向量检索配置（含 local / qdrant 两种引擎）
- `[fulltext]` - 全文检索配置
- `[embedded.runtime]` - 嵌入式运行时配置

> **注意：** 旧版本中的 `[vector.connection]`、`[vector.timeout]`、`[vector.retry]`、`[vector.embedding]` 已迁移至 `[vector.qdrant.*]` 子节；事务配置中的 `enable_2pc`、`auto_cleanup`、`cleanup_interval` 已移除。

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
- **说明**: HTTP 服务监听的端口号（HTTP 服务默认启用并绑定 `0.0.0.0`，可用 `[http]` 节覆盖）
- **配置建议**: 确保端口未被其他服务占用

### 1.3 storage_path
- **类型**: String
- **默认值**: `"data/graphdb"`
- **说明**: 数据存储路径
- **实际效果**: 
  - 相对路径: 相对于配置文件所在目录解析
  - 绝对路径: 直接使用指定路径
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
- **实际效果**: 事务超过此时间未提交将自动中止
- **配置建议**: 
  - 短事务场景: 10-30 秒
  - 复杂查询场景: 60-300 秒

### 2.2 max_concurrent_transactions
- **类型**: usize
- **默认值**: `1000`
- **说明**: 最大并发事务数
- **实际效果**: 限制同时执行的事务数量，超出限制将返回错误（不允许为 0）
- **配置建议**: 
  - 根据服务器内存和 CPU 资源调整
  - 建议值: 100-5000

### 2.3 auto_commit
- **类型**: bool
- **默认值**: `false`
- **说明**: 每条成功语句是否自动提交
- **实际效果**: 
  - `true`: 单语句自动提交，无需显式 BEGIN/COMMIT
  - `false`: 需要显式事务控制
- **配置建议**: 交互式/嵌入式场景可开启，批量导入场景保持 `false`

> **注意：** 当前版本没有 `enable_2pc`、`auto_cleanup`、`cleanup_interval` 配置项。

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

### 4.7 bcrypt_cost
- **类型**: u32
- **默认值**: `12`
- **说明**: bcrypt 密码哈希成本因子（范围 4-12），进程启动时生效，需重启后应用
- **实际效果**: 
  - 成本越高，密码哈希越安全，但每次创建用户/修改密码耗时越长（成本 12 约 200-300ms）
  - 成本 4-6 约 1ms 级别，适合本地开发与低配置机器
- **配置建议**: 
  - 生产环境: `12`（默认值）
  - 本地开发/低配置机器: `4` 或 `6` 可显著降低延迟

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

## 6. gRPC 服务配置 [grpc]

### 6.1 enabled
- **类型**: bool
- **默认值**: `true`
- **说明**: 是否启用 gRPC 服务

### 6.2 port
- **类型**: u16
- **默认值**: `9669`
- **说明**: gRPC 服务端口（不允许为 0）

### 6.3 max_connections
- **类型**: usize
- **默认值**: `100`
- **说明**: gRPC 最大并发连接数

### 6.4 max_request_size / max_response_size
- **类型**: usize
- **默认值**: `10485760`（10MB）
- **说明**: 最大请求/响应消息大小（字节）

### 6.5 keepalive_interval_secs / keepalive_timeout_secs
- **类型**: u64
- **默认值**: `30` / `10`
- **说明**: Keepalive 间隔与超时（秒，0 表示禁用）

### 6.6 connection_timeout_secs
- **类型**: u64
- **默认值**: `10`
- **说明**: 连接超时时间（秒）

### 6.7 request_timeout_secs
- **类型**: u64
- **默认值**: `60`
- **说明**: 请求超时时间（秒，0 表示禁用）

---

## 7. 优化器配置 [optimizer]

### 7.1 参数列表

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| max_iteration_rounds | usize | 5 | 查询优化最大迭代轮数（>0） |
| max_exploration_rounds | usize | 128 | 查询计划最大探索轮数（>0） |
| enable_cost_model | bool | true | 是否启用代价模型 |
| enable_multi_plan | bool | true | 是否启用多计划候选 |
| enable_property_pruning | bool | true | 是否启用属性剪枝 |
| enable_adaptive_iteration | bool | true | 是否启用自适应迭代 |
| stable_threshold | usize | 2 | 连续多轮无改进时停止优化 |
| min_iteration_rounds | usize | 1 | 最小迭代轮数（不得大于 max_iteration_rounds） |

### 7.2 优化器规则配置 [optimizer.rules]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| disabled_rules | Vec\<String\> | `[]` | 禁用的优化规则列表 |
| enabled_rules | Vec\<String\> | `[]` | 显式启用的规则列表（优先级高于默认规则集） |

**可用规则列表**:
- `FilterPushDownRule` - 谓词下推
- `PredicatePushDownRule` - 谓词下推
- `RemoveUselessNodeRule` - 移除无用节点
- `MergeFiltersRule` - 合并过滤条件
- `LimitPushDownRule` - LIMIT 下推

---

## 8. 并行执行配置 [parallel]

查询内并行分区开关，默认关闭（行为与关闭时完全一致）。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| enabled | bool | false | 是否启用查询内并行分区 |
| workers | usize | 1 | 共享调度器的工作线程数（>0） |
| min_rows_per_partition | u64 | 100000 | 每个分区的最小行数（低于阈值拒绝分区） |
| max_partitions | usize | 1 | 单个扫描的最大分区数 |
| max_buffered_chunks | usize | 10 | 每个分区 worker 的最大缓冲块数（背压） |
| vertex_id_start | Option\<i64\> | null | 可选顶点 ID 范围下界（与 end 同时设置才生效） |
| vertex_id_end | Option\<i64\> | null | 可选顶点 ID 范围上界（开区间） |

---

## 9. 存储引擎配置 [storage]

### 9.1 基础参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| engine | String | propertygraph | 存储引擎类型 |
| compression | String | none | 压缩算法（none/lz4/zstd/snappy） |
| compression_level | u32 | 3 | 压缩级别（0-9） |
| checkpoint_interval_secs | u64 | 300 | Checkpoint 间隔（秒，0 禁用） |
| max_db_size | u64 | 0 | 最大数据库大小（字节，0 不限制） |
| auto_statistics | bool | true | 是否自动收集统计信息 |
| statistics_interval_secs | u64 | 60 | 统计信息收集间隔（秒） |

### 9.2 内存预算

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| max_memory_bytes | u64 | 536870912（512MB） | 存储总内存预算（字节） |
| index_memory_bytes | u64 | 134217728（128MB） | 原生索引独立内存预算（不得超过总预算） |
| memory_soft_ratio | f64 | 0.80 | 软内存压力水位（0 < soft < hard <= 1） |
| memory_hard_ratio | f64 | 0.95 | 硬内存准入水位 |

### 9.3 快照与墓碑

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| max_active_snapshots | usize | 1000 | 最大活跃快照数 |
| max_snapshot_age_secs | u64 | 300 | 快照最大存活时间（秒） |
| max_tombstones | usize | 1000000 | 保留墓碑数量上限 |
| max_tombstone_bytes | u64 | 268435456（256MB） | 墓碑估算内存上限（字节） |
| index_gc_batch | usize | 10000 | 每次 GC 处理的索引条目数 |
| operation_timeout_secs | u64 | 30 | 维护操作最大时长（秒） |
| dirty_flush_operations | u64 | 50000 | 触发 flush 的脏操作数阈值 |
| dirty_flush_bytes | u64 | 67108864（64MB） | 触发 flush 的脏数据量阈值（字节） |

### 9.4 缓存与编码

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| cache_ttl_secs | u64 | 60 | 记录缓存 TTL（秒，0 禁用） |
| cache_tti_secs | u64 | 300 | 记录缓存 TTI（秒，0 禁用） |
| string_min_rows | usize | 50 | 字符串编码分析所需最小行数 |
| avg_length_threshold | usize | 16 | 考虑 FSST 编码的最小平均字符串长度 |
| cardinality_ratio_threshold | f64 | 0.5 | 低于该基数比（distinct/total）优先字典编码 |
| fsst_rebuild_threshold | f64 | 0.2 | 触发 FSST 重建的新旧数据比 |
| index_pool_capacity_bytes | u64 | 134217728（128MB） | 每 shard 索引缓冲池容量（字节） |
| index_eviction_enabled | bool | true | 内存压力下是否启用 chunk 级驱逐 |
| index_eviction_high_ratio | f64 | 0.85 | 驱逐触发高水位（usage/capacity） |
| index_eviction_low_ratio | f64 | 0.65 | 驱逐目标低水位（须小于高水位） |

---

## 10. 查询资源限制 [query_resource]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| max_memory_per_query | u64 | 0 | 单查询内存上限（字节，0 不限制） |
| max_concurrent_queries | usize | 100 | 最大并发查询数（0 不允许，必须 >0） |
| query_timeout_secs | u64 | 0 | 查询超时（秒，0 不限制） |
| max_result_size | u64 | 0 | 最大结果集大小（字节，0 不限制） |
| max_vertex_scan | usize | 1000000 | 单查询最大顶点扫描数 |
| max_edge_scan | usize | 10000000 | 单查询最大边扫描数 |

---

## 11. 列式快速路径配置 [columnar]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| column_block_enabled | bool | false | 是否启用存储列块扫描路径（输出与行式路径逐位一致；开启后 PROFILE/EXPLAIN ANALYZE 中可观察 `column_block_hits`） |

---

## 12. 监控配置 [monitoring]

### 12.1 顶层参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| enabled | bool | true | 是否启用监控 |
| memory_cache_size | usize | 1000 | 内存缓存大小（保留最近N条查询，>0） |
| slow_query_threshold_ms | u64 | 1000 | 慢查询阈值（毫秒） |
| progress_report_rows_interval | u64 | 0 | 查询进度通知的行间隔（0 禁用进度上报） |

### 12.2 慢查询日志 [monitoring.slow_query_log]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| enabled | bool | true | 是否启用慢查询日志 |
| threshold_ms | u64 | 1000 | 慢查询阈值（毫秒，>0） |
| log_file_path | String | `"logs/slow_query.log"` | 慢查询日志文件路径 |
| max_file_size_mb | u64 | 100 | 单文件轮转大小（MB） |
| max_files | u32 | 5 | 保留的日志文件数 |
| verbose_format | bool | false | 是否使用详细格式 |
| buffer_size | usize | 100 | 异步写缓冲大小（>0） |
| json_format | bool | false | 是否使用 JSON 格式 |

> **注意：** 旧版文档中的 `slow_query_log_dir`、`slow_query_log_retention_days` 已被 `[monitoring.slow_query_log]` 子节取代。

---

## 13. 向量检索配置 [vector]

向量检索支持两种引擎：`local`（内置，默认）与 `qdrant`（外部服务，需启用 `vector-qdrant` feature）。

### 13.1 顶层参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| enabled | bool | true | 是否启用向量检索 |
| engine | String | `"local"` | 引擎类型：`local` 或 `qdrant` |
| mvcc.ssi_read_set | bool | false | 向量搜索的 MVCC SSI 读集（默认关闭） |
| collection.granularity | String | `"space"` | 向量集合粒度：`space` 或 `field` |
| retention.* | - | 见下 | Outbox 保留策略 |

### 13.2 本地引擎 [vector.local]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| data_dir | Option\<PathBuf\> | null | 数据目录（缺省为 `<storage_path>/vector`） |
| hnsw.m | usize | 16 | HNSW 每层最大连接数 |
| hnsw.ef_construct | usize | 100 | HNSW 构建动态列表大小 |
| hnsw.full_scan_threshold | usize | 0 | 全量扫描阈值（0 禁用） |
| hnsw.ef_search | usize | 0 | 搜索动态列表大小（0 用引擎默认） |
| hnsw.iterative_max_rounds | usize | 0 | 迭代搜索最大轮数 |
| hnsw.max_scan_tuples | u64 | 0 | 最大扫描元组数 |
| ivf.lists | u32 | 0 | IVF 聚类中心数（0 自动） |
| ivf.auto_promotion | bool | false | 是否自动从全量扫描晋升为 IVF |
| ivf.min_build_points | u64 | 100000 | 构建 IVF 所需最小点数 |
| ivf.sample_limit | usize | 65536 | 训练采样上限 |
| ivf.kmeans_max_iter | u32 | 10 | K-means 最大迭代次数 |
| ivf.drift_threshold | f64 | 0.10 | 质心漂移阈值 |
| ivf.drift_check_interval | u64 | 25000 | 漂移检查间隔（点数） |
| ivf.default_nprobe | usize | 8 | 默认探测中心数 |
| ivf.max_probes | usize | 0 | 最大探测中心数（0 不限制） |
| quantization.quantization_type | Option\<String\> | null | 量化类型（scalar/binary/product） |
| quantization.quantile | Option\<f32\> | null | 量化分位数 |
| quantization.compression | Option\<String\> | null | 压缩比（x4/x8/x16/x32/x64） |
| quantization.always_ram | Option\<bool\> | null | 量化数据是否常驻内存 |

### 13.3 Qdrant 引擎 [vector.qdrant]

> 仅当 `engine = "qdrant"` 时生效。旧版 `[vector.connection]` / `[vector.timeout]` / `[vector.embedding]` 均已迁移至此；`retry` 重试配置已移除。

#### [vector.qdrant] 顶层
| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| enabled | bool | true | 是否启用 qdrant 客户端 |

#### [vector.qdrant.connection]
| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| host | String | `"localhost"` | Qdrant 服务地址 |
| port | u16 | 6334 | gRPC 端口（默认传输） |
| http_port | Option\<u16\> | null | HTTP 端口（HTTP 传输或健康检查用） |
| transport | String | `"grpc"` | 传输方式：`grpc` 或 `http` |
| use_tls | bool | false | 是否使用 TLS |
| api_key | Option\<String\> | null | API Key |
| connect_timeout_secs | u64 | 5 | 连接超时（秒） |

#### [vector.qdrant.timeout]
| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| request_timeout_secs | u64 | 30 | 通用请求超时（秒） |
| search_timeout_secs | u64 | 60 | 搜索超时（秒） |
| upsert_timeout_secs | u64 | 30 | 写入超时（秒） |

#### [vector.qdrant.embedding]（可选）

若未配置，向量检索使用原始向量；若配置，GraphDB 会自动调用嵌入服务将文本转为向量。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| base_url | String | `"http://localhost:11434/api/embeddings"` | 嵌入 API 端点 |
| api_key | Option\<String\> | null | 嵌入 API 密钥（OpenAI 等需要） |
| model | String | `"all-minilm"` | 嵌入模型名称 |
| timeout_secs | u64 | 30 | 嵌入请求超时（秒） |
| dimension | Option\<usize\> | null | 期望向量维度（不设则自动检测） |

文本预处理器 `[vector.qdrant.embedding.preprocessor]`（可选）：

| 类型 | 说明 | 额外字段 |
|------|------|---------|
| `none` | 无预处理（默认） | 无 |
| `prefix` | 为文本添加固定前缀 | `prefix` (String) |
| `template` | 使用模板替换 `{{text}}` 占位符 | `template` (String) |
| `nomic` | Nomic-Embed 任务类型前缀 | `task_type` (String) |
| `stella` | Stella 任务类型前缀 | `task_type` (String) |

**Nomic task_type**: `search_query` / `search_document` / `classification` / `clustering`
**Stella task_type**: `s2p_query` / `s2s_document` / `p2p_query` / `p2p_document`

### 13.4 Outbox 保留策略 [vector.retention]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| enabled | bool | true | 是否启用保留清理 |
| prune_interval_secs | u64 | 3600 | 清理执行间隔（秒） |
| grace_lsn_distance | u64 | 10000 | LSN 滞后宽限距离 |
| max_applied_age_ms | u64 | 86400000 | 已应用记录最大保留时间（毫秒） |
| max_archive_rows | u64 | 100000 | 归档行数上限 |

---

## 14. 全文检索配置 [fulltext]

全文索引基于 BM25（tantivy 引擎）。

### 14.1 顶层参数

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| enabled | bool | false | 是否启用全文检索 |
| default_engine | String | `"bm25"` | 默认引擎（当前仅 bm25） |
| index_path | PathBuf | `"data/fulltext"` | 全文索引存储目录 |
| cache_size | usize | 100 | 索引缓存大小 |
| max_result_cache | usize | 1000 | 结果缓存条数上限 |
| result_cache_ttl_secs | u64 | 60 | 结果缓存 TTL（秒） |

### 14.2 同步配置 [fulltext.sync]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| queue_size | usize | 10000 | 同步队列大小 |
| commit_interval_ms | u64 | 1000 | 提交间隔（毫秒） |
| batch_size | usize | 100 | 批量大小 |
| failure_policy | String | `"fail_open"` | 同步失败策略：`fail_open` / `fail_closed` |

### 14.3 Tantivy 引擎配置 [fulltext.tantivy]

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| writer_memory_budget | usize | 50000000 | 写入器内存预算（字节） |
| tokenizer | String | `"default"` | 分词器：`default` / `jieba` / `raw` / `whitespace` |
| doc_store_cache_num_blocks | usize | 100 | 文档存储缓存块数 |
| bm25_params.k1 | f32 | 1.2 | BM25 词频调节参数 |
| bm25_params.b | f32 | 0.75 | BM25 文档长度调节参数 |

---

## 15. 嵌入式运行时配置 [embedded.runtime]

用于嵌入式场景（`#[cfg(feature = "embedded")]`）的数据库实例运行参数。

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| path | Option\<PathBuf\> | null | 数据库路径；`null` 表示纯内存模式 |
| cache_size_mb | usize | 64 | 页缓存大小（MB） |
| default_timeout_secs | u64 | 30 | 默认操作超时（秒） |
| enable_wal | bool | true | 是否启用 WAL（写前日志） |
| sync_mode | String | `"normal"` | 同步模式 |
| read_only | bool | false | 是否以只读方式打开 |
| create_if_missing | bool | true | 数据库不存在时是否自动创建 |
| max_open_files | usize | 100 | 最大打开文件数 |
| worker_threads | usize | 0 | 后台工作线程数（0 = 自动检测） |

### 示例

```toml
[embedded.runtime]
path = "data/embedded"      # 省略则为纯内存模式
cache_size_mb = 128
enable_wal = true
sync_mode = "normal"
read_only = false
create_if_missing = true
max_open_files = 200
worker_threads = 4
```

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
auto_commit = false

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
bcrypt_cost = 6  # 本地开发可调低，生产建议 12

[bootstrap]
auto_create_default_space = true
default_space_name = "default"
single_user_mode = true

[grpc]
enabled = true
port = 9669

[parallel]
enabled = false

[vector]
enabled = true
engine = "local"

[fulltext]
enabled = false

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
```

### 生产环境配置（Qdrant 向量后端）

```toml
[database]
host = "0.0.0.0"
port = 9758
storage_path = "/var/lib/graphdb/data"
max_connections = 100

[transaction]
default_timeout = 30
max_concurrent_transactions = 2000
auto_commit = false

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
bcrypt_cost = 12

[bootstrap]
auto_create_default_space = true
default_space_name = "production"
single_user_mode = false

[query_resource]
max_concurrent_queries = 100
query_timeout_secs = 60

[storage]
max_memory_bytes = 1073741824   # 1GB
index_memory_bytes = 268435456  # 256MB

[vector]
enabled = true
engine = "qdrant"

[vector.qdrant.connection]
host = "qdrant.example.com"
port = 6334
http_port = 6333
use_tls = true
# api_key = "your-api-key"

[vector.qdrant.timeout]
request_timeout_secs = 30
search_timeout_secs = 60
upsert_timeout_secs = 30

# 嵌入服务配置（可选）
# 使用 OpenAI 嵌入模型
[vector.qdrant.embedding]
base_url = "https://api.openai.com/v1/embeddings"
api_key = "sk-xxx"
model = "text-embedding-3-small"
dimension = 1536
timeout_secs = 60

[fulltext]
enabled = true
index_path = "/var/lib/graphdb/fulltext"

[fulltext.tantivy]
writer_memory_budget = 100000000
tokenizer = "jieba"

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

[monitoring.slow_query_log]
enabled = true
threshold_ms = 1000
log_file_path = "/var/log/graphdb/slow_query.log"
max_file_size_mb = 500
max_files = 10
```

---

## 配置加载优先级

1. 配置文件中的显式配置
2. 配置文件中省略的字段使用默认值
3. 路径类配置相对于配置文件所在目录解析
4. 运行时可通过 HTTP 管理接口查看/修改部分配置（`SHOW CONFIGS` / `UPDATE CONFIGS`）

---

## 配置验证

启动时会自动验证配置：
- 检查必需的配置项
- 验证配置值的合法性（端口非 0、并发数大于 0、内存比率满足 0 < soft < hard <= 1 等）
- HTTPS 启用时必须提供证书与私钥路径

配置错误将导致启动失败并输出错误信息。
