# 性能基准总览

基准已按被测代码归属下沉到各子包，根 `benches/` 不再承载任何 `[[bench]]`
目标。本文件只说明新布局与常用命令，各基准的具体决策背景见
`docs/analysis/benches_migration_analysis.md`，迁移实施记录见
`docs/analysis/benches_migration_record.md`。

## 布局

```
benches/                        # 根目录：仅保留共享数据与历史产物
├── data/                       # 基准数据（GQL 文件与生成脚本，各包基准共用）
├── results/                    # 历史报告封存（迁移前的输出，不再更新）
├── README.md                   # 本文件
└── BENCHMARK_REPORT.md         # 历史报告（迁移前 6 套件时代的快照，仅供查阅）

crates/linkrs-storage/benches/  # 存储层（13 个）：GraphStorage 读写、CSR、
                                # ingest 归因、提交竞争、WAL、恢复、分配计数
crates/linkrs-transaction/benches/ # 事务层（2 个）：事务操作/MVCC/认证、
                                   # 写写冲突
crates/linkrs-query/benches/    # 查询层（6 个）：查询主基准、阶段拆分、
                                # 遍历曲线、并行扩展、OLAP 基线、执行器微基准
crates/linkrs-fulltext/benches/ # 全文检索（1 个，需 --features fulltext）
crates/linkrs-core/benches/     # 核心类型 serde（1 个）
crates/simvec/benches/          # 向量距离内核（1 个）+ 已有向量基准
```

各包 `benches/` 内自带所需的 `bench_group.rs`（统一 warm-up／measurement
窗口）与 `results_report.rs`（写 `benches/results/<suite>/results.txt`），
不再跨包共享。 plain-main 基准（`harness = false` 的手工中位数报告）与
criterion 基准结果口径不同，横向对比时注意区分。

## 运行

```bash
# 某包的全部基准
cargo bench -p linkrs-storage
cargo bench -p linkrs-query
cargo bench -p linkrs-transaction

# 单个基准
cargo bench -p linkrs-storage --bench ingest_bench
cargo bench -p linkrs-query --bench olap_e2e_bench
cargo bench -p linkrs-query --bench executor_bench

# 全文基准需要特性门
cargo bench -p linkrs-fulltext --features fulltext --bench fulltext_bench

# SIMD 探针跑两遍对比
cargo bench -p linkrs-query --bench executor_bench
RUSTFLAGS="-C target-cpu=native" cargo bench -p linkrs-query --bench executor_bench
```

回归门：`olap_e2e_bench` 的 Q1～Q5 与 `traversal_perf_bench` 的两项阈值
（锚定 1 跳、非锚定 2 跳）是优化工作的基线门，改动执行器或存储读路径后
必须跑一次对照。

## 新增基准时

1. 放到被测代码所在的包，不要放回根目录。
2. 复用包内已有的 `bench_group::create_benchmark_group`，保持各包窗口一致。
3. 跨 3 个以上工作区包、或触及网络传输层的端到端基准，才考虑放在根目录
  （当前没有，新建前先评估是否真有必要）。
4. `linkrs-server`／`linkrs-api` 的真实基准（HTTP 路由／序列化、gRPC 编解码、
   并发请求）目前仍然缺失，见迁移记录中的后续工作。
