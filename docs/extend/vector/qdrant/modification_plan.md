# Qdrant 向量存储重构方案

## 目标
将当前每个 (space_id, tag, field) 对应独立 Qdrant collection 的模式，重构为每个 space 一个 collection，通过 payload 中的 group_id 实现不同索引的逻辑隔离。

## 当前架构分析

### 现状
- **Collection 命名**: `space_vec_{space_id}_{tag_name}_{field_name}`
- **粒度**: 每个向量索引字段对应一个独立 collection
- **Payload**: 只有 `vertex_id` 字段
- **HNSW 配置**: 每个 collection 独立 HNSW 图

### 存在问题
1. **Collection 数量爆炸**：当存在大量 tag/field 组合时，会产生过多 collections
2. **跨字段查询困难**：无法在一个搜索中跨不同字段的向量
3. **资源利用率低**：每个 collection 独立维护 HNSW 图，内存和计算资源浪费
4. **管理复杂度高**：collection 创建、删除、监控等操作需要遍历所有组合

## 新架构设计

### Collection 粒度
- **按 space 划分**：每个 space 对应一个 collection
- **命名规则**: `space_{space_id}`
- **优势**:
  - 控制 collection 数量与 space 数量线性增长
  - 支持跨字段向量搜索
  - 资源共享，提高内存和计算效率

### Payload 结构
```json
tags: ["Vertex", "User"]
field: "embedding"
group_id: "Vertex_embedding"
vertex_id: "12345"
summary: "用户简介文本..."
```

新增字段说明：
- `group_id`: `{tag_name}_{field_name}`，用于实现逻辑分组和过滤
- `summary`: 可选，存储原始文本内容
- `tags`: vertex 所属的所有标签

### HNSW 配置优化
```json
{
  "m": 16,          // 保持全局 HNSW 索引，保证跨 group 搜索能力
  "payload_m": 16   // 为每个 group_id 构建独立子图，加速同组内搜索
}
```

**不设置 m=0**，原因如下：
1. **全局图提升召回率**：数据点越密集，HNSW 图的连通性越好，导航路径越丰富，检索越精准。合并数据后全局图反而更"结实"。
2. **过滤器保证隔离**：查询时通过 `group_id` 过滤条件（配合 payload 索引）精准限制搜索范围，不会跨组"污染"结果。Qdrant 的可过滤 HNSW 机制在每步导航时都会绕开不符合条件的节点。
3. **payload_m 提供组内快车道**：为 `group_id` 字段设置 payload_m，在全局图基础上为每个组构建专用通道，同类数据检索更快。
4. **保留跨组搜索能力**：如果需要跨 tag/field 的全局语义搜索，保持 m>0 才能实现。只有 m=0 会完全禁用全局索引，无法跨组检索。

**关键**：为 `group_id` 创建 payload 索引（keyword 类型），这是保证过滤性能的前提。

## 修改范围

### 1. Collection 名称生成 (`vector_sync.rs`)
- 修改 `VectorIndexLocation::to_collection_name()` 方法
- 从 `space_vec_{space_id}_{tag}_{field}` 改为 `space_{space_id}`

### 2. 向量点插入 (`vector_sync.rs`)
- 在 `upsert_vertex_vectors()` 中，为每个向量点添加 `group_id`, `tags`, `field` 等 payload 字段
- 保持 `vertex_id` 字段

### 3. 删除操作 (`vector_sync.rs`)
- 修改 `on_vertex_deleted()` 不再需要遍历多个 collections
- 直接在 `space_{space_id}` collection 中通过 `vertex_id` 删除

### 4. 外部客户端 (`external_index/vector_client.rs`)
- 更新 `collection_name()` 方法返回新的命名格式

### 5. API 层 (`vector_api.rs`)
- 更新所有 collection 名称生成逻辑
- 确保创建、删除、搜索等操作使用新命名

### 6. 搜索操作
- 所有搜索必须自动注入 `group_id` 过滤条件
- 使用 Qdrant 的 filter 查询语法确保结果只来自指定的 {tag, field} 组合

### 7. Collection 创建配置
- 在创建 collection 时设置 `hnsw_config.m=0` 和 `hnsw_config.payload_m=16`
- 创建 `group_id` 字段的 payload 索引以加速过滤

## 数据迁移策略
1. **双写阶段**：同时写入旧 collection 和新 collection
2. **数据迁移**：批量读取旧 collections 数据并写入新 collection
3. **查询切换**：逐步将查询流量切到新 collection
4. **清理阶段**：确认无误后删除旧 collections

## 影响评估

### 兼容性
- **不兼容变更**：collection 命名和结构变化
- **建议**：在 v1.0 版本发布前实施，无需考虑向后兼容

### 性能
- **优点**：
  - 减少 collection 数量，降低元数据开销
  - 支持跨字段搜索，提供新功能
  - 更好的资源利用率
- **潜在风险**：
  - 单 collection 数据量增大，需监控性能
  - 查询必须包含 group_id 过滤，否则可能变慢

### 监控
- 需要更新监控指标，关注单 collection 的大小和性能
- 添加对 group_id 分布的监控