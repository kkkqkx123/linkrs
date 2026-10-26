# linkrs-cli 与 linkrs-shell 关系分析及整改方案

## 一、现状与关系

```
linkrs-shell (二进制 crate "linkrs")
  ├── 自定义 Args（database、:memory:、--readonly、-c/-f、--format、--quiet）
  ├── backend.rs: EmbeddedBackend —— 实现 linkrs_cli::session::EmbeddedOps，
  │     持有 GraphDatabase<GraphStorage>（linkrs-api embedded + linkrs-storage），
  │     并把 EmbeddedQueryResult 手工转成 linkrs_cli::client 的 QueryResult
  └── main.rs: 组装 SessionManager::new_embedded(...)，交给 linkrs_cli::app::run_with 驱动

linkrs-cli (库 crate + 自带远程模式二进制)
  ├── lib：REPL/命令解析/输出格式化/补全/config —— 引擎无关
  │     通过 EmbeddedOps trait 反向解耦对 embedded 引擎的依赖
  └── bin：远程模式入口，经 linkrs-client 走 HTTP（cli.rs 定义 Cli/Commands）
```

职责划分方向正确：linkrs-cli 是"壳的通用逻辑库"（解析、执行、渲染），linkrs-shell 是
"嵌入式后端的装配层"（打开本地库、实现 EmbeddedOps 适配器、启动 CLI 框架）。依赖 DAG
无环：shell → cli + api/storage，cli → client/wire/grammar。

## 二、问题清单

| # | 问题 | 严重度 | 位置 |
|---|------|--------|------|
| 1 | 双入口 CLI 语义平行定义：shell 的 `Args` 与 cli 的 `Cli` 各自维护 `-d`/`-c`/`-f`/`--format`/`-q` 等 flag，长期漂移 | 中 | `linkrs-shell/src/main.rs`、`linkrs-cli/src/cli.rs` |
| 2 | 值类型降级：EmbeddedQueryResult → ClientQueryResult 手工逐行 `serde_json::to_value`，以 JSON 为交换格式，丢类型保真度；schema 字段用 `format!("{:?}")` 拼 DataType，与 wire 侧 `DataType::Display` 约定（`BOOL`/`INT`/...）不一致 | 中 | `linkrs-shell/src/backend.rs` |
| 3 | 错误折叠：EmbeddedBackend 把引擎错误统一折成 `ClientError::ConnectionError`/`Other`，本地嵌入式失败被伪装成 HTTP 连接错误 | 中 | `linkrs-shell/src/backend.rs`、`linkrs-client/src/utils/error.rs` |
| 4 | 线程 drop hack：`std::thread::spawn(move || drop(session_mgr)).join()` 绕过会话对象的清理顺序问题 | 低 | `linkrs-shell/src/main.rs` |
| 5 | `EmbeddedBackend._db` 的 `Arc` 无共享语义，仅作字段保活；`_lock`/`_db` 靠命名保活，所有权隐式 | 低 | `linkrs-shell/src/backend.rs` |

## 三、整改方案

1. **统一 CLI 语义（问题 1）**：把两个入口共有的执行相关选项（database/space、
   -c、-f、--format、-q、--force、-1、-v 变量）提取为 `linkrs_cli::cli::ExecArgs`，
   由 `Cli` 与 shell 的 `Args` 各自 `#[command(flatten)]` 复用。远程专属（host/port/
   user/password）留在 `Cli`，嵌入式专属（`--in-memory`/`--readonly`）留在 shell。
   这样 flag 定义只有一份，行为不再漂移；两个二进制继续并存（远程 / 嵌入式是不同的
   部署形态，保留两个入口本身合理）。

2. **消除类型降级（问题 2）**：schema 转换改用 `DataType::to_string()`（Display 已是
   wire 约定的规范拼写）与 `Value::to_string()`；查询结果行保持 JSON 交换格式暂不动
   （替换为中立值枚举涉及 output 层全链路改造，收益/成本比低，列入后续事项）。

3. **新增嵌入式错误变体（问题 3）**：`ClientError` 增加 `Embedded(String)` 变体，
   EmbeddedBackend 引擎错误一律映射到它，不再伪装成连接错误。

4. **去掉线程 drop hack（问题 4）**：main.rs 改为顺序 drop；若 drop 存在运行期阻塞，
   由 backend 显式管理生命周期，不再用跨线程 drop 掩盖。

5. **理顺 EmbeddedBackend 资源所有权（问题 5）**：`_db: Arc<GraphDatabase<_>>` 改为
   直接持有 `GraphDatabase<_>`（session 与 db 同生命周期，无需 Arc）。

## 四、后续事项（本轮不做）

- 用中立值枚举（或 wire 类型）替换 JSON 作为嵌入式结果交换格式，让 output 层泛化。
- 审视 linkrs-cli 的依赖重量（reqwest/rpassword 等随 lib 传递），考虑把远程 client
  拆成 feature，供未来 gpui 客户端等纯交互层消费者复用。
