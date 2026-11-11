# Linkrs 对等 Ladybug 公开数据集基准

**日期**: 2026-11-11
**参考方案**: `graphdb-ben/docs/linkrs-ladybug-parity-benchmark.md`（初步设计，本文为准）
**对照仓库**: `https://github.com/ladybugdb/benchmarks`

目标：用同一批源 CSV、同一组查询语义、同一套计时口径，在 Linkrs 上复现 Ladybug 的基准流水线，使两边数字可以直接并排。

本文已按仓库现状核实并修正参考方案，差异见第 10 节。

---

## 1. 硬约束（对等公平性）

1. **同一份 CSV**。两边都从 `$CSV_DIR` 导入官方/同一份文件，禁止各自 Datagen 再生。
2. **同一查询语义**。结果行数、Top-N 值必须与 Ladybug `.benchmark` 的 `expectedNumOutput` / `----` 期望块对齐；对不上先修查询或标 skip，不上报。
3. **同一计时口径**。主套件 warmup=1、run=5；LSQB warmup=1、run=3、去 max 再平均。只比 execution 时长中位数，compiling 另表。
4. **同一硬件与构建**。报告写明双方二进制、commit、线程数、内存（Linkrs 报进程 RSS 峰值，不人为对齐 Ladybug 的缓冲池上限）。
5. **Embedded 路径**。Linkrs 走 `GraphDatabase::open` + `session.execute`，不走 HTTP/gRPC，不把传输层算进查询时间。
6. **导入单独成组**。COPY 耗时单独上报，与查询时间拆开。

做不到语义对齐的查询进 skip 清单（`benches/parity/skip.txt`），对比表标 `N/A`。

---

## 2. 目录布局

```
benches/parity/
├── README.md
├── schema/
│   └── lsqb.cypher            # LSQB projected-fk 的 DDL + COPY 脚本
├── queries/
│   └── lsqb/q1..q9.benchmark  # 与 Ladybug .benchmark 同构
└── skip.txt                   # dataset/group/name  reason
```

执行器是根 crate 的 binary `linkrs-parity-bench`（`harness = false` 的手写循环，不用 Criterion——公开数据集是分钟级，Criterion 的固定窗口不适用）。

Python 编排（下载、compare.py 对比表）后置到规模放大阶段；v1 用 Rust 二进制完成 header 改写 + COPY + 跑查询的完整闭环。

---

## 3. 执行器 `linkrs-parity-bench`

```
linkrs-parity-bench copy \
  --dataset lsqb-sf0.1 \
  --csv-dir <CSV 目录> \
  --script benches/parity/schema/lsqb.cypher \
  --db <SERIALIZED_DIR>/linkrs-lsqb-sf0.1-serialized/db \
  --threads N

linkrs-parity-bench run \
  --db <同上> \
  --benchmark-dir benches/parity/queries/lsqb \
  --warmup 1 --run 3 \
  --out <输出 JSON 目录>
```

- `copy`：执行脚本内语句（按 `;` 切分，支持 `--` 注释），逐条计时，输出每张表的 COPY 秒数；结束时以 `checkpoint_on_close` 显式关闭数据库（见 4.4），`version.txt` 记录 git HEAD，版本一致跳过导入（`--force` 强制重导）。
- `run`：对目录下每个 `.benchmark` warmup N 次后跑 N 次，物化全部行，写 JSON（与 Ladybug log 字段对齐：`Status` / `Compiling time` / `Execution time` / 行数）。

## 4. 数据导入（serialize）

### 4.1 表头改写（v1 内置于二进制）

LSQB/LDBC projected-fk 数据集是 Neo4j admin-import 风格表头，Linkrs COPY 不识别。`copy` 模式读 CSV 首行并改写为 sidecar 文件（`<db>/prepare/<name>.csv`），不改原始文件：

| 原表头 | Linkrs 表头 |
|--------|-------------|
| `id:ID(Person)`、`id:ID(Comment)` … | `vid` |
| `:START_ID(X)` | `src` |
| `:END_ID(X)` | `dst` |
| `firstName:string` 等属性列 | 去掉 `:type` 后缀，保留属性名 |

### 4.2 schema 翻译规则

