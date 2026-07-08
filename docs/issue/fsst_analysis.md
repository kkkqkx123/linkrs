# FSST 实现问题分析

> 分析日期: 2026-06-18
> 范围: `crates/graphdb-storage/src/storage/encoding/fsst.rs`

---

## 1. 逻辑问题

### 1.1 Bug: decode 中的 `code == 0` 跳过导致数据损坏

**位置**: `fsst.rs:192-194`

```rust
for &code in encoded {
    if code == 0 { continue; }   // ← BUG
```

**问题描述**:

- 编码端 (`encode`, line 177): 字节不匹配任何符号时，通过 `result.push(bytes[i])` 原样输出。如果原始字符串的某个字节是 `0x00`，它被直接写入编码结果。
- 解码端 (`decode`, line 193): `code == 0` 被无条件跳过，导致原始字符串中的 `0x00` 字节在解码后丢失。

**影响**: 虽然标准 UTF-8 字符串中间通常不含 `0x00`（仅在结尾做 C 风格终结符），但：

- Rust `String` 在语义上允许包含 `0x00` 字节
- 如果未来支持二进制数据（blob），此问题会直接导致静默数据损坏

**修复方案**: 移除 `if code == 0 { continue; }` 判断。解码逻辑应当能区分"符号代码 0 不存在"和"字面量字节 0x00"两种情况。由于符号代码从 `1_u8` 开始分配，`code == 0` 只可能来自字面量的 `0x00`，所以直接进入 `else` 分支（`result.push(code)`）即可。

### 1.2 `FsstColumn::set()` 对越界写入静默忽略

**位置**: `fsst.rs:254-256`

```rust
pub fn set(&mut self, row_idx: usize, value: Option<&str>) {
    if row_idx >= self.encoded_data.len() {
        return;  // ← 静默丢弃，不报错
    }
```

**问题描述**:

- 当 `row_idx` 超出当前 `encoded_data` 长度时，函数直接返回，不执行任何写入，也不报告错误
- 这意味着 FSST 编码后的列**无法追加新行**，只能修改已有行

**影响**:

- `Column::set()` 在 encoding 激活时会调用 `self.encoding.set(row_idx, value)`
- 如果新行的索引超过编码列长度，值被静默丢弃，导致数据丢失
- `sync_row_count_from_encoding()` 的行数同步机制与此问题交织，行为不可预测

**修复方案**: 将返回值改为 `Result<()>`，对越界写入返回 `StorageError`，或者在 `encoded_data` 不足时自动填充。

---

## 2. 性能瓶颈

### 2.1 ngram 训练时大量堆分配

**位置**: `fsst.rs:121-126`

```rust
for len in MIN_SYMBOL_LEN..=MAX_SYMBOL_LEN.min(bytes.len()) {
    for i in 0..=bytes.len() - len {
        let ngram: Vec<u8> = bytes[i..i + len].to_vec();  // 每次分配
```

**问题描述**:

- 每个 ngram 都通过 `to_vec()` 创建新的堆分配
- 对长度 L 的字符串，提取 2..=8 的 ngram 约产生 7×L 次分配
- 虽然 `MAX_NGRAMS_PER_STRING=1000` 限制了总量，但无法缓解分配开销

**影响**: 训练大型数据集时，分配压力大，CPU 时间主要花在分配/释放上。

**优化方案**:

- 使用 `(&[u8], usize)` 作为临时 ngram 表示，仅在插入 `HashMap` 时做 `to_vec()`
- 或者使用 `bytes(x..y)` 类型的零拷贝切片结构

### 2.2 `Vec<Vec<u8>>` 的存储格式

**位置**: `fsst.rs:233`

```rust
pub encoded_data: Vec<Vec<u8>>,
```

**问题描述**:

- 每行编码数据独立堆分配（每个 `Vec<u8>` 含 24 字节指针元数据）
- 数据在堆上分散存储，缓存局部性差
- 扫描所有行时需要解引用多个指针

**影响**: 对于大量行（如百万级），内存开销和访问延迟显著。

