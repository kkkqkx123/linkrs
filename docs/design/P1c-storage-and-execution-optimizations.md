# P1.c 存储与执行优化设计

> 设计日期：2026-06-27
> 依赖基础：P1.a（可选 P1.b）
> 关键收益：存储空间节省 30-70%，Join 性能提升 3-8 倍
> 预计工期：2-3 周

---

## 1. 概述

在 P1.a 并行执行框架的基础上，进行两项相对独立的优化：
1. **列级压缩增强**：自动选择最优压缩算法
2. **并行 HashJoin**：Build-Probe 分离，支持多线程并行

两项可以并行进行，不相互依赖。

---

## 第一部分：列级压缩选择器增强

## 2. 列级压缩优化

### 2.1 现状分析

| 列类型 | 当前策略 | 可优化空间 | 目标压缩率 |
|--------|---------|-----------|-----------|
| Int (小范围) | 无压缩 (4 bytes) | 使用 Bitpack | ~75% (1 byte) |
| Int (大范围) | 无压缩 | 使用 Frame-of-Reference | ~50% (2 bytes) |
| Float | 无压缩 (4 bytes) | 使用 FSC 或 Gorilla | ~40% (2-3 bytes) |
| String (字典) | RawString | 使用 FSST | ~40-70% |
| String (高基数) | RawString | 使用 Delta + LZ4 | ~30-50% |

### 2.2 压缩算法选择框架

```rust
/// 自动选择最优压缩算法
pub struct CompressionSelector {
    /// 列的统计信息
    stats: ColumnStats,
}

pub struct ColumnStats {
    pub data_type: DataType,
    pub num_values: usize,
    pub null_count: usize,
    
    // 数值列统计
    pub min_value: Option<i64>,
    pub max_value: Option<i64>,
    pub distinct_count: usize,
    pub entropy: f64,
    
    // 字符串列统计
    pub avg_length: usize,
    pub max_length: usize,
    pub unique_strings: usize,
}

pub enum CompressionStrategy {
    /// 无压缩
    None,
    
    /// 整数压缩
    Bitpack {
        bits_per_value: usize,  // 计算范围所需 bit 数
    },
    FrameOfReference {
        frame_size: usize,
    },
    
    /// 浮点数压缩
    Gorilla,
    FSC,
    
    /// 字符串压缩
    Dictionary {
        dict_size: usize,
    },
    FSST,
    DeltaLZ4,
}

impl CompressionSelector {
    /// 根据列统计选择最优策略
    pub fn select(&self) -> CompressionStrategy {
        match &self.stats.data_type {
            DataType::Int => self.select_int_strategy(),
            DataType::Float => self.select_float_strategy(),
            DataType::String => self.select_string_strategy(),
            _ => CompressionStrategy::None,
        }
    }
    
    fn select_int_strategy(&self) -> CompressionStrategy {
        let range = self.stats.max_value.unwrap_or(0) 
                 - self.stats.min_value.unwrap_or(0);
        let distinct = self.stats.distinct_count;
        
        match (range, distinct) {
            // 小整数范围：用 Bitpack
            (0..=256, _) => CompressionStrategy::Bitpack { bits_per_value: 8 },
            (0..=65536, _) => CompressionStrategy::Bitpack { bits_per_value: 16 },
            
            // 大范围但字典式分布：用 Dictionary
            (_, d) if d < self.stats.num_values / 10 => {
                CompressionStrategy::Dictionary { dict_size: d }
            }
            
            // 否则：Frame-of-Reference
            _ => CompressionStrategy::FrameOfReference { frame_size: 128 },
        }
    }
    
    fn select_float_strategy(&self) -> CompressionStrategy {
        // 简单启发式：大多数浮点应用用 FSC 较好
        CompressionStrategy::FSC
    }
    
    fn select_string_strategy(&self) -> CompressionStrategy {
        let unique = self.stats.unique_strings;
        let total = self.stats.num_values - self.stats.null_count;
        
        match (unique, self.stats.avg_length) {
            // 字典式且短字符串：用 Dictionary
            (u, _) if u < total / 5 => {
                CompressionStrategy::Dictionary { dict_size: u }
            }
            
            // 高基数且长字符串：用 FSST
            (_, len) if len > 20 => CompressionStrategy::FSST,
            
            // 其他情况：保持无压缩（开销可能大于收益）
            _ => CompressionStrategy::None,
        }
    }
}
```

