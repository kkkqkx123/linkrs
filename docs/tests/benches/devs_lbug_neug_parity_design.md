# devs 三引擎性能对比环境补充与对比分析设计

**日期**: 2026-11-12
**范围**: 在 devs（`ssh devs`，无外网）上补齐 lbug（Ladybug）与 neug（Alibaba NeuG）运行环境，并与 Linkrs 完成三方性能对比。
**上游依据**:

- `graphdb-ben/docs/linkrs-ladybug-parity-benchmark.md`（三流水线方法论）
- `docs/release/benchmark/parity-benchmark.md`（Linkrs 侧权威口径，`linkrs-parity-bench` 现状）
- `docs/dev/build/devs-sync.md`（linkrs 代码同步规范）
- `graphdb-ben/docs/spec/sync-plan.md`（基准数据同步规范）
- `docs/tests/remote_bench_server_design.md`（devs 绑核与隔离手段）

本文只解决“devs 上缺 lbug/neug 环境”这一缺口，以及三引擎对比的口径对齐；不重复上述文档已有的 Linkrs 单引擎流程。

## 1. 现状盘点

### 1.1 本地已具备

- `graphdb-ben`（submodule，`https://github.com/kkkqkx123/graphdb-ben`）：`lbug-ben`  vendored 套件（上游 `ladybugdb/benchmarks@42dd288`）、`scripts/kron` 图生成器、已落地数据集。
- 数据集：LSQB sf0.1（52MB）、sf1（562MB）、Neug SF1 标准包（1.1GB，阿里云 OSS 北京 endpoint 获取）、Snap 双数据集（LiveJournal 1.1GB、Pokec 约 1.2GB 转换后）、graph500-27 仅 schema（40GB 级数据从不在本地生成）。
- `linkrs-parity-bench`（根 crate binary）：`copy`（表头改写 sidecar + version.txt 跳过）与 `run`（warmup + N 次 run，LSQB 口径）闭环已打通 LSQB-sf0.1；查询覆盖目前只有 `benches/parity/queries/lsqb/q1..q9`。
- PyPI 可用版本（本地已验证可下载）：`ladybug==0.22.3`（约 10MB wheel）、`neug==0.3.1`（约 18MB wheel）。

### 1.2 devs 已具备

- `/data/kkkqkx/linkrs/repo`：linkrs 源码（暖 `target/` 约 18GB，工具链 cargo/rustc 1.99.1 用户态可用）。
- `/data/kkkqkx/graphdb-ben/`：仅 `data/social-network-sf0.1-projected-fk`（52MB）与 `packages/lsqb-sf0.1.tar.gz`；**无基准代码、无 lbug、无 neug**。
- 硬件：32 逻辑 CPU（2×E5-2667 v3，AVX2）、31GB 内存、`/data` 可用 7.7TB。
- 约束：Ubuntu 18.04（glibc 2.27，内核 4.15）、系统 Python 仅 3.6.9 且无 pip、cmake 3.10 / g++ 7.5、无 sudo、无外网。

### 1.3 缺口一览

| 缺口 | 说明 |
|------|------|
| Python 底座 | 两引擎 wheel 均要求 Python ≥3.8（ladybug 要求 ≥3.10），devs 系统 Python 3.6 不可用 |
| lbug 运行环境 | 无 `ladybug` wheel、无 C++ `lbug_benchmark` 编译产物 |
| neug 运行环境 | 无 `neug` wheel 及其依赖（protobuf 等需 pin 版本） |
| 基准代码 | devs 侧 `graphdb-ben` 只有 sf0.1 数据，无 `lbug-ben/` 与 `scripts/` |
| 中大    数据 | sf1、Neug SF1、Snap 双数据集、graph500-27 均不在 devs |
| 对比脚本 | 无 neug 侧 runner（lbug 侧有 `lsqb/benchmark_runner.py` 可直接复用） |

## 2. 总体路线（结论先行）

