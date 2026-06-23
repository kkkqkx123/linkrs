# VertexId 设计方案分析

## 一、业界实践调研

### 1.1 RocksDB 键设计

RocksDB 的核心设计原则：

- **键和值都是纯字节流 (byte streams)**
- 没有键或值大小的限制
- 使用比较器 (Comparator) 定义键的排序顺序
- 键的版本控制通过在键末尾存储版本号实现

```cpp
// RocksDB 核心 API
Get(key)    // key: byte stream
Put(key, val)   // key, val: byte streams
Delete(key)     // key: byte stream
```

**关键洞察**：RocksDB 完全不关心键的语义，只关心字节序比较。这提供了最大的灵活性。

### 1.2 NebulaGraph VID 演进

| 版本 | VID 类型        | 存储格式                         | 说明                     |
| ---- | --------------- | -------------------------------- | ------------------------ |
| 1.0  | 仅 int64        | 8 字节固定长度                   | 简单高效，但限制用户     |
| 2.0+ | int64 或 string | int64 直接存储，string 使用 hash | 支持字符串，但增加复杂度 |

NebulaGraph 2.0 字符串 VID 处理：

- 使用 `hash()` 函数将字符串映射到分区
- 存储时保留原始字符串
- 查询时需要完整匹配

### 1.3 其他图数据库

| 数据库          | VID 类型         | 设计理念               |
| --------------- | ---------------- | ---------------------- |
| Neo4j           | 内部生成 long ID | 用户不可指定，简化设计 |
| TigerGraph      | int64 或 string  | 类似 NebulaGraph       |
| Oxigraph (Rust) | IRI/BlankNode    | 语义网标准，字符串形式 |

---

## 二、当前问题分析

### 2.1 现状

```rust
// src/core/types/storage_ids.rs
pub type VertexId = u64;  // 仅支持整数

// src/core/vertex_edge_path.rs
pub struct Vertex {
    pub vid: Box<Value>,  // 任意 Value 类型
    // ...
}

// src/storage/vertex/mod.rs
pub struct VertexRecord {
    pub vid: VertexId,  // u64
    // ...
}
```

**问题**：

1. `VertexId = u64` 无法表示字符串 ID
2. `Vertex.vid: Box<Value>` 与 `VertexRecord.vid: u64` 类型不匹配
3. 类型转换时信息丢失

### 2.2 是否需要支持多种 ID 类型？

**用户场景分析**：

| 场景         | 推荐 VID 类型 | 原因                             |
| ------------ | ------------- | -------------------------------- |
| 社交网络用户 | string        | 使用用户名/邮箱作为 ID，语义清晰 |
| 知识图谱实体 | string        | 使用 URI/IRI，符合标准           |
| 时序数据     | int64         | 时间戳作为 ID，范围查询高效      |
| 内部系统     | int64         | 自动生成，性能优先               |

**结论**：**必须支持字符串 ID**，这是图数据库的常见需求。

---

## 三、设计方案对比

### 方案 A：枚举类型

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum VertexId {
    Int64(i64),
    String(String),
}
```

| 优点               | 缺点             |
| ------------------ | ---------------- |
| 类型安全           | 需要处理两种情况 |
| 无需序列化即可使用 | 内存布局不紧凑   |
| Rust 模式匹配友好  | 存储时仍需编码   |

### 方案 B：统一字节串（推荐）

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VertexId(Vec<u8>);

impl VertexId {
    // 构造方法
    pub fn from_int64(id: i64) -> Self {
        VertexId(id.to_be_bytes().to_vec())
    }

    pub fn from_string(s: impl Into<String>) -> Self {
        VertexId(s.into().into_bytes())
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        VertexId(bytes)
    }

    // 解析方法
    pub fn as_int64(&self) -> Option<i64> {
        if self.0.len() == 8 {
            let arr: [u8; 8] = self.0[..].try_into().ok()?;
            Some(i64::from_be_bytes(arr))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    // 类型判断
    pub fn is_int64(&self) -> bool {
        self.0.len() == 8
    }

    pub fn is_string(&self) -> bool {
        self.as_str().is_some()
    }
}
```

| 优点                        | 缺点               |
| --------------------------- | ------------------ |
| **统一表示，无分支逻辑**    | 解析时需要判断类型 |
| **直接对接 RocksDB 等存储** | 字符串构造时有拷贝 |
| **比较高效（字节序）**      | -                  |
| **内存布局紧凑**            | -                  |
| **支持任意类型扩展**        | -                  |

