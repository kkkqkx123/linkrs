# 向量引擎后续改进综合分析与实施方案

> 状态：方案文档（2026-08-26）。
>
> 前置文档：
> - `docs/plan/pgvector_implementation_details.md`（pgvector问题处理详解，§8.2对照表）
> - `docs/plan/vector_local_engine_pgvector_analysis.md`（效仿分析）
> - `docs/vector/vector-engine-design.md`、`docs/extend/vector/qdrant/qdrant_features.md`（量化与稀疏向量配置）
> - `docs/benchmark/vector_search_baseline_report.md`（基线，待数据驱动阈值）
>
> 约束更新（2026-08-26）：
> - **无存量部署兼容负担**，无需为旧硬件保留 `x86-64-v3` 二进制兼容
> - `x86-64-v3` 仅为向量化初步尝试（`.cargo/config.toml:24`），非固定模式
> - 量化对 `vector-search`（本地）与 `qdrant`（远程）均有价值，需统一设计
> - 稀疏向量与 `tantivy BM25` 非同物，`qdrant` 稀疏索引可复用思路，但需求小，仅给出初步设想，暂不实现

---

## 0. 摘要

| 项 | 结论 | 优先级 |
|---|---|---|
| pgvector vs vector-search | 6大维度已对齐（§1），本地“mmap分段+WAL+可丢弃派生索引”与pgvector“页+GenericXLog”语义等价，无架构缺口 | 已完成，仅剩可观测调优 |
| 向量化后继 | `v3`不固定，去编译期单档，改运行时多档（AVX2→AVX512族+ARM NEON/SVE+`std::simd`），以bench阈值准入 | P1，2-3天（不含bench） |
| 量化 | 正式实现三类（Scalar 4x / Binary 32x / Product X4-X64），本地与Qdrant复用同一 `CollectionConfig.quantization_config` `crates/vector-search/src/types.rs:276` | P0，4-5天分期 |
| 稀疏向量 | 暂不实现，仅设想：独立 `SparseVector{dim,nnz,indices[],values[]}` + 倒排 `WAND`，与BM25/`qdrant sparse_vectors`区分 | P2，设想归档 |

---

## 4. 稀疏向量：初步设想（暂不实现）

### 4.1 与BM25的区分

* `tantivy` BM25：词频倒排+`tf-idf`评分，面向关键词检索，无向量语义；已用于 `graphdb-search`
* 稀疏向量 `ref/pgvector/src/sparsevec.h:12` `SparseVector{dim,nnz,indices[],values[]}` + `sparsevec.c:939` `inner_product`：学习稀疏（SPLADE）每维为词表权重，`dot`检索，`dim`可达1B（`pgvector README:253` `sparsevec up to 1,000 non-zero`），`qdrant_features.md:245` `sparse_vectors_config` 独立于稠密

两者倒排结构可复用，但评分与训练来源不同，不可等价。

### 4.2 本地设想模型（归档）

```
SparseVector { dim:u32, nnz:u16, indices:u32[nnz], values:f32[nnz] }  // 0-based有序，复用 sparsevec.h:16 布局
文件：sparse.bin (mmap) + sparse_index.bin (倒排)
索引：term -> posting[(slot, weight)] + WAND/MaxScore 跳表（Qdrant稀疏同款）
查询：sparse_query dot 稀疏倒排求top-N → 可选 dense 混合 RRF
```

* 存储：与 `vectors.bin` 并列，`segment_bytes` 按 `nnz*8` 变长，需独立 `SparseVectors` 管理
* 距离：仅 `Dot` 有意义（`Cosine`可归一复用），`distance/sparse.rs` 新增稀疏`dot`
* 混合：与稠密结果 `Reciprocal Rank Fusion` 融合（`pgvector README:638` hybrid 段），非本期

### 4.3 复用点

* 倒排可复用 `tantivy` `InvertedIndex` 抽象，但 `sparsevec` 的 `values` 权重非 `tf`，需定制 `Weight` trait
* Qdrant稀疏 `SparseVectorParamsBuilder` `qdrant_features.md:241` 可作为远程透传模板，本地与远程稀疏配置同 `CollectionConfig` 扩展 `sparse_config: Option<SparseConfig>`

### 4.4 暂不实现理由

* 主路径为稠密embedding（`768/1536`），图DB顶点向量无稀疏产生方
* 收益小，引入变长存储+倒排+WAND复杂度与现有稠密 `HNSW/IVF` 正交
* 标记 `TODO(sparse)`，待量化落地且出现 `SPLADE` 明确需求后，以Qdrant稀疏为参考再议

---

## 5. 优先级与路线图

```
P0 量化 Q1 Scalar (2天) ─┬─ P0 量化 Q2 Binary (1天) ── P0 量化 Q3 PQ (2天)
P1 向量化 V1 AVX512+NEON (2天) ── V2 std::simd评估 (1天)
P2 稀疏设想归档（本文）
```

每期以 `cargo bench -p vector-search --bench vector_scan_bench --bench ivf_bench --bench hnsw_build_bench` 同机复测为阈值，未达>10%不合并。

---

## 6. 附录：关键锚点

* 向量存储：`crates/vector-search/src/storage/vectors.rs:19` `Vectors`，`storage/meta.rs:54` `Meta`
* 距离：`crates/vector-search/src/distance/avx2.rs:42` `distance_l2`，`distance/kernel.rs:29` `Kernel`，`distance/naive.rs:20` 基线
* 量化类型：`crates/vector-search/src/types.rs:260` `QuantizationType`，`types.rs:276` `QuantizationConfig`，`types.rs:426` `CollectionConfig.quantization_config`
* Qdrant量化：`docs/extend/vector/qdrant/qdrant_features.md:137` `qdrant_configuration.md:198`
* 稀疏：`ref/pgvector/src/sparsevec.h:12` `SparseVector`，`ref/pgvector/README.md:253` 约束
* 编译：`.cargo/config.toml:24` `target-cpu=x86-64-v3`