1. **两引擎一律用官方预编译 wheel，不在 devs 源码构建**。devs 的 cmake 3.10 / g++ 7.5 达不到两引擎 C++ 构建要求，升级工具链需 sudo；wheel 路线零编译、用户态可完成。
2. **先补用户态 Python，再离线装 wheel**。本地下载独立 Python（含 pip）打包传 devs，wheelhouse 本地 `pip download` 一次备齐后离线安装。
3. **代码走 git bundle，数据走压缩包，graph500-27 现场生成**。与现有两份 sync 规范衔接，不另起传输通道。
4. **对比顺序：LSQB-sf0.1 三方 COUNT 对齐先行**，再放大到 sf1 与算子组；主套件 C++ `lbug_benchmark` 与 ClickBench 冷缓存脚本后置。

## 3. 用户态 Python 底座

devs 系统 Python 3.6 无法安装任一引擎，且无 root 不能 apt 升级。采用用户态独立 Python：

- 版本选择：CPython **3.11**（落在 `ladybug>=3.10,<3.15` 与 `neug>=3.8` 的交集，避开最高版以留兼容余量）。
- 获取方式二选一（本地有外网处完成）：`uv python install 3.11` 后打包其安装目录，或下载 python-build-standalone 同版本 tarball。
- devs 落点：`/data/kkkqkx/tools/python3.11`（随 `graphdb-ben` 数据同分区，不占用家目录配额），解压即用，无需安装。
- 验证标准：该解释器 `import ssl` 可用（pip 需要）、`platform.machine() == x86_64`、能离线安装 wheelhouse。

此后所有 lbug/neug 命令统一用该解释器建 venv（`python3.11 -m venv /data/kkkqkx/graphdb-ben/.venv-bench`），不污染系统 Python。

## 4. lbug 环境补充

### 4.1 版本锁定

- Python 侧：`ladybug==0.22.3`（本地已验证：顶层模块 `ladybug`，含 `_lbug` 扩展，`QueryResult` 提供 `get_compiling_time` / `get_execution_time`，与 LSQB runner 口径天然一致）。
- 基准脚本侧：沿用 `graphdb-ben/lbug-ben` vendored 上游 `42dd288`；wheel 版本与 C++ 上游 commit 的对应关系记入结果报告环境块，不强求精确同源（Python 路径只用于 LSQB/Click 独立脚本时）。

### 4.2 import 名核对（动工前必须消项）

vendored `lbug-ben/lsqb/*.py` 现用 `import lbug as lb`（见 `VENDORED.md`），但 PyPI `ladybug==0.22.3` 的顶层模块是 `ladybug`（其文档示例亦为 `import ladybug as lb`），本地实测 `import lbug` 失败。处理：

- 在 devs 用新 Python 底座实测确认导入名；
- 基准脚本加兼容垫片（优先 `lbug`，回退 `ladybug`），或按实测结果修正 `VENDORED.md` 的改动说明；
- 该核对结论写入结果报告，避免三方复现时因导入名分叉。

### 4.3 wheelhouse 内容

`ladybug==0.22.3` 无强制运行时依赖（dev extras 仅开发用），wheelhouse 仅一个 wheel（约 10MB）。glibc 边界注意：该 wheel 标注 `manylinux_2_27`，devs 恰为 glibc 2.27，以实测 `import ladybug; Database(':memory:')` 建库为准；若失败则退回旧版 wheel（`pip index versions ladybug` 另有 0.21/0.20 系列）再试。

### 4.4 C++ 主套件取舍

主套件（算子微基准）依赖随 Ladybug 源码构建的 `lbug_benchmark` 二进制；devs 工具链不足以构建，首阶段**不引入**。LSQB 与 Click 独立脚本只依赖 Python wheel，可先行。何时引入 C++ 主套件对比如ණය：待 LSQB 三方对齐完成后再单独立项（预编译 binary 或容器构建，二选一）。

## 5. neug 环境补充

### 5.1 版本与依赖锁定

- `neug==0.3.1`，`Requires-Python >=3.8`，wheel 标注 `manylinux_2_17`（devs glibc 2.27 兼容）。
- 强依赖必须同 wheelhouse pin：`protobuf==5.29.6`（精确 pin）、`packaging/click/prompt_toolkit/tabulate/PyYAML/tqdm/Flask` 等按 `pip download neug` 解析结果全量收录，devs 侧 `--no-index --find-links` 离线安装。

### 5.2 数据集说明

