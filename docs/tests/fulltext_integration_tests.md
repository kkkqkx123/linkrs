# 全文检索集成测试说明

## 概述

本文档说明了 GraphDB 项目全文检索功能的集成测试文件和测试覆盖范围。

## 测试文件清单

### 1. 基础集成测试文件

**文件路径**: `tests/integration_fulltext_search.rs`

**测试类别**:
- ✅ 基本 CRUD 操作（创建索引、插入、搜索、更新、删除）
- ✅ 多字段和多标签测试
- ✅ 批量操作测试
- ✅ 搜索引擎类型测试（BM25、Inversearch）
- ✅ 同步管理器测试
- ✅ 并发操作测试
- ✅ 边缘情况和错误处理

**测试数量**: 25+ 个测试用例

**核心测试场景**:
1. `test_fulltext_create_index` - 创建全文索引
2. `test_fulltext_insert_and_search` - 插入和搜索
3. `test_fulltext_update_vertex` - 更新顶点数据
4. `test_fulltext_delete_vertex` - 删除顶点数据
5. `test_fulltext_multiple_fields_on_same_tag` - 同一标签的多字段索引
6. `test_fulltext_same_field_on_different_tags` - 不同标签的相同字段
7. `test_fulltext_batch_insert` - 批量插入
8. `test_fulltext_batch_delete` - 批量删除
9. `test_fulltext_scoring_and_sorting` - 评分和排序
10. `test_fulltext_limit_and_offset` - 限制和偏移
11. `test_fulltext_special_characters` - 特殊字符处理
12. `test_fulltext_unicode_content` - Unicode 内容支持
13. `test_fulltext_bm25_engine` - BM25 引擎测试
14. `test_fulltext_inversearch_engine` - Inversearch 引擎测试
15. `test_sync_manager_async_mode` - 异步同步模式
16. `test_sync_manager_sync_mode` - 同步模式
17. `test_fulltext_empty_search` - 空搜索
18. `test_fulltext_duplicate_index_creation` - 重复索引创建
19. `test_fulltext_non_existent_index_search` - 不存在的索引搜索
20. `test_fulltext_rebuild_index` - 重建索引
21. `test_fulltext_concurrent_inserts` - 并发插入
22. `test_fulltext_concurrent_searches` - 并发搜索
23. `test_fulltext_with_storage_layer` - 与存储层集成（已注释）

### 2. 高级集成测试文件

**文件路径**: `tests/integration_fulltext_advanced.rs`

**测试类别**:
- ✅ 复杂查询场景
- ✅ 性能和压力测试
- ✅ 恢复和持久化
- ✅ 多空间场景
- ✅ 边缘情况和错误条件

**测试数量**: 20+ 个测试用例

**核心测试场景**:
1. `test_fulltext_phrase_search` - 短语搜索
2. `test_fulltext_boolean_operators` - 布尔运算符
3. `test_fulltext_wildcard_search` - 通配符搜索
4. `test_fulltext_large_batch_insert` - 大批量插入（1000 条）
5. `test_fulltext_high_concurrency` - 高并发（500 条）
6. `test_fulltext_rapid_insert_delete_cycle` - 快速插入删除循环
7. `test_fulltext_multiple_spaces_isolation` - 多空间隔离
8. `test_fulltext_cross_space_no_leakage` - 跨空间无泄漏
9. `test_sync_manager_with_recovery` - 带恢复的同步管理器
10. `test_task_buffer_batching` - 任务缓冲批处理
11. `test_fulltext_very_long_content` - 超长内容（10000 词）
12. `test_fulltext_empty_string_content` - 空字符串内容
13. `test_fulltext_special_query_characters` - 特殊查询字符
14. `test_fulltext_mixed_language_content` - 混合语言内容
15. `test_fulltext_numeric_string_content` - 数字字符串
16. `test_fulltext_repeated_same_content` - 重复相同内容
17. `test_fulltext_index_drop_and_recreate` - 索引删除和重建
18. `test_fulltext_with_real_storage_operations` - 真实存储操作（已注释）
19. `test_fulltext_property_type_handling` - 属性类型处理

## 运行测试

### 运行所有全文检索测试

```powershell
# 运行基础测试
cargo test --test integration_fulltext_search

# 运行高级测试
cargo test --test integration_fulltext_advanced

# 运行所有全文检索相关测试
cargo test fulltext
```

### 运行特定测试

