# 测试资源优化方案

## 问题背景

运行集成测试（特别是 `cargo test --test integration_dql -p graphdb-query`）时，系统因磁盘 I/O 和内存耗尽而崩溃。

### 根因

- **103 个 DQL 测试 + 大量其他集成测试**，每个独立创建数据库实例
- Rust 默认使用 **7 个并行线程**（CPU 核数），资源叠加
- 每个实例：128 MB 缓存上限 + 4~16 MB WAL 文件 + checkpoint/snapshot 目录
- WAL `SyncPolicy::EveryWrite` 导致每次写入触发 fsync
- 临时文件全部落在 tmpfs（`/tmp`，仅 5.7 GB）上
- 7 个并行实例累积：~896 MB 堆内存 + 临时文件激增，tmpfs 写满后崩溃

### 整体测试规模

| 层级 | 测试数 | DB 实例数 |
|------|--------|-----------|
| 根 `tests/` | ~625 | ~500 |
| `crates/graphdb-query/tests/` | ~955 | ~800 |
| `crates/graphdb-storage/tests/` | 50 | 50 |
| 其他 | ~50 | ~20 |
| **总计** | **~1633** | **~1400** |

~86% 的测试创建独立数据库实例，每个实例默认 128 MB 缓存上限。

---

## 优化方案

### 1. 测试专用配置 Profile（高优先级）

**目标**：将测试实例的缓存上限从 128 MB 降至 8 MB，极大降低内存压力。

**修改 `graphdb-storage`**：

在 `PropertyGraphConfig` 中新增 `test()` 方法：

```rust
impl PropertyGraphConfig {
    /// Create a lightweight test configuration
    pub fn test() -> Self {
        Self {
            enable_cache: true,
            cache_memory: 8 * 1024 * 1024,  // 8 MB for tests
            flush_config: FlushConfig {
                flush_threshold: 10000,  // 提高刷新阈值，减少 flush 频率
                flush_interval: Duration::from_secs(3600), // 测试中不需要定期 flush
            },
            freeze: FreezeConfig::development(),
            merge_config: MergeConfig {
                enable_adaptive_merge: false,
                ..Default::default()
            },
        }
    }
}
```

**修改 `TestStorage`**：使用轻量配置创建存储实例：

```rust
// TestStorage::new() 使用 test() 配置
let storage = Arc::new(RwLock::new(
    GraphStorage::new_with_config(PropertyGraphConfig::test())
        .map_err(|e| Box::new(DBError::from(e)))?,
));
```

**影响**：覆盖所有通过 TestStorage/TestScenario 创建的 ~1300 个测试实例，总内存峰值从 ~900 MB 降至 ~56 MB。

### 2. 统一 TestStorage（高优先级）

**问题**：`tests/common/mod.rs` 和 `crates/graphdb-query/tests/common/mod.rs` 存在两份完全相同的 `TestStorage` 实现，仅 import 路径不同。

**方案**：将 `tests/common/` 提取为独立 crate `graphdb-test-utils`，两个位置都依赖它。

或者更简单的方案：让 `crates/graphdb-query/tests/common/mod.rs` 直接 `use graphdb::...` 重用根级实现。

### 3. 内存模式 vs 持久化模式（中优先级）

**问题**：`TestStorage::new()` 使用 `GraphStorage::new_with_path()`，创建完整持久化存储（WAL、checkpoint、snapshot），但大多数测试不需要持久化语义。

**方案**：为 `TestStorage` 添加内存模式选项：

```rust
pub struct TestStorage {
    storage: Arc<RwLock<GraphStorage>>,
    temp_path: Option<PathBuf>,  // None = in-memory
}
```

无持久化需求时（大多数 DQL/DDL 查询测试），使用 `GraphStorage::new_with_config()`（纯内存），避免 WAL 和 tmpfs 写入。

### 4. 测试线程控制（高优先级）

**问题**：默认 7 个并行线程导致资源叠加。

**短期方案**：在项目根 `.cargo/config.toml` 中添加：

```toml
[env]
RUST_TEST_THREADS = { value = "1", force = true }
```

或更细粒度：在 CI 脚本中设置 `RUST_TEST_THREADS=2`。

**注意**：单线程会延长测试总运行时间，但能避免 OOM。

### 5. 合并共享 Setup 的测试（中优先级）

**问题**：大量测试依次调用：
```
TestScenario::new()
    .setup_space("test")
    .exec_ddl("CREATE TAG Person(...)")
    ...
```

许多测试共享相同的 setup，却各自创建独立的 DB 实例。