- 本地 `data/social_network-sf1-CsvComposite-StringDateFormatter` 即 Neug 官方 LDBC SNB SF1 包（北京 endpoint 直链），devs 同样走压缩包同步，不在 devs 重新下载。
- neug 内置 `datasets` 仅含 `modern_graph/tinysnb/ogbn_mag_small`，**不含 LSQB**；LSQB 对比仍用 projected-fk CSV + 手写 schema（与 Ladybug `serializer.convert()` 的 bidirectional/message 派生逻辑对齐，不自创第二份转换）。
- neug 的 `COPY` 与类型方言与 Ladybug 接近（`CREATE NODE/REL TABLE`、`COPY ... FROM`），schema 翻译复用 `benches/parity/schema/lsqb.cypher` 的端点对拆分经验（每个端点对一个边类型）。

### 5.3 计时手段

neug `QueryResult` 无 compiling/execution 内计时（仅 `to_arrow/get_profile_metrics` 等），对比口径定为：

- 主口径：解释器外 `perf_counter` 包裹 `execute` 全程（含结果物化到 Arrow），与 Linkrs `run` 的 wall-clock、`lbug` 的 `get_execution_time` 并列上报并注明口径差；
- 辅口径：`PROFILE` 前缀查询的 `get_profile_text` 用于定性拆分，不进入中位数对比表。

### 5.4 线程口径声明

neug `Database(max_thread_num=…)` 仅约束 TP 服务槽位，AP 查询当前单线程实现。报告必须如实记录该限制：LSQB 这类 AP 查询的 neug 数字按单线程解读，不与 Linkrs/lbug 的多线程数直接比加速比，只比同线程意愿下的 wall-clock 并附注。

## 6. 数据与代码同步方案

承接 `devs-sync.md`（linkrs 日常包**排除** `graphdb-ben/`）与 `graphdb-ben sync-plan.md`（数据包走校验清单）：

### 6.1 devs 目标布局

```
/data/kkkqkx/graphdb-ben/
├── AGENTS.md  docs/  lbug-ben/  scripts/   # git bundle 同步
├── data/                                    # 压缩包解压（sf0.1/sf1/Neug SF1/snap/graph500-27）
├── wheels/                                  # wheelhouse（ladybug/neug及依赖）
├── packages/                                # 传输缓存
└── results/                                 # 三方结果 JSON（回传）
/data/kkkqkx/tools/python3.11               # 用户态 Python
/data/kkkqkx/linkrs/                        # 现状不变
```

### 6.2 传输分包

| 包 | 内容 | 量级 |
|----|------|------|
| 代码包 | `graphdb-ben` git bundle（AGENTS/docs/lbug-ben/scripts，不含 data） | KB 级 |
| 数据包 A | LSQB sf1（562MB）、Neug SF1（1.1GB） | GB 级，`tar.zst` + rsync 断点续传 |
| 数据包 B | soc-livejournal 转换后 CSV（1.1GB）、soc-pokec（约 1.2GB）；优先传原始 Snap gz + 转换脚本 | GB 级 |
| 数据包 C | graph500-27 **不传**，devs 运行 `scripts/kron/build_and_generate.sh` 现场生成（约 40GB，约 25 分钟） | 现场生成 |
| 环境包 | 用户态 Python 3.11 tarball + wheelhouse（含 `SHA256SUMS`） | 百 MB 级 |

sf0.1 已在 devs，無需重传。

### 6.3 校验

- 数据包沿用 sync-plan 的行数基线抽查（LiveJournal knows 68,993,773 / person 4,847,572、Pokec knows 30,622,563 / person 1,632,803）。
- wheelhouse 与 Python 包用 `SHA256SUMS` 校验（环境包非引擎数据，校验和不受 linkrs“同步包不用校验和”政策限制）。
- Kron 生成物校验 SEED（固定 `SEED=42`）与 scale 3 小规模对拍。

## 7. 三引擎对比分析方法

### 7.1 对齐基线

- 同一份 CSV、同查询语义（行数/Top-N 与 `.benchmark` 期望块对齐，对不上进 skip，不拿近似查询充数）。
- 同硬件同绑定：独占 NUMA node1（`numactl` + `taskset`，见远程基准设计），线程数三方一致并写入环境块；Linkrs 报 RSS 峰值、lbug 报 bm-size、neug 报 buffer_strategy。
- 同正确性门：COUNT 必须相等；PageRank/加权最短路允许相对误差并记录阻尼与迭代次数。

