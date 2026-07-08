# 全文检索模块测试用例设计

## 一、概述

本文档定义 GraphDB 项目全文检索功能的测试架构和测试用例设计，覆盖单元测试、集成测试两个层次，确保全文检索功能的正确性、性能和可靠性。

## 二、测试范围

### 2.1 被测模块

1. **FulltextIndexManager** - 全文索引管理器
   - 索引元数据管理
   - 搜索引擎实例管理
   - 索引生命周期管理

2. **SyncCoordinator** - 同步协调器
   - 顶点变更同步
   - 批量处理
   - 事务缓冲

3. **SearchEngine** - 搜索引擎接口
   - BM25 引擎
   - Inversearch 引擎

### 2.2 功能范围

| 功能类别 | 功能点 | 测试层次 |
|----------|--------|----------|
| 索引管理 | 创建索引、删除索引、查询索引元数据 | 单元测试 + 集成测试 |
| 索引操作 | 插入文档、更新文档、删除文档、批量操作 | 单元测试 + 集成测试 |
| 搜索查询 | 单词条搜索、多词条搜索、评分排序 | 集成测试 |
| 同步机制 | 实时同步、异步同步、事务缓冲 | 集成测试 |
| 并发控制 | 并发插入、并发搜索、并发更新 | 集成测试 |
| 错误处理 | 索引不存在、重复创建、无效查询 | 单元测试 + 集成测试 |
| 持久化 | 索引提交、索引恢复 | 集成测试 |

## 三、测试架构

### 3.1 测试层次划分

```
┌─────────────────────────────────────────────────────────────┐
│                    Level 2: 集成测试                         │
│  - FulltextIndexManager 集成测试                             │
│  - SyncCoordinator 集成测试                                  │
│  - 与存储层集成测试                                          │
│  - 与事务层集成测试                                          │
└─────────────────────────────────────────────────────────────┘
                              ↓
┌─────────────────────────────────────────────────────────────┐
│                    Level 1: 单元测试                         │
│  - SearchEngine 接口实现测试                                 │
│  - IndexKey/IndexMetadata 单元测试                           │
│  - FulltextConfig 配置测试                                   │
└─────────────────────────────────────────────────────────────┘
```

### 3.2 单元测试（Unit Tests）

**位置**: `src/search/` 模块内的 `#[cfg(test)]` 测试

**测试内容**:
- 索引键生成逻辑
- 元数据序列化/反序列化
- 配置验证
- 搜索引擎工厂

**示例**:

```rust
// src/search/metadata.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_key_generation() {
        let key = IndexKey::new(1, "Person", "name");
        assert_eq!(key.to_index_id(), "1.Person.name");
    }

    #[test]
    fn test_index_metadata_serialization() {
        let metadata = IndexMetadata {
            index_id: "1.Person.name".to_string(),
            // ... 其他字段
        };
        let serialized = serde_json::to_string(&metadata).unwrap();
        let deserialized: IndexMetadata = serde_json::from_str(&serialized).unwrap();
        assert_eq!(metadata.index_id, deserialized.index_id);
    }
}
```

### 3.3 集成测试（Integration Tests）

**位置**: `tests/` 目录下的独立测试文件

**测试文件组织**:

```
tests/
├── integration_fulltext_basic.rs      # 基础 CRUD 操作
├── integration_fulltext_sync.rs       # 同步机制测试
├── integration_fulltext_concurrent.rs # 并发测试
├── integration_fulltext_edge_cases.rs # 边缘情况测试
└── common/
    ├── mod.rs                         # 公共测试工具
    └── fulltext_helpers.rs            # 全文检索测试辅助
```

## 四、集成测试用例设计

### 4.1 基础功能测试 (integration_fulltext_basic.rs)

#### 4.1.1 索引管理测试