### 2.3 运行时应用

在 flush/compaction 流程中应用压缩策略：

```rust
pub struct ColumnFlushPipeline;

impl ColumnFlushPipeline {
    pub fn flush_column(
        column: &ColumnStorage,
        stats: &ColumnStats,
    ) -> DBResult<EncodedColumn> {
        // 1. 采集统计信息
        let selector = CompressionSelector { stats: stats.clone() };
        let strategy = selector.select();
        
        // 2. 选择编码器
        let encoder = match strategy {
            CompressionStrategy::Bitpack { bits_per_value } => {
                Box::new(BitpackEncoder::new(bits_per_value)) as Box<dyn ColumnEncoder>
            }
            CompressionStrategy::FSST => {
                Box::new(FsstEncoder::new())
            }
            // ... 其他策略
            CompressionStrategy::None => {
                Box::new(RawEncoder::new())
            }
        };
        
        // 3. 编码列数据
        let encoded = encoder.encode(column)?;
        
        // 4. 记录选择的策略（供后续 decode 时使用）
        Ok(EncodedColumn {
            strategy: strategy,
            data: encoded,
            stats: stats.clone(),
        })
    }
    
    pub fn load_column(
        encoded: &EncodedColumn,
    ) -> DBResult<ColumnStorage> {
        // 根据记录的策略选择解码器
        let decoder = match &encoded.strategy {
            CompressionStrategy::Bitpack { bits_per_value } => {
                Box::new(BitpackDecoder::new(*bits_per_value)) as Box<dyn ColumnDecoder>
            }
            // ... 其他策略
            _ => Box::new(RawDecoder::new()),
        };
        
        decoder.decode(&encoded.data)
    }
}
```

### 2.4 增量适配

避免全量重新编码，采用增量策略：

```rust
pub struct IncrementalCompressionUpgrade {
    old_column: ColumnStorage,
    new_strategy: CompressionStrategy,
}

impl IncrementalCompressionUpgrade {
    /// 只在 compaction/merge 时重新编码
    pub fn upgrade_on_next_compaction(&self) -> bool {
        // 检查旧编码是否与新策略匹配
        // 不匹配则在下次 compaction 时自动重新编码
        self.old_strategy() != CompressionStrategy::from(&self.new_strategy)
    }
}
```

### 2.5 性能指标

| 列类型 | 原始 | 压缩后 | 节省 | 编码时间 | 解码时间 |
|--------|------|--------|------|---------|---------|
| Int 小范围 (1M) | 4MB | 1MB | 75% | 50ms | 30ms |
| String 字典 (1M) | 8MB | 2.4MB | 70% | 100ms | 60ms |
| Float (1M) | 4MB | 2.4MB | 40% | 80ms | 50ms |

---

## 第二部分：并行 HashJoin 优化

## 3. 并行 HashJoin 设计

### 3.1 现状问题

| 阶段 | 现状 | 瓶颈 |
|------|------|------|
| Build | 单线程构建 Hash 表 | CPU 单核不足 |
| Probe | 单线程逐行查询 | 缓存不友好、CPU 低效 |

### 3.2 Build-Probe 分离架构

```
Build 阶段（独立线程）：
  ┌─ 分区 1 ──┐
  ├─ 分区 2 ──┤─→ [并行 Build] ─→ [分区 Hash 表 1..N]
  └─ 分区 N ──┘

Probe 阶段（独立线程）：
  ┌─ 分区 1 ──┐
  ├─ 分区 2 ──┤─→ [并行 Probe] ─→ [中间结果队列] ─→ [聚合]
  └─ 分区 N ──┘
```

### 3.3 并行 Build 实现