### 7.2 各引擎执行路径

| 引擎 | LSQB-sf0.1 执行方式 | 计时来源 |
|------|--------------------|----------|
| Linkrs | `linkrs-parity-bench run --benchmark-dir benches/parity/queries/lsqb --warmup 1 --run 3` | 内计时（去 max 平均，与 LSQB runner 同实现） |
| lbug | `lsqb/benchmark_runner.py`（`DRY_RUN=true`，`SCALE_FACTOR=0.1`，`SERIALIZED_DIR` 指向 devs 数据） | `get_execution_time`，warmup 1 + run 3 去 max 平均 |
| neug | 新增 `neug_runner.py`（COPY + q1–q9，warmup 1 + run 3 去 max 平均，结果全物化） | 外部 wall-clock（5.3），物化策略与 Linkrs 一致 |

neug runner 落点为 `graphdb-ben` 新脚本（与 `lbug-ben` 并列，不混入 vendored 目录），复用同一 CSV 与同一查询语义。

### 7.3 报告产物

- 原始 JSON：三方各写 `results/<date>-<gitsha>-<dataset>.json`（字段对齐 Ladybug log：status/compiling/execution/行数）。
- 对比 markdown：`docs/archive/perf/parity/<date>-<dataset>.md`，列为 `query | linkrs exec ms (median) | lbug exec ms | neug wall ms | rows match`，ratio 以 lbug 为基线（`linkrs/lbug`、`neug/lbug`），相对偏差 ≥20% 高亮，skip 查询列底并注明原因。
- 文首环境块：三方版本（linkrs git sha / ladybug wheel 版 / neug wheel 版）、CPU 亲和、内存策略、线程数、AVX2 状态、是否 drop_caches。

### 7.4 阶段顺序

1. LSQB-sf0.1 三方 COUNT 对齐（正确性优先，不谈快慢）。
2. LSQB-sf1 放大 + Linkrs/LDBC-sf10 算子组（lbug 先行，neug 视 COPY 兼容性跟进）。
3. 主套件 C++ `lbug_benchmark`、ClickBench、算法图（PR/WCC/加权最短路；k-core/SCC/Louvain 暂 skip）。

## 8. 风险与限制

- devs 频率策略 `powersave` 不可调（无 root），为当前最大误差源：以前置检查 + 同核集重复测量 + 中位数口径对冲，跨日期数字不直接对比。
- 共享机器偶发干扰：沿用远程基准设计的前置检查（负载/在线用户/他人编译任务），阈值外等待。
- `ladybug` wheel 的 `manylinux_2_27` 恰卡 devs glibc 下限，以实测建库为准，留旧版回退。
- neug AP 单线程：多线程扩展性对比暂只含 Linkrs 与 lbug，neug 只出单线程基线。
- Ubuntu 18.04 内核 4.15 无 io_uring：存储层数字解读时单独注明（沿用远程基准设计结论）。
- 大包（GB 级）只在非高峰传，超约 2GB 用 `rsync --partial --append-verify`。

## 9. 验收标准

1. devs 上用户态 Python 3.11 可用，`ladybug` 与 `neug` 均可离线安装并建库。
2. LSQB-sf0.1 九条 COUNT 三方一致（Linkrs/bug/neug 结果整数相等）。
3. 产出一份三方对比 markdown（含 COPY 表、查询中位数、20% 高亮、skip 清单、环境块）。
4. 全部传输包可复现：他人按本文第 6 节可重建同一 devs 状态。

## 附录：动工核对表

- [ ] 本地 `pip download ladybug==0.22.3 neug` 备齐 wheelhouse（含 `protobuf==5.29.6` pin）
- [ ] 本地备好用户态 Python 3.11 包
- [ ] 实测确认 `ladybug` / `lbug` 导入名，定垫片方案并更正 `VENDORED.md` 说明
- [ ] `graphdb-ben` git bundle 传 devs 并检出 `lbug-ben/` + `scripts/`
- [ ] 数据包 A/B 传 devs 并按行数基线抽查
- [ ] graph500-27 在 devs 现场生成（后置阶段）
- [ ] 编写 `neug_runner.py`（LSQB 口径：warmup 1 + run 3 去 max 平均）
- [ ] LSQB-sf0.1 三方 COUNT 对齐，出首份对比 markdown
