# StreamingExecutor 设计与实现

> 更新日期：2026-06-27
> 核心：Enum-based pull executor，逐 chunk 处理

---

## 1. StreamingExecutor Enum 定义

### 1.1 完整的 Enum 定义

```rust
pub enum StreamingExecutor {
    // ============ 数据源（无输入） ============
    
    ScanVertices {
        table: Arc<VertexTable>,
        partition_id: usize,
        partition_range: Range<u32>,
        current_offset: u32,
    },
    
    ScanEdges {
        table: Arc<EdgeTable>,
        partition_id: usize,
        partition_range: Range<u32>,
        current_offset: u32,
    },
    
    // ============ 单输入算子 ============
    
    Filter {
        input: Box<StreamingExecutor>,
        condition: Expression,
        opened: bool,
    },
    
    Project {
        input: Box<StreamingExecutor>,
        columns: Vec<ColumnRef>,
        opened: bool,
    },
    
    Limit {
        input: Box<StreamingExecutor>,
        limit: u32,
        consumed: u32,
        opened: bool,
    },
    
    // ============ 有状态算子 ============
    
    Aggregate {
        input: Box<StreamingExecutor>,
        group_by: Vec<ColumnRef>,
        agg_funcs: Vec<AggregateFunc>,
        groups: HashMap<Vec<Value>, Vec<Value>>, // 聚合状态
        group_iter: Option<std::collections::hash_map::IntoIter<Vec<Value>, Vec<Value>>>,
        opened: bool,
    },
    
    // ============ 双输入算子 ============
    
    HashJoin {
        left: Box<StreamingExecutor>,
        right: Box<StreamingExecutor>,
        condition: Expression,
        build_side_tuples: Vec<DataRow>, // 右表已加载
        left_consumed: bool,
        right_consumed: bool,
        build_idx: usize,
        opened: bool,
    },
    
    // ============ 其他 ============
    
    Sort {
        input: Box<StreamingExecutor>,
        order_by: Vec<(ColumnRef, SortOrder)>,
        all_rows: Vec<DataRow>,
        row_iter: Option<std::vec::IntoIter<DataRow>>,
        opened: bool,
    },
}

impl StreamingExecutor {
    /// 初始化
    pub fn open(&mut self) -> DBResult<()> {
        match self {
            Self::ScanVertices { .. } => {
                // 初始化扫描
                Ok(())
            }
            Self::Filter { input, opened, .. } => {
                input.open()?;
                *opened = true;
                Ok(())
            }
            Self::Aggregate { input, groups, opened, .. } => {
                input.open()?;
                groups.clear(); // 重置聚合状态
                *opened = true;
                Ok(())
            }
            // ... 其他 executor
        }
    }
    
    /// 拉取下一个 chunk
    pub fn next(&mut self) -> DBResult<Option<DataChunk>> {
        match self {
            Self::ScanVertices { table, partition_range, current_offset, .. } => {
                // 按分区范围扫描，每次返回最多 1024 行
                let chunk_size = 1024;
                let end = (*current_offset + chunk_size).min(partition_range.end);
                
                if *current_offset >= partition_range.end {
                    return Ok(None);
                }
                
                let rows = table.get_rows(*current_offset..end)?;
                *current_offset = end;
                
                Ok(Some(DataChunk::from_rows(rows)))
            }
            
            Self::Filter { input, condition, .. } => {
                // 拉取上游 chunk，过滤，返回
                if let Some(chunk) = input.next()? {
                    let filtered_rows: Vec<_> = chunk
                        .rows
                        .into_iter()
                        .filter(|row| condition.evaluate(row).unwrap_or(false))
                        .collect();
                    
                    if filtered_rows.is_empty() {
                        // 没有行通过过滤，继续拉取下一个 chunk
                        self.next()
                    } else {
                        Ok(Some(DataChunk::from_rows(filtered_rows)))
                    }
                } else {
                    Ok(None)
                }
            }
            
            Self::Limit { input, limit, consumed, .. } => {
                // 拉取上游数据，直到达到 limit
                if *consumed >= *limit {
                    return Ok(None);
                }
                
                if let Some(mut chunk) = input.next()? {
                    let remaining = *limit - *consumed;
                    
                    // 如果 chunk 超过剩余配额，截断
                    if chunk.rows.len() > remaining as usize {
                        chunk.rows.truncate(remaining as usize);
                    }
                    
                    *consumed += chunk.rows.len() as u32;
                    Ok(Some(chunk))
                } else {
                    Ok(None)
                }
            }
            
            Self::Aggregate { input, group_by, agg_funcs, groups, group_iter, .. } => {
                // 第一次调用时，消费所有输入，构建聚合状态
                if group_iter.is_none() {
                    while let Some(chunk) = input.next()? {
                        for row in chunk.rows {
                            let key = group_by
                                .iter()
                                .map(|col| row.get_value(col))
                                .collect::<Vec<_>>();
                            
                            groups
                                .entry(key)
                                .or_insert_with(Vec::new)
                                .push(row);
                        }
                    }
                    
                    // 准备迭代
                    *group_iter = Some(groups.clone().into_iter());
                }
                
                // 返回下一个聚合组的结果
                if let Some(ref mut iter) = group_iter {
                    if let Some((key, rows)) = iter.next() {
                        let agg_result = self.compute_aggregates(&key, &rows, agg_funcs)?;
                        Ok(Some(DataChunk::from_rows(vec![agg_result])))
                    } else {
                        Ok(None)
                    }
                } else {
                    Ok(None)
                }
            }
            
            // ... 其他 executor 的 next 实现
        }
    }
    
    /// 停止执行（用于 LIMIT）
    pub fn stop(&mut self) -> DBResult<()> {
        match self {
            Self::Filter { input, .. } => input.stop(),
            Self::Limit { input, .. } => input.stop(),
            Self::Aggregate { input, .. } => input.stop(),
            // ... 递归停止
            _ => Ok(()),
        }
    }
    
    /// 清理资源
    pub fn close(&mut self) -> DBResult<()> {
        match self {
            Self::Filter { input, opened, .. } => {
                if *opened {
                    input.close()?;
                    *opened = false;
                }
                Ok(())
            }
            Self::Aggregate { input, groups, opened, .. } => {
                if *opened {
                    input.close()?;
                    groups.clear();
                    *opened = false;
                }
                Ok(())
            }
            // ... 其他 executor
            _ => Ok(()),
        }
    }
}
```