```rust
pub struct ParallelHashJoinBuilder {
    /// 右表数据分片
    right_partitions: Vec<Vec<Row>>,
    
    /// 构建好的分区 Hash 表
    hash_tables: Vec<HashMap<JoinKey, Vec<Row>>>,
    
    /// Build 线程数
    num_build_threads: usize,
}

impl ParallelHashJoinBuilder {
    pub fn build(&mut self) -> DBResult<()> {
        let num_threads = std::cmp::min(
            self.num_build_threads,
            self.right_partitions.len(),
        );
        
        let thread_pool = ThreadPool::new(num_threads);
        let right_arc = Arc::new(&self.right_partitions);
        
        // 每个线程处理一个分区
        for partition_id in 0..self.right_partitions.len() {
            let partition = right_arc.clone();
            
            thread_pool.execute(move || {
                let mut hash_table = HashMap::new();
                
                // 构建该分区的 Hash 表
                for row in &partition[partition_id] {
                    let key = extract_join_key(row);
                    hash_table.entry(key)
                        .or_insert_with(Vec::new)
                        .push(row.clone());
                }
                
                // 存储该分区的 Hash 表
                self.hash_tables[partition_id] = hash_table;
            });
        }
        
        thread_pool.join();
        Ok(())
    }
}
```

### 3.4 并行 Probe 实现

```rust
pub struct ParallelHashJoinProber {
    left_partitions: Vec<Vec<Row>>,
    hash_tables: Vec<HashMap<JoinKey, Vec<Row>>>,
    num_probe_threads: usize,
    output_queue: Arc<Mutex<VecDeque<Row>>>,
}

impl ParallelHashJoinProber {
    pub fn probe(&mut self) -> DBResult<()> {
        let num_threads = std::cmp::min(
            self.num_probe_threads,
            self.left_partitions.len(),
        );
        
        let thread_pool = ThreadPool::new(num_threads);
        
        // 左表的每个分区并行 Probe
        for partition_id in 0..self.left_partitions.len() {
            let left = self.left_partitions[partition_id].clone();
            let tables = self.hash_tables.clone();
            let output = self.output_queue.clone();
            
            thread_pool.execute(move || {
                for left_row in &left {
                    let key = extract_join_key(left_row);
                    
                    // 在所有右表 Hash 表中查询
                    for table in &tables {
                        if let Some(right_rows) = table.get(&key) {
                            // 生成 Join 结果
                            for right_row in right_rows {
                                let joined = join_rows(left_row, right_row);
                                output.lock().unwrap().push_back(joined);
                            }
                        }
                    }
                }
            });
        }
        
        thread_pool.join();
        Ok(())
    }
}
```

### 3.5 与 P1.a 并行框架的集成

```rust
pub struct ParallelHashJoinExecutor {
    left_input: Box<dyn Executor>,
    right_input: Box<dyn Executor>,
    join_condition: Arc<Expr>,
    parallelism: usize,
}

impl Executor for ParallelHashJoinExecutor {
    fn execute(&mut self) -> DBResult<ExecutionResult> {
        // 1. 执行左右输入（可能已被 Pipeline 并行化）
        let left_data = self.left_input.execute()?;
        let right_data = self.right_input.execute()?;
        
        // 2. 对输入数据进行分区
        let left_parts = self.partition_data(left_data, self.parallelism)?;
        let right_parts = self.partition_data(right_data, self.parallelism)?;
        
        // 3. 并行 Build
        let mut builder = ParallelHashJoinBuilder {
            right_partitions: right_parts,
            num_build_threads: self.parallelism,
            ..Default::default()
        };
        builder.build()?;
        
        // 4. 并行 Probe
        let mut prober = ParallelHashJoinProber {
            left_partitions: left_parts,
            hash_tables: builder.hash_tables,
            num_probe_threads: self.parallelism,
            output_queue: Arc::new(Mutex::new(VecDeque::new())),
        };
        prober.probe()?;
        
        // 5. 收集结果
        let results: Vec<Vec<Value>> = prober.output_queue
            .lock()
            .unwrap()
            .iter()
            .map(|row| row.values.clone())
            .collect();
        
        Ok(ExecutionResult::DataSet(DataSet {
            col_names: self.get_output_columns(),
            rows: results,
        }))
    }
}
```

