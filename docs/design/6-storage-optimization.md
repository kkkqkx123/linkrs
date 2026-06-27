# 存储与执行优化设计

> 更新日期：2026-06-27
> 依赖：可与阶段 1-2 并行，但推荐在之后进行
> 收益：存储减少 30-70%，Join 性能提升 6-7 倍

---

## 1. 列级压缩选择器

### 1.1 目标

根据列的数据特征，自动选择最优的压缩算法，在保证查询性能的前提下，最大化存储节省。

### 1.2 压缩算法选择矩阵

| 列类型 | 基数特征 | 推荐算法 | 节省比例 |
|--------|---------|---------|---------|
| **ID** | 连续单调 | Delta encoding + RLE | 70-80% |
| **Timestamp** | 单调递增 | Delta encoding | 60-70% |
| **枚举类型** | 低基数（<1000） | Dictionary | 80-90% |
| **年龄/等级** | 低基数（<100） | Dictionary + RLE | 85-95% |
| **随机数据** | 高基数 | LZ4（快速压缩） | 20-30% |
| **文本** | 高基数 | ZSTD（强压缩） | 40-60% |

### 1.3 选择器实现

```rust
/// 列压缩类型
pub enum CompressionType {
    None,                                    // 无压缩
    DeltaEncoding,                           // 差值编码
    RLE,                                     // 游程编码
    Dictionary,                              // 字典压缩
    LZ4,                                     // 快速压缩
    ZSTD,                                    // 强压缩
}

/// 压缩选择器
pub struct CompressionSelector;

impl CompressionSelector {
    /// 根据列的样本数据推荐压缩算法
    pub fn select(
        column_name: &str,
        sample: &[Value],
    ) -> CompressionType {
        let cardinality = Self::estimate_cardinality(sample);
        let monotonic = Self::is_monotonic(sample);
        let value_type = Self::infer_type(sample);
        
        match (value_type, cardinality, monotonic) {
            // ID 和时间戳：选择差值编码
            (ValueType::Integer, _, true) if column_name.contains("id") => {
                CompressionType::DeltaEncoding
            }
            (ValueType::Integer, _, true) if column_name.contains("time") => {
                CompressionType::DeltaEncoding
            }
            
            // 低基数：选择字典编码
            (_, c, _) if c < 1000 => {
                CompressionType::Dictionary
            }
            
            // 文本：选择强压缩
            (ValueType::String, _, _) => {
                CompressionType::ZSTD
            }
            
            // 默认：不压缩（高基数随机数据压缩效率低）
            _ => {
                CompressionType::None
            }
        }
    }
    
    fn estimate_cardinality(sample: &[Value]) -> usize {
        let mut unique = HashSet::new();
        for val in sample.iter().take(10000) {
            unique.insert(val.hash());
        }
        unique.len()
    }
    
    fn is_monotonic(sample: &[Value]) -> bool {
        sample.windows(2).all(|w| w[0] <= w[1])
    }
    
    fn infer_type(sample: &[Value]) -> ValueType {
        // 推断主要类型
        unimplemented!()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_select_id_column() {
        let sample = vec![
            Value::Integer(1),
            Value::Integer(2),
            Value::Integer(3),
        ];
        let compression = CompressionSelector::select("user_id", &sample);
        assert_eq!(compression, CompressionType::DeltaEncoding);
    }
    
    #[test]
    fn test_select_low_cardinality() {
        let sample = vec![
            Value::String("active".into()),
            Value::String("inactive".into()),
            Value::String("active".into()),
        ];
        let compression = CompressionSelector::select("status", &sample);
        assert_eq!(compression, CompressionType::Dictionary);
    }
}
```

### 1.4 集成到存储引擎

```rust
impl VertexTable {
    /// 刷新表到磁盘，自动选择压缩
    pub fn flush(&mut self) -> DBResult<()> {
        for (col_idx, column) in self.columns.iter_mut().enumerate() {
            // 选择最优压缩
            let compression = CompressionSelector::select(
                &column.name,
                &column.sample_values,
            );
            
            // 应用压缩并写入
            column.compress(compression)?;
        }
        
        Ok(())
    }
}
```

---

## 2. 并行 HashJoin

### 2.1 目标

实现 HashJoin 的并行化，充分利用多核，提升 Join 性能。

### 2.2 HashJoin 执行模型

```
┌─────────────────────────────────────────┐
│ 并行 HashJoin 执行流程                  │
└─────────────────────────────────────────┘

1. 构建阶段 (Build Phase)
   - 选择 build side（通常是较小的表）
   - 并行分区：将右表按哈希分区
   - 每个分区建立独立的哈希表
   
2. 探测阶段 (Probe Phase)
   - 左表按相同方式分区
   - 每个 probe partition 与对应的 build partition join
   - 并行执行多个分区的 join

3. 结果合并
   - 收集所有分区的 join 结果
   - 向上游返回
```

### 2.3 实现框架