```rust
// TC-FT-001: 创建全文索引
#[tokio::test]
async fn test_create_fulltext_index() {
    // 场景：为 Tag 的字符串字段创建全文索引
    // 步骤:
    // 1. 创建 FulltextIndexManager
    // 2. 调用 create_index(space_id=1, tag="Article", field="content", engine=BM25)
    // 3. 验证返回的 index_id 格式正确
    // 4. 验证 has_index 返回 true
    // 预期：索引创建成功，元数据正确
}

// TC-FT-002: 重复创建索引
#[tokio::test]
async fn test_create_duplicate_index() {
    // 场景：重复创建相同索引
    // 步骤:
    // 1. 创建索引
    // 2. 再次调用 create_index 相同参数
    // 预期：返回 IndexAlreadyExists 错误
}

// TC-FT-003: 删除索引
#[tokio::test]
async fn test_drop_index() {
    // 场景：删除全文索引
    // 步骤:
    // 1. 创建索引
    // 2. 调用 drop_index
    // 3. 验证 has_index 返回 false
    // 4. 验证索引文件被删除
    // 预期：索引删除成功，清理磁盘文件
}

// TC-FT-004: 查询索引元数据
#[tokio::test]
async fn test_get_index_metadata() {
    // 场景：获取索引元数据
    // 步骤:
    // 1. 创建索引
    // 2. 调用 get_metadata
    // 3. 验证返回的元数据包含正确信息
    // 预期：元数据查询成功
}

// TC-FT-005: 列出空间的所有索引
#[tokio::test]
async fn test_get_space_indexes() {
    // 场景：获取空间下所有索引
    // 步骤:
    // 1. 为同一空间创建多个索引
    // 2. 调用 get_space_indexes(space_id)
    // 3. 验证返回的索引列表包含所有索引
    // 预期：索引列表完整
}
```

#### 4.1.2 索引操作测试

```rust
// TC-FT-006: 插入文档并搜索
#[tokio::test]
async fn test_index_and_search() {
    // 场景：插入文档并执行搜索
    // 步骤:
    // 1. 创建索引
    // 2. 获取引擎并调用 index(doc_id="1", content="Hello World")
    // 3. 调用 commit_all()
    // 4. 调用 search(query="Hello", limit=10)
    // 5. 验证返回结果包含 doc_id="1"
    // 预期：搜索成功，返回正确结果
}

// TC-FT-007: 批量插入文档
#[tokio::test]
async fn test_batch_index() {
    // 场景：批量插入多个文档
    // 步骤:
    // 1. 创建索引
    // 2. 获取引擎并调用 index_batch(vec![(doc_id, content), ...])
    // 3. 调用 commit_all()
    // 4. 验证所有文档可搜索
    // 预期：批量插入成功
}

// TC-FT-008: 更新文档
#[tokio::test]
async fn test_update_document() {
    // 场景：更新已存在的文档
    // 步骤:
    // 1. 插入文档 doc_id="1", content="Old Content"
    // 2. 更新文档 doc_id="1", content="New Content"
    // 3. 搜索 "Old" 应该无结果
    // 4. 搜索 "New" 应该有结果
    // 预期：文档更新成功
}

// TC-FT-009: 删除文档
#[tokio::test]
async fn test_delete_document() {
    // 场景：删除文档
    // 步骤:
    // 1. 插入文档
    // 2. 调用 delete(doc_id="1")
    // 3. 搜索该文档应该无结果
    // 预期：文档删除成功
}

// TC-FT-010: 批量删除文档
#[tokio::test]
async fn test_batch_delete() {
    // 场景：批量删除多个文档
    // 步骤:
    // 1. 插入多个文档
    // 2. 调用 delete_batch(vec![doc_id1, doc_id2, ...])
    // 3. 验证所有文档不可搜索
    // 预期：批量删除成功
}
```

#### 4.1.3 搜索功能测试

```rust
// TC-FT-011: 单词条搜索
#[tokio::test]
async fn test_single_term_search() {
    // 场景：单词条搜索
    // 步骤:
    // 1. 插入多个包含不同词汇的文档
    // 2. 搜索单个词汇
    // 3. 验证返回结果包含该词汇的文档
    // 预期：搜索结果正确
}

// TC-FT-012: 多词条搜索
#[tokio::test]
async fn test_multi_term_search() {
    // 场景：多词条搜索
    // 步骤:
    // 1. 插入多个文档
    // 2. 搜索多个词汇
    // 3. 验证返回结果按相关性排序
    // 预期：搜索结果按评分排序
}

// TC-FT-013: 搜索结果限制
#[tokio::test]
async fn test_search_limit() {
    // 场景：限制搜索结果数量
    // 步骤:
    // 1. 插入 100 个文档
    // 2. 搜索并设置 limit=10
    // 3. 验证返回结果数量为 10
    // 预期：结果数量正确
}

// TC-FT-014: 空搜索
#[tokio::test]
async fn test_empty_search() {
    // 场景：搜索不存在的词汇
    // 步骤:
    // 1. 插入文档
    // 2. 搜索不存在的词汇
    // 3. 验证返回空结果
    // 预期：返回空列表
}

// TC-FT-015: 特殊字符搜索
#[tokio::test]
async fn test_special_characters_search() {
    // 场景：包含特殊字符的搜索
    // 步骤:
    // 1. 插入包含特殊字符的文档
    // 2. 搜索特殊字符
    // 3. 验证正确处理
    // 预期：特殊字符被正确处理
}
```

