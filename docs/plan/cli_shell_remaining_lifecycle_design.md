# linkrs-shell 剩余任务细化设计：会话生命周期与顺序释放

## 0. 范围界定

本文承接 `docs/plan/cli_shell_relationship_analysis.md`，先复核该文档“整改方案”
的落地状态，再对仍未完成的项给出细化设计并实施。

复核结论（代码基线：当前工作区）：

| 原问题 | 状态 | 代码依据 |
|---|---|---|
| 1. 双入口 CLI 语义平行定义 | 已完成 | 共享执行选项集中在 `linkrs-cli/src/cli.rs` 的 `ExecArgs`；`Cli` 与 shell 的 `Args` 均通过 `#[command(flatten)]` 复用（`linkrs-cli/src/cli.rs:72`、`linkrs-shell/src/main.rs:32`） |
| 2. schema 字段类型拼写降级 | 已完成 | `linkrs-shell/src/backend.rs` 的 `FieldInfo` 使用 `DataType::to_string()` 与 `Value::to_string()`，与 wire 侧 `DataType::Display` 约定一致 |
| 3. 嵌入式错误折叠为连接错误 | 已完成 | `ClientError::Embedded(String)` 已存在（`linkrs-client/src/utils/error.rs:74`）；`EmbeddedBackend` 统一经 `embedded_error` 映射 |
| 4. 跨线程 drop hack | **未完成** | `linkrs-shell/src/main.rs` 仍通过 `tokio::task::spawn_blocking(move \|\| drop(session_mgr))` 把会话移到阻塞线程释放 |
| 5. `_db: Arc` 无共享语义 | 已完成 | `EmbeddedBackend` 直接持有 `GraphDatabase<GraphStorage>`（`linkrs-shell/src/backend.rs:10`），`EmbeddedSession` 内部自持所需 `Arc` |

原文档第四节的两项（中立值枚举替换 JSON、远程 client 拆 feature）仍按原文
“本轮不做”保留，本文不展开，只在第 5 节说明其与本设计的边界。

因此，本轮剩余任务只有问题 4：移除跨线程 drop，恢复显式、顺序的生命周期管理。

---

## 1. 问题根因

### 1.1 现象

`linkrs-shell` 的 `main` 由 `#[tokio::main]` 展开为：

```rust
fn main() {
    tokio::runtime::Runtime::new().unwrap().block_on(async { /* 全部逻辑 */ })
}
```

`SessionManager::Embedded` 中的 `EmbeddedBackend` 持有
`GraphDatabase<GraphStorage>`。该对象内部保活一个**专属 tokio Runtime**
（`linkrs-api/src/embedded/database.rs:287-291`，用于 embedded 模式的向量操作），
存放在 `GraphDatabaseInner.vector_runtime: Arc<Runtime>`。

tokio 禁止在异步执行上下文中释放 `Runtime`：当 `Arc<Runtime>` 的最后一个引用在
`block_on` 的 future 内被 drop 时，会触发 “Cannot drop a runtime in a context
where blocking is not allowed” 的 panic。原有代码因此把 `session_mgr` 移入
`spawn_blocking` 任务，由阻塞线程池负责释放。

### 1.2 为什么当前的规避不可接受

- `spawn_blocking(move || drop(session_mgr))` 把对象所有权移出主线程，Drop 时机
  与实际使用方分离，生命周期不再由调用栈显式表达。
- 它依赖“阻塞线程池一定不在异步上下文”这一 tokio 实现细节，属于用线程切换
  掩盖 drop 语义问题。
- 主运行时在 `block_on` 结束后仍然存活，而会话先被移走释放，释放顺序与对象的
  真实依赖关系（会话依赖数据库，数据库依赖运行时）不一致。

---

## 2. 设计方案

### 2.1 目标

1. 会话与数据库不再跨线程移动，全部在 `main` 线程顺序释放。
2. 释放发生在异步上下文之外，从而避免 tokio 的 Runtime drop panic。
3. 不改变 CLI 行为：参数、连接、输出、退出码、REPL 流程保持一致。

### 2.2 采用方案：同步 `main` + 显式 `Runtime::block_on`

把 `main` 从 `#[tokio::main] async fn` 改为普通同步函数：

1. 在同步栈上完成参数解析、后端打开、`SessionManager` 组装、
   `OutputFormatter` / `CommandExecutor` / `RunOptions` 组装。
2. 显式创建主 tokio 运行时，用 `block_on` 只包裹真正需要异步的片段：
   `session_mgr.connect(...)` 与 `run_with(...)`。
3. `block_on` 返回后，异步上下文已经退出，在 `main` 线程依次
   `drop(executor)`、`drop(session_mgr)`、`drop(runtime)`。