```rust
pub struct StreamingHashJoinExecutor {
    left: Box<StreamingExecutor>,
    right: Box<StreamingExecutor>,
    condition: Expression,
    
    // Build phase
    build_side_partitions: Vec<Vec<DataRow>>,
    num_partitions: usize,
    build_complete: bool,
    
    // Probe phase
    left_partition_iter: Option<std::vec::IntoIter<DataRow>>,
    current_partition: usize,
}

impl StreamingHashJoinExecutor {
    pub fn new(
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        condition: Expression,
        num_partitions: usize,
    ) -> Self {
        Self {
            left,
            right,
            condition,
            build_side_partitions: vec![vec![]; num_partitions],
            num_partitions,
            build_complete: false,
            left_partition_iter: None,
            current_partition: 0,
        }
    }
    
    /// 构建阶段：将右表分区并构建哈希表
    fn build_right_table(&mut self) -> DBResult<()> {
        while let Some(chunk) = self.right.next()? {
            for row in chunk.rows {
                let partition_id = self.hash_partition_id(&row);
                self.build_side_partitions[partition_id].push(row);
            }
        }
        self.build_complete = true;
        Ok(())
    }
    
    /// 探测阶段：用左表探测右表的分区
    fn probe_partition(&mut self, partition_id: usize) -> DBResult<Option<DataChunk>> {
        let mut result_rows = vec![];
        
        // 获取当前分区的行迭代器
        if self.left_partition_iter.is_none() {
            let mut partition_rows = vec![];
            while let Some(chunk) = self.left.next()? {
                // 筛选出属于当前分区的行
                for row in chunk.rows {
                    if self.hash_partition_id(&row) == partition_id {
                        partition_rows.push(row);
                    }
                }
            }
            self.left_partition_iter = Some(partition_rows.into_iter());
        }
        
        // 探测
        if let Some(ref mut iter) = self.left_partition_iter {
            let build_partition = &self.build_side_partitions[partition_id];
            let chunk_size = 1024;
            
            for left_row in iter.take(chunk_size) {
                for right_row in build_partition {
                    if self.condition.evaluate_pair(&left_row, right_row)? {
                        result_rows.push(DataRow::join(&left_row, right_row));
                    }
                }
            }
            
            if result_rows.is_empty() {
                Ok(None)
            } else {
                Ok(Some(DataChunk::from_rows(result_rows)))
            }
        } else {
            Ok(None)
        }
    }
    
    fn hash_partition_id(&self, row: &DataRow) -> usize {
        let hash = row.hash();
        (hash as usize) % self.num_partitions
    }
}

// 在 StreamingExecutor enum 中添加
pub enum StreamingExecutor {
    // ...
    HashJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        condition: Expression,
        build_side_tuples: Vec<DataRow>,
        build_complete: bool,
        opened: bool,
    },
}
```

### 2.4 性能优化

#### 2.4.1 分区数量

```rust
// 推荐分区数 = CPU 核心数 * 2
let num_partitions = std::thread::available_parallelism()
    .unwrap_or(NonZeroUsize::new(8).unwrap())
    .get() * 2;
```

#### 2.4.2 哈希函数

```rust
// 使用快速的哈希函数（而非密码学哈希）
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

fn fast_hash(row: &DataRow) -> u64 {
    let mut hasher = DefaultHasher::new();
    row.hash(&mut hasher);
    hasher.finish()
}
```

#### 2.4.3 构建端优化

```rust
// Build side 可用小的哈希表，减少内存占用
// 如果 build side 太大，自动使用 grace hash join
if self.build_side_tuples.len() > 100_000_000 {
    self.use_grace_hash_join();
}
```

---

## 3. 性能目标

### 3.1 压缩效果

| 场景 | 压缩前 | 压缩后 | 节省 |
|------|--------|--------|------|
| 百万顶点（ID+属性） | 500MB | 150MB | **70%** |
| 千万边（源+目标+时间） | 5GB | 1.5GB | **70%** |
| 混合数据（低/中/高基数） | 1GB | 300-500MB | **30-70%** |

### 3.2 Join 性能

| 表大小 | 单线程 | 8 核并行 | 加速比 |
|--------|--------|---------|--------|
| 1M × 1M | 2000ms | 300ms | **6.7x** |
| 10M × 100k | 5000ms | 800ms | **6.25x** |

---

## 4. 实现清单

### 依赖

- 可与阶段 1-2 并行
- 推荐在阶段 1 完成后进行

### 任务

- [ ] CompressionSelector 实现
- [ ] 各种压缩算法集成
- [ ] 存储引擎集成（flush/load）
- [ ] 并行 HashJoin executor
- [ ] 分区和背压机制
- [ ] 基准测试（压缩率、性能）

### 验收标准

- [ ] 压缩率：30-70%（取决于数据特征）
- [ ] 压缩速度：不超过原始写入时间的 30%
- [ ] Join 性能：6-7 倍加速
- [ ] 所有集成测试通过

---

## 5. 实施建议

### 5.1 优先级

1. **列级压缩选择器**（高优先级）
   - 收益立竿见影（30-70% 存储节省）
   - 复杂度中等
   - 工期：1.5 周

2. **并行 HashJoin**（中优先级）
   - 收益有限（6-7 倍加速，仅限于 Join 操作）
   - 复杂度高
   - 工期：1.5 周

### 5.2 与其他阶段的关系

```
阶段 0-2 （架构 + executor 流式化）
    ↓
    ↓ （9 周后）
    ↓
阶段 3 （存储优化）← 可独立进行
```

---

## 总结

- ✅ **列级压缩**：自动化选择，简单高效
- ✅ **并行 Join**：充分利用多核
- ✅ **性能目标明确**：存储 30-70%，Join 6-7x
- ✅ **可独立进行**：不依赖前面的阶段