### 4.2 同步机制测试 (integration_fulltext_sync.rs)

#### 4.2.1 SyncCoordinator 基础测试

```rust
// TC-FT-016: 顶点插入自动同步
#[tokio::test]
async fn test_vertex_insert_auto_sync() {
    // 场景：通过 SyncCoordinator 插入顶点自动同步索引
    // 步骤:
    // 1. 创建 SyncCoordinator
    // 2. 创建索引
    // 3. 调用 on_vertex_change(Insert)
    // 4. 调用 commit_all()
    // 5. 验证可通过 FulltextIndexManager 搜索到
    // 预期：索引自动同步
}

// TC-FT-017: 顶点更新自动同步
#[tokio::test]
async fn test_vertex_update_auto_sync() {
    // 场景：顶点属性更新自动同步索引
    // 步骤:
    // 1. 插入顶点
    // 2. 调用 on_vertex_change(Update) 更新属性
    // 3. 验证旧索引删除，新索引创建
    // 预期：索引正确更新
}

// TC-FT-018: 顶点删除自动同步
#[tokio::test]
async fn test_vertex_delete_auto_sync() {
    // 场景：顶点删除自动同步索引
    // 步骤:
    // 1. 插入顶点
    // 2. 调用 on_vertex_change(Delete)
    // 3. 验证索引删除
    // 预期：索引删除成功
}
```

#### 4.2.2 事务缓冲测试

```rust
// TC-FT-019: 事务缓冲插入
#[tokio::test]
async fn test_transaction_buffered_insert() {
    // 场景：事务内缓冲索引操作
    // 步骤:
    // 1. 开启事务
    // 2. 调用 buffer_operation 缓冲多个插入
    // 3. 验证缓冲计数正确
    // 4. 提交事务
    // 5. 验证索引同步
    // 预期：事务提交后批量同步
}

// TC-FT-020: 事务回滚
#[tokio::test]
async fn test_transaction_rollback() {
    // 场景：事务回滚丢弃缓冲操作
    // 步骤:
    // 1. 开启事务
    // 2. 缓冲多个操作
    // 3. 调用 rollback_transaction
    // 4. 验证索引未同步
    // 预期：回滚后索引未改变
}

// TC-FT-021: 多事务并发缓冲
#[tokio::test]
async fn test_concurrent_transaction_buffers() {
    // 场景：多个并发事务的缓冲隔离
    // 步骤:
    // 1. 开启多个事务
    // 2. 每个事务缓冲不同操作
    // 3. 验证各事务缓冲独立
    // 4. 分别提交
    // 预期：事务隔离正确
}
```

### 4.3 并发测试 (integration_fulltext_concurrent.rs)

```rust
// TC-FT-022: 并发插入
#[tokio::test]
async fn test_concurrent_inserts() {
    // 场景：多线程并发插入文档
    // 步骤:
    // 1. 创建索引
    // 2. 启动 100 个并发任务插入文档
    // 3. 等待所有任务完成
    // 4. 验证所有文档可搜索
    // 预期：并发插入成功，无数据丢失
}

// TC-FT-023: 并发搜索
#[tokio::test]
async fn test_concurrent_searches() {
    // 场景：多线程并发搜索
    // 步骤:
    // 1. 插入数据
    // 2. 启动 100 个并发任务执行搜索
    // 3. 验证所有搜索返回正确结果
    // 预期：并发搜索安全
}

// TC-FT-024: 并发插入和搜索
#[tokio::test]
async fn test_concurrent_insert_and_search() {
    // 场景：并发插入和搜索混合
    // 步骤:
    // 1. 启动并发任务：一部分插入，一部分搜索
    // 2. 验证搜索能看到已提交的数据
    // 3. 验证无崩溃
    // 预期：读写并发安全
}

// TC-FT-025: 并发更新同一文档
#[tokio::test]
async fn test_concurrent_updates_same_document() {
    // 场景：并发更新同一文档
    // 步骤:
    // 1. 插入文档
    // 2. 启动多个并发任务更新同一文档
    // 3. 验证最终状态一致
    // 预期：并发更新安全
}
```

### 4.4 边缘情况和错误处理测试 (integration_fulltext_edge_cases.rs)

