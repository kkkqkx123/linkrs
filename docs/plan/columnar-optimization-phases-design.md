# 列式化优化分阶段落地方案（Columnar Optimization Phases）

- 状态：待实施
- 依据：`docs/archive/benches/columnar-necessity-verification.md`（必要性验证已归档）
- 验证结论回顾：列式 DataChunk **4.7x**（立项）、SIMD 编译选项 **3.46x**（立项，拒绝手工 SIMD）、Validity Bitmap **不立项**、选择向量跨算子 **3300x**（立项）、Morsel 并行扩展与存储层改造 **待评估**

## 阶段总览

| 阶段 | 内容 | 依赖 | 预期收益 | 立项依据 |
|------|------|------|----------|----------|
| Phase 0 | SIMD 编译选项落地 | 无 | 全查询 3.46x（编译级） | 已验证 |
| Phase 1 | 列式 DataChunk（typed 定长列 + 行式兜底） | 无 | 过滤/投影 2~5x | 已验证 4.7x |
| Phase 2 | 选择向量跨算子传播 + scan 谓词下沉 | Phase 1 | 低选择率 10~1000x | 已验证 3300x |
| Phase 3 | 并行扩展评估、存储层读取基线 | Phase 1/2 数据 | 待定 | 待验证 |

优先级依据：收益/成本比与依赖关系。Phase 0 独立且零代码，最先落地；Phase 1 是 Phase 2/3 的地基；Phase 3 需要前两阶段数据支撑再立项。

## Phase 0：SIMD 编译选项落地

**目标**：以零代码成本获得已验证的 3.46x 自动向量化收益。

**改动**：
- `.cargo/config.toml`：`[target.x86_64-unknown-linux-gnu]` 设 `rustflags = ["-C", "target-cpu=x86-64-v3"]`（AVX2，Haswell+ 2013 年后 CPU，兼容本地部署主流硬件）
- 构建文档（README/AGENTS.md 构建说明）注明该配置与降级方式（`RUSTFLAGS="-C target-cpu=x86_64"` 回退基线）

**注意**：
- `rustflags` 只能通过 `.cargo/config.toml` 设置，`Cargo.toml` profile 不支持
- 影响所有 crate 的编译产物；CI 与本地保持一致（CI 环境如需基准可比性，同样设 RUSTFLAGS）

**验收**：
- `cargo bench --bench columnar_necessity_bench -- autovectorization`：scalar 组从 ~81µs 降至 ~23µs（3.4x+）
- `cargo test --test '*'` 全量通过
- 回退：删除 `.cargo/config.toml` 条目即恢复基线，无代码残留

## Phase 1：列式 DataChunk（typed 定长列 + 行式兜底）

**目标**：为 i64/f64/i32 定长列引入 typed 列存储（`Vec<i64>` 等），行式兜底，渐进式切换，不做全量重构。

**改动点**（`crates/graphdb-query/src/query/executor/streaming/chunk.rs` 为主）：
- P1.1 列布局判定与构建：新增 typed 列表示（如 `TypedColumn::I64(Vec<i64>)` / `F64(Vec<f64>)` / `I32(Vec<i32>)` / `Fallback(Vec<Value>)`）；source 物化时按 SlotLayout 类型信息选布局（混合类型列/字符串列/NULL 列走 Fallback）
- P1.2 语义保持层：`get_column`/`get_by_slot`/`column_ref`/`take_indices`/`filter_indices` 在 typed 列上保持现有返回语义（Value），内部从 typed 批量转换；RowPool 增加 typed 分配池
- P1.3 typed 批量求值快路径：`eval_with_cache` 在纯 typed 列上直接批量运算（Binary/Unary/TypeCast），避免逐行 Value 构造；结果仍以 `Vec<Value>` 输出（保持接口不变，内部提速）
- P1.4 观测与内存：ColumnarStats 增加 typed 命中计数（在现有 hit/miss 之外）；MemoryTracker 记账 typed 分配

**不做**：Validity Bitmap（已验证不必要）；NULL 列进入 typed 布局（null 稀疏时保持行式或后续按 HasNull 双路径重估）。

**验收**（基于已验证基准）：
- `columnar_necessity_bench` 新增真实 DataChunk 路径用例：4096 行单列过滤 ≤250µs（当前 599µs，≥2.4x）
- `expr_eval`/`filter_throughput`/`column_materialize` 组无回归
- 全量测试通过；clippy 全绿
- 回退：typed 布局由运行时标志或构建 feature 开关，关闭即回行式

## Phase 2：选择向量跨算子传播 + scan 谓词下沉

**依赖**：Phase 1（列式下选择向量收益最大化；T4.3 已做 chunk 内延迟）

**目标**：
- Filter 输出选择向量（indices）而非物化 chunk，沿 Project → Join → Aggregate 链传播
- ScanVertices 谓词下沉到存储层 CSR 遍历（跳过不满足的邻居）

**改动点**：
- `chunk.rs`：`SelectionVector` 表示（indices 或位图）与 `take_indices` 延迟状态规范化，支持跨算子传递
- `operators/`：Filter 输出 selection；Project 透传 selection（懒物化）；Join/Aggregate 消费 selection 输入
- `source_operator/` 与 `graphdb-storage`：scan 谓词下推接口（`QueryStorage` 扫描签名扩展），CSR 遍历时应用
- `ColumnarStats`：记录 selection 传递命中与物化率，支撑 Phase 3 评估

**分步落地**（控制风险面）：
- P2.1 Filter → Project 透传（纯透传，语义等价）
- P2.2 传播进 Join（build 侧物化、probe 侧消费）
- P2.3 传播进 Aggregate（先简单聚合，再哈希聚合）
- P2.4 scan 谓词下沉（跨 crate 接口，独立验收）

**验收**：
- 端到端链式过滤（1% 选择率）相对当前物化路径 ≥10x
- 各步骤集成测试 + 全量回归通过；spill/排序路径不受影响（物化边界明确）
- 回退：selection 传播由执行器配置开关控制

## Phase 3：并行扩展评估与存储层读取基线（先验证后立项）

**目标**：补齐已验证清单中"待评估"两项的证据，数据达标才立项。

- P3.1 存储层读取基线：`storage_bench` 补充读取路径（宽表全扫 vs 窄表随机取属性），对齐写入路径既有基准
- P3.2 并行加速曲线：端到端并行基准（1/2/4/8 核），利用 `ProfileBoard` 的 `parallel_work_time_us`/`parallel_wall_time_us` 计算并行效率；区分表扫描型与图遍历型查询
- 决策规则：P3.1 数据显示存储读占端到端 >30% 且列式属性块 PoC ≥1.5x 才立项存储改造；P3.2 加速 ≥2x（4 核）才立项并行扩展

## 非目标（本轮明确不做）

- Validity Bitmap：四档 null 率实测全落后 Option 编码，除非未来引入"HasNull 标志 + fast/slow 双路径"设计再重估
- 手工 SIMD 代码：编译器自动向量化（Phase 0）已获主要收益，手工展开实测更慢
- 全量列式化：字符串列/图实体列保持行式，typed 覆盖定长标量列

## 验收总则

每阶段结束用统一 Q-set 回归（operator_bench、query_bench、end_to_end_bench + `cargo test --test '*'`），并以 `docs/archive/benches/columnar-necessity-verification.md` 中的基线数字为对照；不达标即回退或调整范围。
