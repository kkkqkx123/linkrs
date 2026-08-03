# SIMD 编译优化不友好模式分析与改进任务（SIMD-Friendly Patterns）

- 状态：待实施（独立任务，与 `docs/plan/columnar-optimization-phases-design.md` 的 Phase 1 协同）
- 背景：Phase 0（`-C target-cpu=native`）已验证自动向量化带来 3.46x 收益。本任务分析剩余不友好模式，判定哪些需要改进、改在哪。

## 分析结论速览

| # | 模式 | 位置 | 影响 | 改进 | 归属 |
|---|------|------|------|------|------|
| M1 | Value 枚举双重 match（类型×操作分派） | `value_arithmetic.rs`、`operations.rs` | 高 | 是 | Phase 1（typed 批量运算） |
| M2 | 迭代器链 + 每元素函数调用 + Result 传播 | `chunk.rs` eval_with_cache | 高 | 是 | Phase 1 |
| M3 | 逐行解释执行（per-row context） | `chunk.rs` per_row、`filter_indices` | 中 | 降频 | Phase 1 覆盖 |
| M4 | 哈希聚合 `HashMap<Vec<Value>, …>` | `aggregate.rs` | 中低 | 可选 | Phase 2/3 另议 |
| M5 | 排序比较器 `compare_values` | `helpers/comparison.rs`、`sort.rs` | 低 | 否 | 不进本轮 |
| M6 | gather 型转置/取列（56B 元素） | `chunk.rs` take_indices/get_column | 中 | 顺带 | Phase 1 实现细节 |
| M7 | 存储层属性解码 | graphdb-storage encoding/vertex | 待测 | 否 | Phase 3 评估 |
| M8 | 短路分支与逻辑运算 | T4.1 Filter 短路、eval_and/or | 低 | 否（保持短路） | Phase 1 实现细节 |

关键区分：**迭代器链本身不是问题**（Rust 的 `filter`/`map` 链在简单类型上 LLVM 通常完美向量化）；真正的反模式是**循环体内每元素的 match、函数调用、Result 传播**。

## 模式详析

### M1：Value 枚举双重 match

**证据**：
- `crates/graphdb-core/src/core/value/value_arithmetic.rs`：`add`/`sub`/`mul` 等为 19 变体枚举的嵌套 match，30+ 分支，每分支构造新 `Value`（`Value` 实测 56 字节）
- `crates/graphdb-query/src/query/executor/expression/evaluator/operations.rs:15`：`BinaryOperationEvaluator::evaluate` 先 match 操作符（19 个），再进入上述值 match

**机制**：每元素两层分支 → 分支发散；56B 元素宽度浪费 7x 带宽；`Result` + 分配开销。

**判断**：**需要改进**。这是过滤/投影/算术热路径的核心，与 Phase 0 已验证的 4.7x（typed 列）同源。改进方式即 Phase 1 P1.3：列级一次类型判定后，元素循环为纯 `i64`/`f64` 运算，无分支、无 match，LLVM 可直接向量化。

**修改位置**：
- `crates/graphdb-query/src/query/executor/streaming/chunk.rs`：`eval_with_cache` 增加 typed 快路径
- 新增 typed 求值模块（如 `streaming/expression/typed_eval.rs`），`operations.rs` 保持行式语义路径不变（typed 路径不经过 `value_arithmetic.rs`，后者不做修改）

### M2：迭代器链 + 每元素函数调用 + Result

**证据**：`chunk.rs` eval_with_cache Binary 分支（约 766 行）：
```rust
left_values.into_iter()
    .zip(right_values)
    .map(|(l, r)| BinaryOperationEvaluator::evaluate(&l, op, &r))  // 每元素函数调用+match+Result
    .collect()
```

**机制**：循环体调用含 match 的非内联函数 → LLVM 放弃向量化。错误传播使每个元素都有 Result 分支。

**判断**：**需要改进**。与 M1 同源，一并处理：typed 路径下类型判定后运算不可失败，错误处理移至循环外（列级）。

**修改位置**：同 M1（typed 求值模块）。

### M3：逐行解释执行

**证据**：`chunk.rs:836` `evaluate_expression_per_row`（每行构造 `BorrowedRowContext` + 表达式树解释）；`chunk.rs:535` `filter_indices`（FnMut 每行）。

**机制**：每元素 context 构造 + 动态分发，完全无向量化机会。