```rust
// TC-FT-026: 索引不存在时搜索
#[tokio::test]
async fn test_search_non_existent_index() {
    // 场景：在不存在的索引上搜索
    // 步骤:
    // 1. 不调用 create_index
    // 2. 直接调用 search
    // 预期：返回 IndexNotFound 错误
}

// TC-FT-027: 空字符串内容
#[tokio::test]
async fn test_index_empty_content() {
    // 场景：索引空字符串内容
    // 步骤:
    // 1. 调用 index(doc_id="1", content="")
    // 2. 调用 commit_all()
    // 3. 搜索任意词汇
    // 预期：不报错，但无结果
}

// TC-FT-028: 超长内容
#[tokio::test]
async fn test_index_very_long_content() {
    // 场景：索引超长内容
    // 步骤:
    // 1. 创建 10000 词的文档
    // 2. 插入索引
    // 3. 搜索其中的词汇
    // 预期：正确处理长文档
}

// TC-FT-029: Unicode 内容
#[tokio::test]
async fn test_index_unicode_content() {
    // 场景：索引 Unicode 内容
    // 步骤:
    // 1. 插入包含中文、emoji 的文档
    // 2. 搜索 Unicode 词汇
    // 预期：Unicode 正确处理
}

// TC-FT-030: 特殊查询字符
#[tokio::test]
async fn test_special_query_characters() {
    // 场景：查询包含特殊字符
    // 步骤:
    // 1. 插入文档
    // 2. 搜索包含 *, ?, + 等特殊字符的查询
    // 预期：特殊字符被转义或正确处理
}

// TC-FT-031: 索引重建
#[tokio::test]
async fn test_rebuild_index() {
    // 场景：删除并重建索引
    // 步骤:
    // 1. 创建索引并插入数据
    // 2. 删除索引
    // 3. 重新创建同名索引
    // 4. 插入新数据
    // 预期：重建后索引正常工作
}

// TC-FT-032: 多空间隔离
#[tokio::test]
async fn test_multi_space_isolation() {
    // 场景：多个空间的索引隔离
    // 步骤:
    // 1. 为 space1 和 space2 创建相同 tag.field 的索引
    // 2. 在 space1 插入数据
    // 3. 在 space2 搜索
    // 预期：space2 搜索不到 space1 的数据
}

// TC-FT-033: 内存限制
#[tokio::test]
async fn test_memory_limit() {
    // 场景：大量索引的内存控制
    // 步骤:
    // 1. 创建大量索引
    // 2. 验证内存使用在合理范围
    // 预期：内存使用受控
}
```

## 五、测试基础设施

### 5.1 测试辅助工具

**文件**: `tests/common/fulltext_helpers.rs`

```rust
use graphdb::search::{FulltextConfig, FulltextIndexManager, EngineType};
use graphdb::sync::{SyncCoordinator, BatchConfig};
use std::sync::Arc;
use tempfile::TempDir;

/// 全文检索测试上下文
pub struct FulltextTestContext {
    pub manager: Arc<FulltextIndexManager>,
    pub coordinator: Option<Arc<SyncCoordinator>>,
    pub temp_dir: TempDir,
}

impl FulltextTestContext {
    /// 创建基础测试上下文
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp dir");
        let config = FulltextConfig {
            enabled: true,
            index_path: temp_dir.path().to_path_buf(),
            default_engine: EngineType::Bm25,
            ..Default::default()
        };
        let manager = Arc::new(
            FulltextIndexManager::new(config)
                .expect("Failed to create manager")
        );
        Self {
            manager,
            coordinator: None,
            temp_dir,
        }
    }

    /// 创建带 SyncCoordinator 的测试上下文
    pub fn with_sync() -> Self {
        let ctx = Self::new();
        let coordinator = Arc::new(
            SyncCoordinator::new(ctx.manager.clone(), BatchConfig::default())
        );
        Self {
            manager: ctx.manager,
            coordinator: Some(coordinator),
            temp_dir: ctx.temp_dir,
        }
    }

    /// 创建测试索引
    pub async fn create_test_index(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
    ) -> Result<String, SearchError> {
        self.manager
            .create_index(space_id, tag_name, field_name, None)
            .await
    }

    /// 插入测试文档
    pub async fn insert_test_doc(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        doc_id: &str,
        content: &str,
    ) -> Result<(), SearchError> {
        use graphdb::search::engine::SearchEngine;
        if let Some(engine) = self.manager.get_engine(space_id, tag_name, field_name) {
            engine.index(doc_id, content).await?;
        }
        Ok(())
    }

    /// 执行搜索
    pub async fn search(
        &self,
        space_id: u64,
        tag_name: &str,
        field_name: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SearchResult>, SearchError> {
        self.manager
            .search(space_id, tag_name, field_name, query, limit)
            .await
    }
}
```