**方案**：对纯只读测试使用 `once_cell::sync::Lazy` 共享实例，或使用 `rstest` 框架：

```rust
use once_cell::sync::Lazy;
use std::sync::Mutex;

static SHARED_DB: Lazy<Mutex<TestScenario>> = Lazy::new(|| {
    Mutex::new(
        TestScenario::new()
            .setup_space("test")
            .exec_ddl("CREATE TAG Person(name STRING, age INT)")
            .exec_dml("INSERT VERTEX Person(name, age) VALUES 1:('Alice', 30)")
    )
});
```

**适用场景**：对同一个 schema 做多条只读查询的测试（例如 `go.rs`、`match_query.rs` 中的多个独立测试）。

**注意事项**：测试间隔离需要确保状态不变；增删改操作仍需独立实例。

### 6. 合并同类测试（中优先级）

**问题**：许多测试文件包含大量微小测试，每个仅验证一个查询。

**统计**：
- `aggregation.rs`：33 个测试（16 个创建 DB）
- `match_query.rs`：28 个测试（17 个创建 DB）
- `find_path.rs`：20 个测试（11 个创建 DB）

**方案**：将验证同一特性的微测试合并为数据驱动测试，例如：

```rust
// 之前：3 个独立测试
#[test] fn test_go_simple()  { ... }
#[test] fn test_go_step()    { ... }
#[test] fn test_go_reverse() { ... }

// 之后：1 个参数化测试
#[test]
fn test_go_all() {
    let scenario = shared_scenario();
    for (query, expected_count) in [
        ("GO FROM 1 OVER knows", 3),
        ("GO 1 STEPS FROM 1 OVER knows", 3),
        ("GO FROM 1 OVER knows REVERSELY", 2),
    ] {
        scenario.query(query).assert_result_count(expected_count);
    }
}
```

**注意**：不合并涉及不同 setup 或状态变更的测试。

### 7. 解析器测试与 DB 测试分离（低优先级）

**问题**：多个测试文件前有纯解析器测试（不创建 DB），后有 DB 测试。解析器测试应移到独立的纯解析器测试文件中，或在文件名中加 `#[cfg(test)] mod parser_tests` 隔离，避免被计入 DB 创建测试。

**现状**：DQL 各文件中已有 ~73 个纯解析器测试（占 41%），但这些测试与 DB 测试混在同一文件中，按文件整体执行时仍会加载模块内其他测试的辅助代码。

**改进**：将纯解析器测试移到独立测试文件（如 `dql/parser_go.rs`、`dql/parser_match.rs`），不引用 `TestScenario`，在编译期就消除依赖。

### 8. 减少重复断言模式（低优先级）

**问题**：许多测试使用长链式断言，重复检查相同的属性：

```rust
.assert_success()
.assert_result_count(1)
.assert_result_contains(vec![...])
```

**方案**：在 `TestScenario` 中添加复合断言方法，将常见的断言序列合并为单个调用。

---

## 实施优先级

| 优先级 | 方案 | 预估影响 | 工作量 |
|--------|------|----------|--------|
| P0 | `PropertyGraphConfig::test()` 配置 | 缓存峰值降至 ~8% | 1 天 |
| P0 | `RUST_TEST_THREADS=1` | 消除并行叠加 | 0.5 天 |
| P1 | 统一两份 `TestStorage` | 减少维护成本 | 0.5 天 |
| P1 | `TestStorage` 内存模式选项 | 消除 WAL I/O | 1 天 |
| P2 | 只读测试共享 DB 实例 | 减少 DB 创建数 ~30% | 2 天 |
| P2 | 合并同类测试 | 减少测试文件行数 ~20% | 2 天 |
| P3 | 解析器测试分离 | 编译期解耦 | 1 天 |
| P3 | 减少重复断言 | 改善可读性 | 0.5 天 |

## 预期效果

| 指标 | 优化前 | 优化后（P0+P1） | 优化后（全部） |
|------|--------|---------|---------|
| 最大并行缓存 | ~900 MB | ~56 MB | ~56 MB |
| tmpfs 峰值 | 易超 5.7 GB | < 500 MB | < 200 MB |
| DB 实例数 | ~1400 | ~1400 | ~600 |
| WAL fsync | 每次 DML | 几乎无 | 无（纯内存模式） |

## 实施原则

- **不改变测试逻辑语义**——只改变 setup 方式
- **不破坏测试隔离**——共享实例仅用于只读查询
- **分阶段实施**——优先落地 P0 解决系统崩溃问题，再逐步优化