---

## 2. DataChunk 定义

```rust
/// 数据块：流式执行的基本单位
pub struct DataChunk {
    pub rows: Vec<DataRow>,
    pub schema: Arc<Schema>,
}

impl DataChunk {
    pub fn new(rows: Vec<DataRow>, schema: Arc<Schema>) -> Self {
        Self { rows, schema }
    }
    
    pub fn from_rows(rows: Vec<DataRow>) -> Self {
        // 从行推断 schema
        let schema = if rows.is_empty() {
            Arc::new(Schema::empty())
        } else {
            Arc::new(Schema::from_row(&rows[0]))
        };
        Self { rows, schema }
    }
    
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}
```

---

## 3. Executor 迁移模式

### 3.1 无状态单输入（Filter、Project）

**模式**：
```rust
// 在 StreamingExecutor::next() 中
Self::Filter { input, condition, .. } => {
    if let Some(chunk) = input.next()? {
        // 对 chunk 的每一行应用过滤
        let filtered = chunk.rows
            .into_iter()
            .filter(|row| condition.evaluate(row).unwrap())
            .collect();
        Ok(Some(DataChunk::from_rows(filtered)))
    } else {
        Ok(None)
    }
}
```

**特点**：
- 不维护内部状态
- 每个 chunk 独立处理
- 可递归调用 input.next()

### 3.2 有状态单输入（Aggregate）

**模式**：
```rust
Self::Aggregate { input, groups, group_iter, .. } => {
    // 第一次调用时消费所有输入
    if group_iter.is_none() {
        while let Some(chunk) = input.next()? {
            // 累积状态
            for row in chunk.rows {
                let key = extract_group_key(&row);
                groups.entry(key).or_default().push(row);
            }
        }
        // 准备迭代聚合结果
        *group_iter = Some(groups.clone().into_iter());
    }
    
    // 后续调用返回聚合结果
    if let Some(ref mut iter) = group_iter {
        iter.next().map(|(k, v)| compute_aggregate(&k, &v))
    } else {
        None
    }
}
```

**特点**：
- 第一次调用消费所有输入
- 维护内部聚合状态
- 后续调用返回已缓存的结果
- 适用于 Aggregate、Sort、Distinct

### 3.3 二元输入（HashJoin）

**模式**：
```rust
Self::HashJoin { 
    left, right, condition, 
    build_side_tuples, 
    left_consumed, 
    .. 
} => {
    // 第一次：构建阶段（从 build side 加载）
    if !*left_consumed {
        while let Some(chunk) = right.next()? {
            build_side_tuples.extend(chunk.rows);
        }
        *left_consumed = true;
    }
    
    // 探测阶段（逐行与 build side 连接）
    if let Some(chunk) = left.next()? {
        let joined_rows: Vec<_> = chunk.rows
            .iter()
            .flat_map(|left_row| {
                build_side_tuples
                    .iter()
                    .filter(|right_row| {
                        condition.evaluate_pair(left_row, right_row).unwrap()
                    })
                    .map(|right_row| {
                        DataRow::join(left_row, right_row)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
        
        Ok(Some(DataChunk::from_rows(joined_rows)))
    } else {
        Ok(None)
    }
}
```