### 方案 C：仅 Int64 + Hash

```rust
pub type VertexId = i64;

// 字符串 ID 通过 hash 映射
pub fn hash_to_vid(s: &str) -> VertexId {
    // hash function
}
```

| 优点     | 缺点               |
| -------- | ------------------ |
| 最简单   | **hash 碰撞风险**  |
| 比较最快 | **丢失原始字符串** |
| 内存最小 | 无法显示原始 ID    |

---

## 四、推荐方案：统一字节串

### 4.1 理由

1. **与存储引擎一致**：RocksDB、LevelDB 等都使用字节串作为键
2. **统一无分支**：内部逻辑无需处理不同类型
3. **扩展性强**：未来可支持 UUID、复合键等
4. **性能可控**：字节序比较高效

### 4.2 实现细节

```rust
// src/core/types/storage_ids.rs

use serde::{Deserialize, Serialize};
use std::fmt;

/// Vertex identifier - unified byte representation
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VertexId(Vec<u8>);

impl VertexId {
    pub const fn new() -> Self {
        VertexId(Vec::new())
    }

    pub fn from_int64(id: i64) -> Self {
        VertexId(id.to_be_bytes().to_vec())
    }

    pub fn from_u64(id: u64) -> Self {
        VertexId(id.to_be_bytes().to_vec())
    }

    pub fn from_string(s: impl Into<String>) -> Self {
        VertexId(s.into().into_bytes())
    }

    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        VertexId(bytes)
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn as_int64(&self) -> Option<i64> {
        if self.0.len() == 8 {
            let arr: [u8; 8] = self.0[..].try_into().ok()?;
            Some(i64::from_be_bytes(arr))
        } else {
            None
        }
    }

    pub fn as_u64(&self) -> Option<u64> {
        if self.0.len() == 8 {
            let arr: [u8; 8] = self.0[..].try_into().ok()?;
            Some(u64::from_be_bytes(arr))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        std::str::from_utf8(&self.0).ok()
    }

    pub fn is_int64(&self) -> bool {
        self.0.len() == 8
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl fmt::Display for VertexId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(i) = self.as_int64() {
            write!(f, "{}", i)
        } else if let Some(s) = self.as_str() {
            write!(f, "\"{}\"", s)
        } else {
            write!(f, "{:?}", self.0)
        }
    }
}

impl Default for VertexId {
    fn default() -> Self {
        Self::new()
    }
}

impl From<i64> for VertexId {
    fn from(id: i64) -> Self {
        Self::from_int64(id)
    }
}

impl From<u64> for VertexId {
    fn from(id: u64) -> Self {
        Self::from_u64(id)
    }
}

impl From<String> for VertexId {
    fn from(s: String) -> Self {
        Self::from_string(s)
    }
}

impl From<&str> for VertexId {
    fn from(s: &str) -> Self {
        Self::from_string(s)
    }
}

impl Ord for VertexId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.cmp(&other.0)
    }
}

impl PartialOrd for VertexId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}
```

### 4.3 迁移影响

| 文件                    | 修改内容                                   |
| ----------------------- | ------------------------------------------ |
| `storage_ids.rs`        | VertexId 从 type alias 改为 struct         |
| `vertex_edge_path.rs`   | Vertex.vid 从 Box<Value> 改为 VertexId     |
| `storage/vertex/mod.rs` | VertexRecord.vid 类型不变（已是 VertexId） |
| `storage/edge/mod.rs`   | EdgeRecord 添加 ranking 字段               |
| `type_utils.rs`         | 更新转换逻辑                               |

---

## 五、EdgeRecord ranking 字段

当前 EdgeRecord 缺少 ranking 字段，需要添加：

```rust
// src/storage/edge/mod.rs
pub struct EdgeRecord {
    pub edge_id: EdgeId,
    pub src_vid: VertexId,
    pub dst_vid: VertexId,
    pub ranking: i64,  // 新增：支持同一对顶点间的多条边
    pub properties: Vec<(String, Value)>,
}
```

---

## 六、总结

| 决策               | 选择          | 理由                       |
| ------------------ | ------------- | -------------------------- |
| VertexId 类型      | 统一字节串    | 与存储引擎一致，无分支逻辑 |
| EdgeRecord.ranking | 添加 i64 字段 | 支持多边场景               |
| 向后兼容           | 不考虑        | 强制迁移，编译时报错       |

**下一步**：按此方案修改代码，不创建新文件，直接修改现有实现。