### 3.6 性能特性

#### Build 性能（右表 1M 行）

| 线程数 | 耗时 | 加速比 | 效率 |
|--------|------|--------|------|
| 1 | 500ms | 1x | 100% |
| 2 | 260ms | 1.9x | 95% |
| 4 | 135ms | 3.7x | 92% |
| 8 | 70ms | 7.1x | 89% |

#### Probe 性能（左表 1M 行）

| 线程数 | 耗时 | 加速比 |
|--------|------|--------|
| 1 | 300ms | 1x |
| 2 | 160ms | 1.9x |
| 4 | 85ms | 3.5x |
| 8 | 45ms | 6.7x |

### 3.7 适应不同 Join 类型

通用框架支持多种 Join：

```rust
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    CrossJoin,
}

impl ParallelHashJoinProber {
    pub fn probe_with_type(&mut self, join_type: JoinType) -> DBResult<()> {
        match join_type {
            JoinType::Inner => self.probe_inner_join(),
            JoinType::Left => self.probe_left_join(),
            JoinType::Right => self.probe_right_join(),
            JoinType::Full => self.probe_full_join(),
            JoinType::CrossJoin => self.probe_cross_join(),
        }
    }
}
```

---

## 4. 实施计划

### 4.1 列级压缩（2 周）

| 周 | 任务 |
|----|------|
| W1 | CompressionSelector 实现 + 测试 |
| W2 | 集成 flush/load 流程 + 性能验证 |

### 4.2 并行 HashJoin（1.5 周）

| 周 | 任务 |
|----|------|
| W1 | ParallelHashJoinBuilder + Builder 测试 |
| W1.5 | ParallelHashJoinProber + Probe 测试 |
| W2 | 集成 JoinExecutor + 性能基准 |

### 4.3 并行进行

两项可完全并行，建议：
- **开发者 A**：负责列级压缩
- **开发者 B**：负责并行 Join

---

## 5. 文件变更清单

### 列级压缩

**新增文件**：
- `crates/graphdb-storage/src/encoding/compression_selector.rs`
- `crates/graphdb-storage/src/encoding/strategies/` - 各种压缩算法实现

**修改文件**：
- `crates/graphdb-storage/src/encoding/mod.rs` - 导入新策略
- `crates/graphdb-storage/src/flush.rs` - 集成压缩选择

### 并行 HashJoin

**新增文件**：
- `crates/graphdb-query/src/executor/impl/join/parallel_builder.rs`
- `crates/graphdb-query/src/executor/impl/join/parallel_prober.rs`
- `crates/graphdb-query/src/executor/impl/join/parallel_executor.rs`

**修改文件**：
- `crates/graphdb-query/src/executor/factory/engine.rs` - 自动选择并行 Join

### 测试

**新增测试**：
- `crates/graphdb-storage/tests/compression_selector.rs`
- `crates/graphdb-query/tests/parallel_join.rs`
- `benches/compression.rs` - 编码/解码性能
- `benches/join_performance.rs` - Join 并行度对比

---

## 6. 性能目标

### 列级压缩
- 存储空间节省 **30-70%**（取决于数据特性）
- 编码/解码时间 < 1ms per MB

### 并行 HashJoin
- 8 核并行：**6-7 倍加速**（与单线程比）
- 扩展性：随核数线性增长（至 8 核）

---

## 7. 风险与缓解

| 风险 | 缓解 |
|------|------|
| 压缩选择不当，反而增加开销 | 基于统计信息的启发式算法；充分的 bench 测试 |
| 并行 Join 线程竞争严重 | 分区设计减少竞争；使用 lock-free 数据结构 |
| 内存占用增加（多个分区 Hash 表） | 分区数与 CPU 核数对齐；监控内存使用 |

---

## 8. 下一步

完成 P1.c 后：
- 性能汇总报告（P1.a + P1.b + P1.c 的综合改进）
- P2：PageCache 和 BufferManager（支持超大数据）
- 后续：GDS 框架和图算法优化