```powershell
# 运行特定测试
cargo test test_fulltext_create_index
cargo test test_fulltext_concurrent_inserts
cargo test test_fulltext_high_concurrency

# 运行匹配模式的测试
cargo test test_fulltext_batch
cargo test test_fulltext_concurrent
```

### 运行测试并显示输出

```powershell
# 显示测试输出
cargo test --test integration_fulltext_search -- --nocapture

# 显示成功和失败的测试
cargo test --test integration_fulltext_search -- --show-output
```

## 测试覆盖的架构层次

### 1. 搜索引擎层 (Search Engine Layer)
- ✅ SearchEngine Trait 实现
- ✅ BM25 引擎适配器
- ✅ Inversearch 引擎适配器
- ✅ 引擎工厂模式

### 2. 索引管理层 (Index Management Layer)
- ✅ FulltextIndexManager
- ✅ 索引元数据管理
- ✅ 索引创建和删除
- ✅ 索引重建

### 3. 协调器层 (Coordinator Layer)
- ✅ FulltextCoordinator
- ✅ 顶点变更同步
- ✅ 索引更新协调

### 4. 同步管理层 (Sync Management Layer)
- ✅ SyncManager
- ✅ 同步模式（Sync/Async/Off）
- ✅ 批处理缓冲
- ✅ 恢复机制

### 5. 查询引擎层 (Query Engine Layer)
- ✅ 搜索查询执行
- ✅ 评分和排序
- ✅ 限制和偏移

## 测试数据特点

### 内容类型覆盖
- ✅ 普通文本
- ✅ 特殊字符 (@#$% ^&*())
- ✅ Unicode 内容（中文、日文、emoji）
- ✅ 混合语言内容
- ✅ 数字字符串
- ✅ 超长文本（10000 词）
- ✅ 空字符串

### 操作场景覆盖
- ✅ 单次插入/搜索
- ✅ 批量插入（50-1000 条）
- ✅ 并发插入（50-500 条）
- ✅ 更新和删除
- ✅ 快速循环操作
- ✅ 重建索引

### 并发场景覆盖
- ✅ 单线程操作
- ✅ 多线程并发插入
- ✅ 多线程并发搜索
- ✅ 高并发压力测试

## 已知限制和注释

### 已注释的测试
以下测试已被注释，因为它们依赖于特定的存储层 API：

1. `test_fulltext_with_storage_layer` (integration_fulltext_search.rs)
   - 原因：需要 RedbStorage 的 create_tag API
   - 状态：等待存储层 API 完善

2. `test_fulltext_with_real_storage_operations` (integration_fulltext_advanced.rs)
   - 原因：需要 RedbStorage 的 create_tag API
   - 状态：等待存储层 API 完善

### 测试依赖
- ✅ Tokio 运行时（异步测试）
- ✅ TempDir（临时目录）
- ✅ Futures（并发测试）
- ✅ Parking_lot（锁机制）

## 性能基准

测试中包含的性能参考：

| 测试场景 | 数据量 | 预期时间 |
|---------|--------|---------|
| 基本插入搜索 | 1-3 条 | <100ms |
| 批量插入 | 50-100 条 | <500ms |
| 大批量插入 | 1000 条 | <1s |
| 高并发插入 | 500 条 | <1s |
| 并发搜索 | 10 并发 | <200ms |

## 故障排查

### 常见问题

1. **测试失败：索引不存在**
   ```
   原因：索引创建后没有等待足够时间
   解决：确保调用 commit_all() 和 sleep()
   ```

2. **测试失败：并发测试超时**
   ```
   原因：并发操作未完成
   解决：增加 sleep 时间或检查 Arc 使用
   ```

3. **编译错误：存储 API 不存在**
   ```
   原因：存储层 API 变更
   解决：注释相关测试或更新 API 调用
   ```

## 后续改进建议

### 短期改进
1. 取消注释存储层集成测试
2. 添加更多边界条件测试
3. 增加性能基准测试

### 长期改进
1. 添加模糊测试（fuzzing）
2. 集成到 CI/CD 流程
3. 添加性能回归测试
4. 增加内存和磁盘使用测试

## 参考文档

- [全文检索集成分析](../docs/fulltext_integration_analysis.md)
- [全文检索架构设计](../docs/extend/fulltext_architecture_decision.md)
- [全文检索使用场景](../docs/extend/fulltext_use_cases.md)
- [同步模块设计](../docs/extend/plan/phase4_data_sync_mechanism.md)

---

**文档创建日期**: 2026-04-07  
**测试文件版本**: v1.0  
**适用 GraphDB 版本**: 0.1.0
