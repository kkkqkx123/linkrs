# Path 与 NPath 使用场景指南

## 概述

本项目采用**双路径结构**设计，结合 `Path` 和 `NPath` 的优势，在不同场景下选择最合适的路径表示方式。

- **Path**: 传统的线性路径结构，适合序列化和输出
- **NPath**: 链表结构，使用 `Arc` 共享前缀，适合遍历计算

## 数据结构对比

| 特性 | Path | NPath |
|------|------|-------|
| 结构 | `Vec<Step>` | 链表节点（`Arc` 连接） |
| 扩展操作 | O(n) - 需要复制 | O(1) - 新建节点 |
| 内存占用 | 独立存储每条路径 | 共享前缀，节省内存 |
| 序列化 | ✅ 原生支持 | ❌ 需要转换 |
| 遍历访问 | 随机访问 O(1) | 顺序访问 O(n) |
| 适用场景 | 输出、存储 | 遍历计算 |

## 使用场景决策树

```
是否需要序列化/网络传输？
├── 是 → 使用 Path
└── 否 → 是否频繁扩展路径？
    ├── 是 → 使用 NPath
    └── 否 → 使用 Path（简单场景）
```

## 具体使用场景

### 1. 必须使用 Path 的场景

#### 序列化与存储
```rust
// 存储到磁盘或网络传输
#[derive(Serialize, Deserialize, Encode, Decode)]
pub struct Path {
    pub src: Box<Vertex>,
    pub steps: Vec<Step>,
}
```

#### 最终查询结果输出
```rust
// 返回给客户端的结果
ExecutionResult::Values(vec![Value::Path(path)])
```

#### 执行上下文缓存
```rust
// ExecutionContext.current_path - 使用频率低，保持 Path
pub struct ExecutionContext {
    pub current_path: Option<Path>,  // 保持使用 Path
}
```

### 2. 必须使用 NPath 的场景

#### 图遍历中间计算
```rust
// AllPathsExecutor - 使用 NPath 存储队列
left_queue: VecDeque<(Value, Arc<NPath>)>,
right_queue: VecDeque<(Value, Arc<NPath>)>,
```

#### 双向 BFS 路径拼接
```rust
// ShortestPathExecutor - 双向 BFS 使用 NPath
pub struct BidirectionalBFSState {
    pub left_queue: VecDeque<(Value, Arc<NPath>)>,
    pub right_queue: VecDeque<(Value, Arc<NPath>)>,
}
```

#### DFS 深度探索
```rust
// TraverseExecutor - 使用 NPath 避免路径复制
current_npaths: Vec<Arc<NPath>>,
completed_npaths: Vec<Arc<NPath>>,
```

#### 递归路径扩展
```rust
// ExpandAllExecutor - 递归扩展使用 NPath
fn expand_paths_recursive(
    &mut self,
    current_npath: &Arc<NPath>,
    current_depth: usize,
    max_depth: usize,
) -> Result<Vec<Arc<NPath>>, QueryError>
```

## 转换方法

### Path → NPath（计算前）
```rust
use crate::core::NPath;
use std::sync::Arc;

// 方法1: 从 Path 创建 NPath
let npath = NPath::from_path(&path);

// 方法2: 手动构建
let start_vertex = Arc::new(vertex);
let npath = Arc::new(NPath::new(start_vertex));
let extended = Arc::new(NPath::extend(npath, edge, next_vertex));
```

### NPath → Path（输出前）
```rust
// 单个转换
let path = npath.to_path();

// 批量转换
let paths: Vec<Path> = npaths.iter().map(|np| np.to_path()).collect();

// 并行转换（大量数据时使用）
use rayon::prelude::*;
let paths: Vec<Path> = npaths.par_iter().map(|np| np.to_path()).collect();
```

## 执行器更新状态

| 执行器 | 状态 | 说明 |
|--------|------|------|
| AllPathsExecutor | ✅ 已更新 | 使用 NPath 存储队列和缓存 |
| ShortestPathExecutor | ✅ 已更新 | 双向 BFS 使用 NPath |
| TraverseExecutor | ✅ 已更新 | 使用 NPath 进行遍历计算 |
| ExpandAllExecutor | ✅ 已更新 | 递归扩展使用 NPath |

## 最佳实践

### 1. 遍历算法中使用 NPath
```rust
// 好的做法：使用 NPath 进行遍历
fn traverse(&mut self) {
    for npath in &self.current_npaths {
        let current_node = &npath.vertex().vid;
        let neighbors = self.get_neighbors(current_node)?;
        
        for (neighbor_id, edge) in neighbors {
            // O(1) 扩展
            let new_npath = Arc::new(NPath::extend(
                npath.clone(),
                Arc::new(edge),
                Arc::new(vertex),
            ));
            next_npaths.push(new_npath);
        }
    }
}
```

### 2. 输出时转换为 Path
```rust
// 好的做法：只在输出时转换
fn build_result(&self) -> ExecutionResult {
    // 批量转换为 Path
    let paths: Vec<Path> = self.npaths.iter()
        .map(|np| np.to_path())
        .collect();
    
    // 构建结果
    ExecutionResult::Values(
        paths.into_iter()
            .map(|p| Value::Path(p))
            .collect()
    )
}
```

### 3. 避免频繁转换
```rust
// 不好的做法：频繁来回转换
for npath in &npaths {
    let path = npath.to_path();  // ❌ 每次循环都转换
    // 处理...
    let npath2 = NPath::from_path(&path);  // ❌ 又转回去
}

// 好的做法：保持 NPath 直到最后
let paths: Vec<Path> = npaths.iter()
    .map(|np| np.to_path())  // ✅ 只在最后转换一次
    .collect();
```

## 性能考量

### 内存使用
- **Path**: 每条路径独立存储，内存占用 = 路径数 × 平均路径长度
- **NPath**: 共享前缀，内存占用 ≈ 唯一节点数 × 节点大小

### 时间复杂度
- **Path 扩展**: O(n) - 需要复制整个 Vec
- **NPath 扩展**: O(1) - 只需创建新节点

### 适用数据规模
- **小规模遍历**（<1000 条路径）: Path 和 NPath 差异不大
- **大规模遍历**（>10000 条路径）: NPath 显著节省内存和时间

## 总结

```
输入/存储: Path (序列化友好)
    ↓
转换: Path → NPath (使用 from_path)
    ↓
遍历计算: NPath (内存高效，O(1)扩展)
    ↓
输出/存储: NPath → Path (使用 to_path)
```

这种分层架构既保持了与现有系统的兼容性，又在性能关键路径上获得了 NPath 的内存效率优势。