**判断**：不做 SIMD 改造，但**降低出现率**——Phase 1 扩大列式快路径覆盖后，该路径仅服务函数/复杂表达式。保留作为 fallback。

### M4：哈希聚合

**证据**：`operators/blocking/aggregate.rs:21` `HashMap<Vec<Value>, Vec<AggregateAccumulator>>`。

**机制**：每行堆分配 group key（`Vec<Value>`）+ 枚举 hash + HashMap 随机访问。哈希 build 阶段受内存延迟主导，SIMD 收益有限。

**判断**：可选改进（定长列 key 无分配 + 哈希计算向量化），但**不进入本轮 SIMD 任务**；Phase 2 选择向量落地后如聚合仍为热点再评估。

### M5：排序比较器

**证据**：`helpers/comparison.rs:17` `compare_values`（NULL 置后 + 类型 match + 兜底 `to_string()` 比较）；`sort.rs:112` `indices.sort_by` 闭包。

**机制**：每比较一次函数调用；排序本质数据依赖（`sort_by` 比较网络），向量化机会小（radix sort 是另一类工作）。

**判断**：不改进。兜底分支 `a.to_string().cmp(&b.to_string())`（混合类型排序）本身是罕见路径。

### M6：gather 型转置/取列

**证据**：`chunk.rs:480` `get_column`（`rows.iter().map(|row| row[slot].clone())`）、`:512` `take_indices`（`mem::take(&mut self.rows[i])`）、`materialize_columns`。

**机制**：随机访问 + 56B 元素拷贝，无法向量化。

**判断**：随 Phase 1 改进——列式化后转为顺序 typed 循环；元素变为 8B 后 AVX2 gather 才可能值得（本轮不做 gather 指令，先用顺序布局收益）。**修改位置**：`chunk.rs` 布局相关方法，属 Phase 1 实现细节。

### M7：存储层属性解码

**证据**：`crates/graphdb-storage/src/storage/encoding/`、`compression.rs`、vertex 属性读取（source 每行 `FlatVertexRecord` 构造，`source_operator/util.rs:222`）。

**机制**：编码/解码分支 + 随机属性访问；是否有 SIMD 解码收益取决于列式编码块布局。

**判断**：待测。Phase 3（存储读取基线）数据出来后再评估；本轮不改。

### M8：短路分支与逻辑运算

**证据**：T4.1 Filter 短路；`operations.rs` `eval_and`/`eval_or`（`left.and(right)` 值级 match）。

**机制**：短路分支反向量化（分支发散），但短路收益在"跳过求值"，属上层优化，与向量化不冲突——**保持短路**。typed 路径下逻辑运算以位掩码实现（`&`/`|` 位运算，向量化友好），为 Phase 1 实现细节。

## 验证手段（验收标准）

1. **基准**：`cargo bench --bench columnar_necessity_bench` 与 `operator_bench` 的 expr/filter 组：真实 DataChunk 路径单列过滤 ≥2.4x（对照 `docs/archive/benches/columnar-necessity-verification.md` 基线 599µs@4096 → ≤250µs）
2. **汇编证据**（反模式是否消除的直接证明）：对 typed 求值模块
   ```
   cargo build --release --target-dir /tmp/linkrs-asm
   objdump -d target/release/libgraphdb_query*.rlib 2>/dev/null | grep -cE "vp(add|sub|mul|and|cmpeq|cmpgt)"
   ```
   向量指令（`vp*`）出现即确认向量化；对比行式路径无 `vp*` 可佐证反模式消除
3. **优化备注**（LLVM 循环向量化报告）：
   `RUSTFLAGS="-C target-cpu=native -C llvm-args=-mllvm -pass-remarks=loop-vectorize"` 编译，确认 typed 循环无 `not vectorized: loop body is not inlinable` / `unsupported instruction` 类备注
4. **回归**：`cargo test --test '*'` 全量通过；clippy 全绿
5. **语义**：typed 路径与行式路径结果逐值一致（随机数据对拍测试）

## 任务边界（独立任务，与 Phase 1 的关系）

- 本任务产出：M1/M2 的 typed 批量求值实现 + M6 布局调整（即 Phase 1 的 P1.2/P1.3 核心），可独立先行，行式路径完全保留
- 不触碰：`value_arithmetic.rs`、比较器、聚合、存储层（均维持现状）
- 明确不做：手工 SIMD 内联汇编 / `std::simd` / `wide` 依赖（Phase 0 结论：编译器自动向量化已足够）