| Ladybug | Linkrs |
|---------|--------|
| `CREATE NODE TABLE Person (…, PRIMARY KEY(id))` | `CREATE SPACE lsqb (vid_type=INT64)` + `USE`；`CREATE TAG Person(firstName STRING, …)`；CSV 首列作 vid，不进属性 |
| `CREATE REL TABLE knows (FROM Person TO Person, …)` | `CREATE EDGE knows(…) FROM Person TO Person`；端点对必须声明，且**每个端点对一个边类型**（见 4.4）；边 CSV 前两列为 `src,dst` |
| `INT64/STRING/DATE/TIMESTAMP` | 同名（已对齐） |
| `SERIAL` 主键（Click hits） | `vid_type=INT64` + 导入时生成单调 vid（后置） |

### 4.3 执行顺序

1. `CREATE SPACE IF NOT EXISTS` + `USE`。
2. CREATE TAG/EDGE 全部 DDL。
3. 先 COPY 全部顶点，再 COPY 边（端点必须先存在）。
4. `BATCH_SIZE` 控制单次提交量。**注意**：参考方案提出的 `begin_bulk_load` 会话位于 `linkrs_storage::GraphStorage`，embedded `GraphDatabase` 尚未暴露该入口；v1 依赖 COPY 自身的批量提交，embedded API 暴露 bulk-load 会话后再切换（避免逐批 checkpoint 的 O(n²)）。

### 4.4 持久化约束（LSQB-sf0.1 冒烟实测）

- **边类型必须声明 `FROM <tag> TO <tag>`**。checkpoint 恢复按 `(src_label, dst_label, edge_label)`
  三元组注册边表；未声明端点对的边表只在写入时懒注册，进程重启后加载会被静默跳过——
  最小 repro（3 顶点 + 3 边）表现为顶点存活、边全丢。
- **`ALTER EDGE ... ADD FROM ... TO ...` 只改 schema，不注册物理边表**。因此 LSQB 的
  hasCreator / replyOf 按源表各拆成两个边类型（`HASCREATOR_COMMENT` / `HASCREATOR_POST`、
  `REPLYOF_COMMENT` / `REPLYOF_POST`），与 Ladybug LSQB 的分表结构一致；q5–q8 查询同步改名。
- **`copy` 模式结束必须显式 `close()` 且配置 `checkpoint_on_close`**。COPY 的边表 WAL 由
  后台线程延迟刷盘，直接退出进程会丢边（repro 验证）。
- **`CREATE TAG` 不允许空属性列表**：存储层要求顶点 tag 声明主键列 `id`，否则报
  `property_not_found: id`；`CREATE EDGE x()` 允许空列表。
- **保留字碰撞改名**：`Comment` 与保留字 `COMMENT` 冲突（词法器优先关键字），LSQB 节点标签
  `Comment` 统一改为 `Comment_`；该映射内联在 schema 与查询文件中，Python adapter 落地后进
  `adapters/dialect.py`。

---

## 5. 查询文件格式

沿用 Ladybug `.benchmark` 结构：

```
----name
lsqb/q1
----query
MATCH (n:Person) RETURN count(n.id)
----expected
50
```

- `----query` 块到空行为止；可选 `----prerun` / `----postrun`。
- `----expected`：期望行文本，逐行与结果物化文本比对（count 类查询是正确性门，不相等整行作废）。
- 无 `----expected` 时只校验可执行、记录行数。

---

## 6. 计时与正确性细则

- **v1 只报总时长**（parse+bind+plan+execute+物化，`Instant` 包住 `session.execute` 并 drain 全部行）。参考方案提出的 compiling/executing 拆分依赖 `query_stage_bench` 的阶段计时，但该拆分在 query crate 内部管线里，embedded `session.execute` 未暴露阶段耗时；v1 保持 embedded 对等口径，只报总时长，阶段拆分后置（需要 embedded API 暴露 metadata，或明确改走 query-crate 直连口径并写进报告）。
- 结果必须物化全部行，禁止只取第一行。
- COUNT 必须精确相等；浮点（PageRank 等）相对误差 1e-3。
- LSQB 样本处理与 Ladybug `lsqb/benchmark_runner.py` 逐行对齐：warmup 1 次 + 3 次，3 次中去 max 再平均。

