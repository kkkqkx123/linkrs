# 压缩比例估算方法分析

## 概述

曾计划实现 `ColumnStore::estimate_compression_ratio()` 和 `VertexTable::estimate_space_savings()` 方法，用于在编码前预测压缩效率。该方法已被删除，原因如下。

---

## 删除原因

### 1. 启发式方法过于简化

原始设计基于单一指标 `distinct_ratio`（不同值占比）：

```rust
let compression_factor = if distinct_ratio > 0.9 {
    0.8  // 80% of original size
} else if distinct_ratio > 0.5 {
    0.5  // 50% of original size
} else if distinct_ratio > 0.1 {
    0.3  // 30% of original size
} else {
    0.2  // 20% of original size
};
```

**问题**：
- 忽视数据类型差异（String vs Int 压缩特性完全不同）
- 忽视字符串平均长度的影响
- 忽视实际 NULL 比例对压缩的影响
- 完全独立于实际编码方式选择

### 2. 数值设置不合理

| 情景 | 设置 | 评价 |
|------|------|------|
| 高基数（distinct_ratio > 0.9） | 0.8 | **过于乐观**。高基数数据几乎无法压缩，应接近 1.0 或 > 1.0（可能扩大） |
| 中等基数（distinct_ratio > 0.5） | 0.5 | **不确定**。取决于编码方式和数据类型 |
| 低基数（distinct_ratio < 0.1） | 0.2 | **可能过于激进**。忽视了数据类型特性 |

### 3. 与现有编码系统不一致

项目已有完善的 `CompressionSelector` 系统：
- 根据 `ColumnStats` 自动选择最优编码（FSST、Dictionary、RLE、BitPacking、ALP）
- 每种编码方式有明确的适用场景和压缩特性

新的估算方法绕过了这套系统，维护成本高且容易不同步。

### 4. 无可靠的验证机制

- 没有与实际压缩后的大小进行对比
- 没有精确测试来校准启发式系数
- 容易误导用户做出错误的优化决策

---

## 后续实现方案

### Phase 1：收集真实数据（推荐）

```
1. 在 compact 操作后记录：
   - 压缩前大小
   - 压缩后大小
   - 编码方式
   - 数据特征（distinct_count, null_count, avg_length 等）

2. 累积足够的样本数据

3. 构建机器学习模型或基于真实数据的查表法
```

### Phase 2：基于编码选择器的估算

集成现有的 `CompressionSelector` 逻辑：

```rust
pub fn estimate_compression_ratio(&self) -> (usize, usize, f64) {
    let selector = CompressionSelector::new();
    
    for col in &self.columns {
        let stats = col.compute_stats();
        let encoding = selector.select(&stats);
        
        // 根据 encoding 类型使用不同的估算因子
        let factor = match encoding {
            EncodingType::Dictionary => 0.5,   // 50% 压缩
            EncodingType::Fsst => 0.65,        // 35% 压缩
            EncodingType::Rle => 0.3,          // 70% 压缩
            EncodingType::BitPacking => 0.4,   // 60% 压缩
            // ...
        };
    }
}
```

### Phase 3：精确计算（最终）

- 使用实际的编码库（fsst-rs, zstd）进行样本压缩
- 基于样本推断整列的压缩大小
- 返回更准确的估算值

---

## 现有可用的替代方案

### 1. 获取列统计信息

```rust
let stats = col.compute_stats();  // 已实现
// 包含：row_count, null_count, distinct_count, data_type 等
```

### 2. 选择最优编码

```rust
let selector = CompressionSelector::new();
let encoding = selector.select(&stats);  // 已实现
```

### 3. 手动应用编码

```rust
col.apply_encoding(encoding)?;  // 已实现
// 查询压缩前后的 memory_usage()
```

---

## 建议

**短期**：使用上述替代方案的组合，不进行自动估算。

**中期**：在 compact 操作中记录压缩统计，为后续分析积累数据。

**长期**：基于实际数据构建准确的压缩模型。

---

## 相关代码位置

- `crates/graphdb-storage/src/storage/encoding/selector.rs` - 编码选择器
- `crates/graphdb-storage/src/storage/vertex/column_store.rs` - 列存储和统计
- `crates/graphdb-storage/src/storage/vertex/vertex_table/optimizer.rs` - 压缩优化