**特点**：
- 一侧构建哈希表（build side，通常是右表）
- 另一侧探测（probe side，通常是左表）
- Build side 必须被完全加载
- Probe side 可以流式处理

---

## 4. ExecutionMode 支持

### 4.1 PlanExecutor 的改造

```rust
pub struct PlanExecutor<S: StorageClient> {
    factory: ExecutorFactory<S>,
}

impl<S> PlanExecutor<S> {
    pub fn execute_plan(
        &mut self,
        plan: &ExecutionPlan,
        mode: ExecutionMode,
    ) -> DBResult<ExecutionResult> {
        match mode {
            ExecutionMode::Materialized => {
                // 现有逻辑：使用 ExecutorEnum 和 Push 执行
                self.execute_materialized(plan)
            }
            ExecutionMode::Streaming => {
                // 新逻辑：使用 StreamingExecutor 和 Pull 执行
                self.execute_streaming(plan)
            }
        }
    }
    
    fn execute_streaming(
        &mut self,
        plan: &ExecutionPlan,
    ) -> DBResult<ExecutionResult> {
        // 1. 构建 StreamingExecutor 树
        let mut root = self.build_streaming_tree(plan)?;
        
        // 2. Pull 循环
        root.open()?;
        let mut rows = Vec::new();
        while let Some(chunk) = root.next()? {
            rows.extend(chunk.rows);
        }
        root.close()?;
        
        Ok(ExecutionResult::DataSet(DataSet {
            col_names: plan.col_names().clone(),
            rows,
        }))
    }
    
    fn build_streaming_tree(
        &self,
        plan: &ExecutionPlan,
    ) -> DBResult<StreamingExecutor> {
        // 根据执行计划 DAG 递归构建 executor 树
        match plan.op_type() {
            OpType::Scan(table) => {
                Ok(StreamingExecutor::ScanVertices {
                    table: self.storage.get_vertex_table(table)?,
                    partition_id: 0,
                    partition_range: 0..u32::MAX,
                    current_offset: 0,
                })
            }
            OpType::Filter(condition) => {
                let input = Box::new(self.build_streaming_tree(plan.child(0)?)?);
                Ok(StreamingExecutor::Filter {
                    input,
                    condition: condition.clone(),
                    opened: false,
                })
            }
            // ... 其他操作符
        }
    }
}
```

---

## 5. 错误处理与清理

### 5.1 资源管理

```rust
pub struct StreamingExecutorGuard(StreamingExecutor);

impl Drop for StreamingExecutorGuard {
    fn drop(&mut self) {
        // 自动清理
        let _ = self.0.close();
    }
}

// 使用示例
let mut executor = StreamingExecutorGuard(root);
executor.0.open()?;
while let Some(chunk) = executor.0.next()? {
    process(chunk)?;
}
// 离开作用域时自动 close
```

### 5.2 中途停止（LIMIT）

```rust
impl StreamingExecutor {
    pub fn stop(&mut self) -> DBResult<()> {
        // 通知下游停止执行
        match self {
            Self::Filter { input, .. } => input.stop(),
            Self::Limit { input, .. } => input.stop(),
            Self::Aggregate { input, .. } => input.stop(),
            // ... 其他有下游的 executor
            _ => Ok(()),
        }
    }
}

// 使用示例
let mut root = StreamingExecutor::Limit { limit: 10, ... };
root.open()?;
while let Some(chunk) = root.next()? {
    if condition {
        root.stop()?; // 立即停止，下游不再扫描
        break;
    }
}
root.close()?;
```

---

## 6. 与现有 Executor 的对比

| 方面 | 旧 Executor | StreamingExecutor |
|------|-----------|-------------------|
| **调用方式** | `execute() -> DataSet` | `next() -> Option<DataChunk>` |
| **执行时机** | 立即执行子树 | 消费者驱动 |
| **内存占用** | 全表 | 固定大小（~4MB/chunk） |
| **LIMIT 支持** | 浪费（全表扫描） | 高效（提前停止） |
| **实现位置** | ExecutorEnum | StreamingExecutor enum |

---

## 7. 实现清单

### 阶段 0 必须完成

- [ ] StreamingExecutor enum 定义
- [ ] DataChunk 数据结构
- [ ] StreamingExecutor::open/next/close/stop 基础实现
- [ ] ExecutionMode 枚举
- [ ] PlanExecutor 的 execute_streaming 方法
- [ ] 基准测试框架

### 验收标准

- [ ] 编译通过，无 warnings
- [ ] 单元测试覆盖 50%+
- [ ] 架构文档完整
- [ ] 可以为 Scan 编写流式版本

---

## 总结

- ✅ **Enum 设计**：清晰、类型安全、易于维护
- ✅ **Pull 模型**：自然支持流式执行和中途停止
- ✅ **灵活的迁移模式**：无状态、有状态、二元输入各有对应
- ✅ **资源管理**：open/close/stop 完整的生命周期