---

## 7. 数据集优先级

1. `lsqb-sf0.1` 冒烟（数据在 `graphdb-ben/data/social-network-sf0.1-projected-fk`，devs 同步在
   `/data/kkkqkx/graphdb-ben/data/social-network-sf0.1-projected-fk`）：q1–q9 全部为 `MATCH … RETURN count(*)`，正确性就是一个整数。**第一步只证 COPY + MATCH 闭环，不谈快慢。**
   ✅ 已通过（devs，2026-11-11）：COPY 1,027,372 行与源 CSV 逐表一致；q1–q9 计数全部对齐，
   warmup=1、run=3 中位 1.0–1300 ms；结果与 COPY 日志归档在
   `docs/archive/perf/parity/lsqb-sf0.1-smoke/`。
2. `lsqb-sf1`（本地已有 projected-fk）放大验证。
3. LDBC sf10/sf100 算子组（54 queries，需下载数据）。
4. ClickBench（105 列宽表，INT128 探针，quoted-newline 预处理）、算法图（PageRank/WCC/加权最短路，k-core/SCC/Louvain skip）。

## 8. 对比产物

`compare.py`（后置）读两边 JSON，输出 markdown 对比表：

```
| group | query | ladybug exec ms (median) | linkrs exec ms (median) | ratio | rows match |
```

- ratio > 1 表示 Linkrs 更慢；相对偏差 ≥ 20% 高亮。
- skip 查询列在表底，不进几何平均。
- 文首固定环境块：CPU、affinity、内存、磁盘、双方 commit、线程。
- COPY 单独一张表：每表秒数 + 库体积。
- 结果检入 `docs/archive/perf/parity/`，不接 Ladybug 的 JWT 上报服务。

## 9. 风险

- LDBC 大规模 CSV 体积大，sf100/Graph500 只在独占机器跑。
- COPY 对 `|` 分隔、DATE/TIMESTAMP 解析需用小文件 golden test 锁住（从 `copy_e2e.rs` 扩）。
- INT128 / SERIAL 可能卡住整组 Click；允许先导入子集列，其余 skip。
- 算法数值对不齐时，先对齐迭代次数与阻尼，再比时间；仍对不齐就 skip。

## 10. 相对参考方案的修正

| 参考方案 | 现状核实结果 |
|----------|--------------|
| COPY 语法 `WITH (HEADER true, DELIMITER '\|', BATCH_SIZE n)` | 正确，`docs/release/statements/02_dml_data_manipulation.md` 确认选项可放括号内 |
| `storage.begin_bulk_load()` 作为导入第 3 步 | 该会话在 `linkrs_storage::GraphStorage` 上，embedded `GraphDatabase` 未暴露；v1 降级为 COPY 自带批量提交，暴露后再切换 |
| compiling/executing 拆分接入 `query_stage_bench` | 阶段计时在 query crate 内部管线，embedded API 不暴露；v1 只报总时长 |
| 边 CSV 改写 header 为 `src\|dst` | 正确且必须；具体化：projected-fk 数据集是 `:START_ID(X)\|:END_ID(X)`，属性头带 `:type` 后缀，均需改写 |
| 边类型可省略端点声明、多条 CSV 折进同一边类型 | 不成立：`FROM/TO` 是 checkpoint 恢复注册边表的必要条件；`ALTER EDGE ADD FROM` 不注册物理表 → 每个端点对一个边类型（见 4.4） |
| `CREATE TAG Person()` 空属性列表可行 | 不成立：顶点 tag 必须声明主键列 `id`（见 4.4） |
| 导入后进程直接退出即可 | 不成立：COPY 边表 WAL 由后台线程延迟刷盘，必须 `checkpoint_on_close` + 显式 `close()`（见 4.4） |
| Python 编排（serialize.py/runner.py）先行 | 后置；v1 编排内置于 Rust 二进制，减少一步依赖 |
| 第 2 步即 LDBC-sf10 | 本地仅有 sf0.1/sf1 数据；先做 LSQB-sf0.1 → sf1，LDBC 待下载后追加 |
