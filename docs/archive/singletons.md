# GraphDB 全局单例使用说明

本文档记录 GraphDB 项目中全局单例的使用情况，包括设计理由、使用场景和注意事项。

## 设计原则

根据项目编码标准，我们尽量减少全局单例的使用，优先使用依赖注入模式。但在以下场景下，全局单例是合理且必要的：

1. **纯配置数据**：只读的静态配置，初始化后不再改变
2. **资源管理**：需要全局唯一实例的资源（如日志系统）
3. **跨组件共享**：需要在整个应用生命周期内共享的状态

---

## 当前全局单例列表

### 1. 函数注册表 (FunctionRegistry)

**位置**: `src/expression/functions/registry.rs`

**实现方式**:
```rust
pub fn global_registry() -> Arc<FunctionRegistry> {
    static REGISTRY: OnceLock<Arc<FunctionRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Arc::new(FunctionRegistry::new())).clone()
}
```

**设计理由**:
- 函数注册表是只读的（注册后不变），线程安全
- 内置函数在编译期确定，不需要动态修改
- 使用 `Arc` 共享，避免重复创建
- 符合项目规则（避免 `dyn`，使用静态分发）

**使用场景**:
- 表达式求值时查找函数
- 查询验证时检查函数存在性

**注意事项**:
- 自定义函数注册后无法动态更新（当前设计限制）
- 如需支持动态函数注册，需要考虑使用 `RwLock` 或重构为依赖注入

---

### 2. 日志句柄 (LOGGER_HANDLE)

**位置**: `src/utils/logging.rs`

**实现方式**:
```rust
static LOGGER_HANDLE: Mutex<Option<LoggerHandle>> = Mutex::new(None);
```

**设计理由**:
- 日志系统需要全局唯一实例
- 程序退出时需要获取句柄进行 flush
- 使用 `parking_lot::Mutex` 保证线程安全

**使用场景**:
- 程序退出时 flush 日志
- 检查日志是否已初始化

**注意事项**:
- 日志句柄在 `init()` 中设置，在 `shutdown()` 中释放
- 使用 `parking_lot::Mutex` 而非 `std::sync::Mutex`，性能更好

---

## 使用指南

### 何时使用全局单例

✅ **可以使用**:
- 只读的静态配置数据
- 真正的全局资源（如日志系统）
- 应用生命周期内不变的共享状态

❌ **避免使用**:
- 可变状态（除非使用适当的同步机制）
- 业务逻辑组件
- 需要独立测试的组件
- 可能需要在不同上下文中使用不同实例的组件

### 实现建议

1. **使用 `std::sync::OnceLock` 或 `std::sync::LazyLock`**
   - Rust 标准库提供，无需外部依赖
   - 线程安全，性能良好

2. **优先使用 `Arc` 共享**
   - 避免直接暴露静态变量
   - 便于测试时替换为 mock 实现

3. **考虑使用 `parking_lot` 的同步原语**
   - 性能优于标准库
   - API 更简洁（如 `Mutex` 不需要 `unwrap`）

4. **文档化**
   - 说明为什么需要全局单例
   - 记录使用场景和注意事项
   - 在本文档中登记

---

## 重构历史

| 日期 | 修改内容 | 相关文件 |
|------|----------|----------|
| 2026-02-27 | 移除全局 ID 生成器 | `src/common/id.rs` |
| 2026-02-27 | 移除 EPIdGenerator | `src/utils/id_gen.rs` |
| 2026-02-27 | 移除全局查询管理器 | `src/query/query_manager.rs` |
| 2026-02-27 | logging.rs 使用 parking_lot::Mutex | `src/utils/logging.rs` |

---

## 相关文档

- [动态分发使用报告](./dynamic.md)
- [unsafe 使用报告](./unsafe.md)