**优化方案**: 改用 flat buffer 格式：

```rust
pub encoded_data: Vec<u8>,     // 所有编码数据的连续存储
pub offsets: Vec<u32>,          // 每行起始偏移
```

### 2.3 decode 容量预估不足

**位置**: `fsst.rs:190`

```rust
Vec::with_capacity(encoded.len() * 2)
```

**问题描述**:

- 假设解码后最多膨胀到 2 倍
- 但最大符号长度为 8（`MAX_SYMBOL_LEN`），最坏情况膨胀到 8 倍
- 低估容量导致多次 reallocation

**影响**: 解码短字符串时影响不大，但解码包含多个长符号序列的数据时产生不必要的分配。

**优化方案**: 使用 `encoded.len() * MAX_SYMBOL_LEN`（即 8 倍）预估容量，或根据符号表的平均膨胀率动态估算。

### 2.4 encode 容量总是分配原始长度

**位置**: `fsst.rs:159`

```rust
let mut result = Vec::with_capacity(bytes.len());
```

**问题描述**:

- 如果没有任何符号匹配，输出长度等于输入长度——容量刚好够，无需 realloc
- 如果有符号匹配，输出会小于输入——容量过剩，但无 realloc

**影响**: 轻微的内存浪费，对性能影响有限。

**优化方案**: 容量估算合理，无需修改。

### 2.5 训练 score 函数过于简单

**位置**: `fsst.rs:136-140`

```rust
ngrams.sort_by(|a, b| {
    let score_a = a.1 * a.0.len();
    let score_b = b.1 * b.0.len();
    score_b.cmp(&score_a)
});
```

**问题描述**:

- 使用 `frequency × length` 作为分数，倾向于选择长 ngram
- 原始 FSST 论文使用更复杂的增益模型，考虑替换后实际节省的字节数
- 长 ngram 可能仅出现少数几次，而短 ngram 可能被更频繁地使用，当前公式可能选出次优符号

**影响**: 压缩率可能低于理论最优值，但不会产生正确性问题。

**优化方案**: 使用 `(frequency - 1) × (length - 1)` 或基于节省字节数的增益模型。

### 2.6 训练时 ngram 提取顺序偏置

**位置**: `fsst.rs:120-132`

```rust
for len in MIN_SYMBOL_LEN..=MAX_SYMBOL_LEN.min(bytes.len()) {
    for i in 0..=bytes.len() - len {
```

**问题描述**:

- 总是先提取短 ngram（len=2），后提取长 ngram（len=8）
- 由于 `MAX_NGRAMS_PER_STRING` 限制是全局的，长字符串的末尾部分可能因为计数耗尽而完全不被采样
- 对于长短不一的字符串集合，采样不公平

**影响**: 训练质量受字符串遍历顺序影响。

**优化方案**: 改用随机采样 ngram，或多轮迭代确保不同长度 ngram 的覆盖。

---

## 3. 设计合理性评价

| 方面 | 评价 |
|------|------|
| 纯 Rust 自实现 | 无外部依赖，可控性好 |
| 解码速度 O(n) | 查表解码，非常快 ✓ |
| 自动选择策略 | `avg_length >= 20 && cardinality_ratio > 0.5` 合理 ✓ |
| 训练采样 | `MAX_TRAINING_SAMPLES=10000` 避免 OOM ✓ |
| 编码算法 | 贪心最长匹配，简单有效 ✓ |
| 符号表不持久化 | 每次 reload 重新训练，浪费 CPU（notable tradeoff） |

---

## 4. 建议优先级

| 优先级 | 问题 | 影响 |
|--------|------|------|
| **P0** | decode 零字节 bug (1.1) | 数据正确性 |
| **P1** | `set()` 静默越界 (1.2) | 数据正确性 |
| **P2** | flat buffer 存储格式 (2.2) | 内存效率 |
| **P2** | 训练 ngram 堆分配 (2.1) | 训练性能 |
| **P3** | decode 容量预估 (2.3) | 解码性能 |
| **P3** | score 函数 (2.5) | 压缩率 |