关键点：

- tokio 的“异步执行上下文”只在 `block_on` 执行 future 期间存在；返回后当前线程
  不再处于该上下文，因此 `session_mgr` 中 `vector_runtime` 的 Drop 是安全的。
- 释放顺序与依赖方向一致：先释放命令执行器，再释放会话（含数据库），最后释放
  承载 CLI 异步任务的主运行时。
- 不再有任何跨线程所有权转移。

### 2.3 代码形态

```rust
fn main() {
    let args = Args::parse();
    // ... 同步完成 backend、session_mgr、executor、options 的组装 ...

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("ERROR: failed to start the async runtime: {}", e);
            std::process::exit(1);
        }
    };

    let run_result = runtime.block_on(async {
        if let Err(e) = session_mgr.connect("embedded", "").await {
            eprintln!("WARNING: {}", e);
        }
        run_with(&mut session_mgr, &mut executor, &options).await
    });

    // 已退出异步上下文，在 main 线程顺序释放。
    drop(executor);
    drop(session_mgr);
    drop(runtime);

    if let Err(e) = run_result {
        eprintln!("ERROR: {}", e);
        std::process::exit(1);
    }
}
```

### 2.4 备选方案与取舍

| 方案 | 结论 | 原因 |
|---|---|---|
| 保持 `spawn_blocking` drop | 不采用 | 跨线程转移所有权，掩盖真实生命周期，依赖运行时实现细节 |
| `block_in_place` 内释放 | 不采用 | 仍处于异步上下文，且仅多线程运行时可用；无法解决 Runtime 嵌套 drop 的语义问题 |
| 为 `EmbeddedBackend` 实现 `Drop` 并在其中 `shutdown` | 不采用 | 只要 Drop 发生在 `block_on` 内，`vector_runtime` 的释放仍会 panic；线程切换无法省略 |
| 同步 `main` + `block_on` 后顺序释放 | **采用** | 释放自然发生在异步上下文之外，改动局部、语义清晰 |

### 2.5 边界与风险

- **退出路径**：`run_with` 在 `-c` / `-f` 出错且未加 `--force` 时会直接
  `std::process::exit(1)`。该行为与改造前一致，进程退出会跳过析构，不引入新问题。
- **REPL 阻塞读取**：`InputHandler::read_line` 是同步阻塞调用，改造前后都运行在
  多线程运行时的 `block_on` 线程上，行为不变。
- **运行时创建失败**：改造前 `#[tokio::main]` 在创建失败时 panic；改造后转为
  明确的错误输出并以退出码 1 结束，属于更可控的行为。
- **无 API 变化**：仅调整 `linkrs-shell` 的二进制入口，不触及 `linkrs-cli` 的库接口，
  不影响远程 CLI 与其他消费者。

---

## 3. 代码改动清单

| 文件 | 改动 |
|---|---|
| `crates/linkrs-shell/src/main.rs` | 去掉 `#[tokio::main]`；显式创建运行时并 `block_on` 异步片段；`block_on` 返回后在主线程顺序 drop `executor` / `session_mgr` / `runtime`；删除 `spawn_blocking` 跨线程释放 |

不新增公共类型，不修改 `linkrs-cli`、`linkrs-client`、`linkrs-api`。

---

## 4. 验证

1. `cargo fmt -p linkrs_shell`
2. `cargo check -p linkrs_shell`
3. 命令模式（内存库）：
   `cargo run -q -p linkrs_shell -- -c "\\show_spaces"`，应正常输出并退出码 0。
4. 命令模式（文件库，覆盖文件锁生命周期）：
   - 首次打开并执行 DDL；
   - 进程退出后再次打开同一路径，确认锁已释放且无 panic。
5. REPL 路径：`cargo run -q -p linkrs_shell -- <db> < /dev/null`，
   读到 EOF 后正常退出，确认会话析构不再触发 tokio panic。
6. 只读模式：`--readonly` 打开文件库，确认提示与写拒绝行为不变。

---

## 5. 与原文档第四节的边界

以下两项仍不在本轮范围，维持原文档结论：

1. **中立值枚举替换 JSON 交换格式**：需要同时改造 `linkrs-client::QueryResult`
   的行类型、`output/` 全部渲染器与 `io/` 导入导出链路，属于跨层重构；
   本轮只处理生命周期问题，不改变值交换格式。
2. **远程 client 拆 feature**：涉及 `linkrs-cli` 对 `linkrs-client`、`reqwest`、
   `rpassword` 的可选化以及 `session` / `app` / 连接类元命令的条件编译，
   需要独立设计与 feature 矩阵验证，本轮不动。