### 5.2 测试数据生成

```rust
/// 生成测试文档数据
pub fn generate_test_docs(count: usize, prefix: &str) -> Vec<(String, String)> {
    (0..count)
        .map(|i| {
            (
                format!("doc_{}", i),
                format!("{} document number {} with some content for testing", prefix, i),
            )
        })
        .collect()
}

/// 创建带全文索引的测试 Tag
pub fn create_tag_with_fulltext(
    storage: &StorageClient,
    space_name: &str,
    tag_name: &str,
    field_name: &str,
) -> Result<(), CoreError> {
    // 创建 Tag 定义
    let tag_info = TagInfo::new(tag_name)
        .with_field(field_name, FieldType::String);
    
    // 创建 Tag
    storage.create_tag(space_name, &tag_info)?;
    
    // 创建全文索引
    let index = Index::new(IndexConfig {
        name: format!("idx_{}_{}", tag_name, field_name),
        space_name: space_name.to_string(),
        schema_name: tag_name.to_string(),
        fields: vec![IndexField::new(field_name, FieldType::String, false)],
        index_type: IndexType::FulltextIndex,
        ..Default::default()
    });
    
    storage.create_tag_index(space_name, &index)?;
    Ok(())
}
```

## 六、测试执行

### 6.1 运行测试

```powershell
# 运行所有全文检索集成测试
cargo test --test integration_fulltext_basic
cargo test --test integration_fulltext_sync
cargo test --test integration_fulltext_concurrent
cargo test --test integration_fulltext_edge_cases

# 运行所有全文检索相关测试
cargo test fulltext

# 运行特定测试
cargo test test_create_fulltext_index
cargo test test_concurrent_inserts -- --nocapture

# 运行所有测试（包括单元测试）
cargo test --lib search
```

### 6.2 测试覆盖率

```powershell
# 生成覆盖率报告
cargo tarpaulin --out Html --output-dir ./target/coverage --test integration_fulltext_basic
cargo tarpaulin --out Html --output-dir ./target/coverage --test integration_fulltext_sync
```

## 七、与现有测试的区别

### 7.1 单元测试（已有）

- **位置**: `src/search/*.rs` 中的 `#[cfg(test)]` 模块
- **内容**: 测试单个函数/方法的逻辑
- **特点**: 快速、隔离、使用 Mock

### 7.2 集成测试（本文档）

- **位置**: `tests/integration_fulltext_*.rs`
- **内容**: 测试模块间交互和完整功能
- **特点**: 真实环境、多模块协作、验证端到端流程

### 7.3 避免重复

- 单元测试已覆盖的逻辑不在集成测试中重复
- 集成测试专注于：
  - 模块间协作
  - 真实场景验证
  - 性能和并发
  - 错误处理和边缘情况

## 八、测试优先级

### P0 - 必须实现（核心功能）

- TC-FT-001 ~ TC-FT-005: 索引管理
- TC-FT-006 ~ TC-FT-010: 索引操作
- TC-FT-011 ~ TC-FT-015: 搜索功能
- TC-FT-016 ~ TC-FT-018: 同步机制

### P1 - 重要（质量保证）

- TC-FT-019 ~ TC-FT-021: 事务缓冲
- TC-FT-022 ~ TC-FT-025: 并发测试
- TC-FT-026 ~ TC-FT-030: 错误处理

### P2 - 可选（增强覆盖）

- TC-FT-031 ~ TC-FT-033: 边缘情况

## 九、后续工作

1. **实现测试文件**: 根据本文档创建实际的测试文件
2. **补充单元测试**: 在 `src/search/` 中补充缺失的单元测试
3. **性能基准**: 在 `benches/` 目录创建性能基准测试
4. **自动化**: 将测试集成到 CI/CD 流程

## 十、参考资料

- `src/search/manager.rs` - FulltextIndexManager 实现
- `src/search/engine.rs` - SearchEngine trait 定义
- `src/sync/coordinator/coordinator.rs` - SyncCoordinator 实现
- `docs/crates/search_engines_analysis.md` - 搜索引擎分析
- `docs/fulltext_integration_analysis.md` - 全文集成分析
