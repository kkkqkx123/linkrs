# 内置向量引擎 Phase C 设计定案与实施方案（qdrant 路径适配 + 收尾）

> 状态：实施方案（2026-08-23）。
>
> 前置：`docs/plan/vector_local_engine_plan.md`（总方案）；Phase A/B 已合入
> （`crates/vector-search` Tier 0/Tier 1、`VectorBackend` 本地路径、coordinator
> WAL 提交协议）。
>
> 本文回答三个问题：Phase C 三项清单的现状核对（§1，第一项约九成已随
> Phase A/B 顺带完成）；剩余缺口的决策定案（§2，C1~C6）；完整代码修改方案
> 与验收标准（§3~§4）。

---

## 0. 结论摘要

| # | 缺口 | 决策 |
|---|------|------|
| C1 | 调用点仍无条件构造 HNSW `CollectionConfig`，本地引擎静默忽略 | 字段保留在类型中（Qdrant 建集合需要）；调用点改为按后端条件构造：本地路径不设置索引字段 |
| C2 | 根 crate `[dev-dependencies]` 无条件拉 vector-client（tonic/prost 进入一切测试构建） | 移除该依赖；qdrant-only 测试改经 graphdb-sync 的 feature 门控公开再导出引用类型 |
| C3 | `src/test_utils/sync_helpers.rs` 过时注释与 qdrant-only helper | helper 门控从 `vector-qdrant` 放宽为 `vector`（coordinator API 本就后端无关），类型路径改道再导出 |
| C4 | `docs/vector/*` 停留在 Qdrant/Mock 时代；archive 未标注取代 | 按"重写 / 重定位 / 归档 / 保留"处置矩阵逐个处理（§3.2） |
| C5 | 仓库无任何 CI 配置 | 新增 `.github/workflows/ci.yml`，四 job：lint / 默认全量测试（内置路径主战场）/ feature 矩阵 / bench 编译 |
| C6 | "全链路"范围未定义；查询层 e2e 缺失；图写入→向量自动传播无生产接线 | 本期全链路 = 查询语句 / VectorApi → coordinator → 本地引擎，新增查询层 e2e 补齐；图写入自动传播在两条路径下均缺产生侧，登记为独立后续项，不在收尾阶段扩 scope |

估时修正：总方案的 1~2 天偏紧（CI 从零搭建 + 文档批量处置），修正为 **约 2 天**。

---

## 1. 现状盘点

### 1.1 清单第一项：类型转发与 feature 重构、错误映射 —— 约九成已完成

以下工作已随 Phase A/B 合入，无需重复实施：

| 子项 | 状态 | 位置 |
|------|------|------|
| 共享类型迁至 vector-search 并转发 | 已完成 | `crates/vector-client/src/types.rs`（`pub use vector_search::types`） |
| root feature 重构（`vector` 默认开启、`vector-qdrant` 门控） | 已完成 | 根 `Cargo.toml:186-191` 及各子 crate features |
| 双后端统一抽象 `VectorBackend::{Local,Qdrant}` | 已完成 | `crates/graphdb-sync/src/sync/backend.rs` |
| 错误映射 `From<VectorSearchError>`（本地路径） | 已完成 | `crates/graphdb-sync/src/sync/vector_error.rs:136` |
| 旧错误映射 `From<VectorClientError>` 仅服务 qdrant 路径 | 已完成 | 同上 :178 |
| `EngineType` 收敛为 Qdrant 单变体（Mock/Disabled 枚举清除） | 已完成 | `crates/vector-client/src/config/client.rs:7` |
| 配置 `[vector] engine = local/qdrant` + `[vector.local]` 配置段 | 已完成 | `crates/graphdb-config/src/config.rs:205-247` |
| server/embedded 启动按配置分派双后端 | 已完成 | `crates/graphdb-server/src/startup.rs:84-136` |

编译验证（本方案撰写时实测通过）：`--features vector`、`--features vector-qdrant`、
`-p graphdb-sync --no-default-features --features vector` 三种组合均编译干净。

### 1.2 清单第二项：文档更新 —— 未完成

| 文档 | 现状 |
|------|------|
| `docs/vector/README.md` | 索引仍指向旧五文档结构，正文引导读者阅读 MockEngine 实现，全部失效 |
| `docs/vector/vector-engine-design.md` | 顶部已有 Phase A/B 补充说明，但正文仍是 VectorEngine trait / QdrantEngine / MockEngine 时代的架构描述 |
| `docs/vector/testing-guide.md` | 完全过时：以 `VectorClientConfig::mock()` + `VectorManager` 为中心的测试流程 |
| `docs/vector/implementation-checklist.md` | 完全过时：未勾选的 VectorEngine trait 实现清单，已被 vector-search 实际实现取代 |
| `docs/vector/benchmark-baseline.md` | 现行有效（Phase A 基线数据） |
| `docs/archive/local-vector-engine.md` | **未标注被取代**（旧 arroy 方案，总方案 §2 已否决该路线） |

### 1.3 清单第三项：CI 覆盖内置路径全链路 —— 未开始

**仓库当前没有任何 CI 配置**（无 `.github/workflows/`、无 GitLab/Jenkins 配置；
`crates/tantivy/.github` 属 vendored 第三方代码，不作为依据）。

现有本地链路测试资产按层盘点：

| 层 | 资产 | 覆盖内容 |
|----|------|---------|
| 引擎内核 | `crates/vector-search/tests/{storage,recovery,search,ivf}_test.rs` + 单元测试 | mmap 存储、WAL 回放、SIMD 核对照、过滤、IVF |
| coordinator | `crates/graphdb-sync/tests/vector_local_backend.rs`（9 例） | 本地后端建索引 / 缓冲提交 / WAL 幂等 / 搜索 / 删除传播 |
| 服务 API | `tests/vector_local_startup_e2e.rs` | GraphService 启动 + VectorApi 全往返（含重启一致性） |
| 查询语句 | **缺失** | `SEARCH VECTOR` 等 Cypher 向量语句经 parser → executor → operator → coordinator → 本地引擎的全链路无用例 |
| 远程路径 | `tests/sync/{vector_sync,vector_transaction}.rs`（`#![cfg(feature = "vector-qdrant")]`） | qdrant 后端缓冲/事务语义（disabled engine 即可跑，无需真实 Qdrant） |

盘点中发现的一个关键事实（影响 C6 决策）：**图写入 → 向量索引的自动传播在
生产侧不存在**。coordinator 的入口 `buffer_vector_change` /
`commit_transaction` 目前仅被测试调用；qdrant 时代从顶点属性抽取向量的
`SyncManager::apply_vector_mutation`（manager.rs:684）挂在
`#[cfg(feature = "vector-qdrant")]` 下且依赖 outbox/receiver 机制，而其输入——
target="vector" 的 `IndexMutation`——在生产代码中**没有任何产生方**
（transaction/storage 层只生成 target="fulltext"/"native-index"）。即两条路径下
向量集合目前都只能经 VectorApi 显式维护。该缺口的处置见 §2-C6。

---

## 2. 设计决策定案

### C1：CollectionConfig 的 HNSW 残留构造

**现状**：两处调用点无条件以 HNSW 构造集合配置——
`crates/graphdb-api/src/api/core/vector_api.rs:79-88` 与
`crates/graphdb-sync/src/sync/vector_sync.rs:480`。而本地引擎完全不消费
`index_type`/`hnsw_config`（`vector-search` 的 `engine.rs`/`storage/meta.rs`
均无引用，`meta.bin` 也不含它们）；本地层级实际由 `[vector.local.ivf]` +
promotion 机制控制。

**风险**：语义误导。阅读者会以为本地引擎使用 HNSW；未来若有人按
`config.index_type` 写分支逻辑会引入隐性 bug。

**决策**：
- 类型字段**保留**：Qdrant 建集合确实需要 hnsw 参数，删除会破坏远程路径；
- 调用点改为**按后端条件构造**：`backend.is_local()` 时用裸
  `CollectionConfig::new(vector_size, distance)`；否则追加
  `.with_hnsw(HnswConfig::new(16, 100).with_payload_m(16))`；
- 不采用"本地引擎继续静默忽略"的现状：配置应反映其对目标后端是否有效，
  这也是总方案"pgvector 思路（层级由引擎自身机制控制）"的自然推论。

### C2：根 crate dev-dependencies 无条件拉 vector-client

**现状**：根 `Cargo.toml:219` 在 `[dev-dependencies]` 无条件声明
vector-client。而唯一使用它的是三个 `#[cfg(feature = "vector-qdrant")]`
门控的位置（`tests/sync/vector_sync.rs`、`tests/sync/vector_transaction.rs`、
`src/test_utils/sync_helpers.rs` 的两个函数）。后果：任何 `cargo test`
（包括无向量的 CI leg）都编译 tonic/prost/reqwest——与总方案 §5
"`vector` 默认开启且不拉 tonic/prost/reqwest"的精神冲突。Cargo 无法按
feature 门控 dev-deps，故只能消除对它的直接引用。

**决策**：移除根 dev-dependency，类型经 graphdb-sync 公开再导出获取：
- `graphdb-sync` 在 `sync.rs` 增加
  `#[cfg(feature = "vector-qdrant")] pub use vector_client::{VectorClientConfig, VectorManager};`，
  在 `vector_sync.rs` 现有再导出行（:19）扩展 `{DistanceMetric, PointId,
  SearchQuery, SearchResult, VectorPoint}`；
- 上述三处引用改为 `graphdb::sync::...` 路径；
- 理由：根 crate 对外只暴露 `graphdb-*` 面，vector-client 是 sync 层的可选
  实现细节，不应泄漏到根测试的 import 面；改动为机械替换，风险低。

**否决的备选**：(a) 接受现状并记录——把成本转嫁给所有后续 CI leg，不可取；
(b) 把 qdrant-only 测试文件搬进 `crates/graphdb-sync/tests/`——同样需要
dev-dep（Cargo 限制相同），却破坏了根集成测试的完整性，纯增加搬运成本。

### C3：test_utils 过时 helper

**现状**：`with_vector()` 注释声称"需要外部 vector_client 设置"，实现是空壳；
`create_tag_with_vector`/`search_vector` 门控 `vector-qdrant` 且直接引用
`vector_client::` 类型。

**决策**：
- 两个 helper 的门控**放宽为 `feature = "vector"`**：它们内部走的是
  coordinator 的 `create_vector_index`/`search`——这两个 API 经
  `VectorBackend` 分派，本就后端无关，本地后端可直接工作；
- 类型引用按 C2 改道（`graphdb::sync::vector_sync::{DistanceMetric,
  SearchQuery, SearchResult}`）;
- `with_vector()` 注释更正为内置本地引擎语义，行为不变。

### C4：文档处置矩阵

原则：不整篇重写仍有参考价值的正文；失效流程文档归档而非删除（项目惯例，
参照 `docs/archive/` 现状）；所有被移动/重定位的文档更新反向引用。

| 文档 | 处置 | 动作 |
|------|------|------|
| `archive/local-vector-engine.md` | 加横幅 | 顶部标注：arroy 方案已被 `docs/plan/vector_local_engine_plan.md` 取代，否决理由见其 §0/§2 |
| `docs/vector/testing-guide.md` | 归档 | `git mv` 至 `docs/archive/vector-testing-guide.md` + 取代横幅（描述 Mock/Qdrant 时代流程） |
| `docs/vector/implementation-checklist.md` | 归档 | `git mv` 至 `docs/archive/vector-implementation-checklist.md` + 取代横幅（trait 清单已由 vector-search 实现落地） |
| `docs/vector/vector-engine-design.md` | 重定位 | 头部改写：定位为"外部 Qdrant 适配层设计参考"；Mock/QdrantEngine 章节标注"历史参考"；指向现行方案文档 |
| `docs/vector/benchmark-baseline.md` | 保留 | 现行有效 |
| `docs/vector/README.md` | 重写 | 新索引：① 内置引擎（现行）——plan 三部曲 + `crates/vector-search` + 配置入口 `[vector.local]`；② 外部适配——vector-engine-design + vector-client；③ bench 基线；④ 归档链接 |
| 总方案 `vector_local_engine_plan.md` | 修订 | 头部对 implementation-checklist 的引用路径更新；Phase C 清单按本文执行后勾选 |

### C5：CI 选型与结构

**决策**：GitHub Actions（`.github/workflows/ci.yml`），四 job：

```yaml
name: ci
on:
  push: { branches: [main] }
  pull_request:

jobs:
  lint:            # rustfmt --check + clippy --workspace --all-targets -D warnings
  test-default:    # cargo test --workspace（默认 feature = server + vector）
                   # ← 内置路径全链路主战场：engine 单测/集成、sync 本地后端、
                   #    VectorApi e2e、查询语句 e2e（§3.4）
  feature-matrix:  # strategy.matrix:
                   #   no-vector : cargo test --no-default-features --features server
                   #   qdrant    : cargo test --features vector-qdrant
                   #   sync-local: cargo test -p graphdb-sync --no-default-features --features vector
  bench-build:     # cargo bench -p vector-search --no-run（保障 benches 可编译，不在 CI 计时）
```

要点：
- runner 兼容性：ubuntu-latest CPU 支持 x86-64-v3（AVX2），`.cargo/config.toml`
  的 `-C target-cpu=x86-64-v3` 无需调整；SIMD 对照测试本身即运行时验证；
- qdrant leg 使用 `VectorClientConfig::disabled()`，不需要真实 Qdrant 服务；
- 缓存用 `Swatinem/rust-cache@v2`；toolchain 固定 1.88（AGENTS.md 前提）；
- clippy 若存在存量告警，在本阶段一并清零（此前已有 "fix all dead code"
  专项，预计残留少），之后 CI 以 `-D warnings` 锁住。

### C6："全链路"范围界定与图写入传播缺口

**决策一（本期范围）**：Phase C 的"全链路"定义为
**查询语句 / VectorApi → VectorSyncCoordinator → vector-search 本地引擎**，
即用户可见的两条向量访问入口端到端可验证。补齐方式：新增
`tests/vector_query_e2e.rs`（§3.4），覆盖现有四层资产缺失的最后一层。

**决策二（登记不做）**：图写入（顶点携带 vector 属性提交）→ 向量集合的
自动传播**本期不实施**，理由：
- 该链路的产生侧不存在：transaction/storage 层从不生成 target="vector" 的
  `OutboxIntent`/`IndexMutation`，补齐需动 transaction WAL filter 与 sync
  manager 分派，是一个独立特性（涉及幂等、late-arrival、失败语义），远超
  收尾阶段的边界；
- 总方案 §4.1 已定案"同步机制内置路径不接线"，自动传播若做也应走
  coordinator 直连而非复活 outbox，需要单独评审；
- 现状语义（VectorApi 显式维护）自洽可用，先以文档明确登记，避免静默缺口。

**产出物**：在总方案 §9（明确不做）或本文档登记该后续项，含缺口描述
（§1.3 末段），供后续立计划。

---

## 3. 完整代码修改方案

### 3.1 类型路径与调用点清理（C1/C2/C3）

```
Cargo.toml                                        修改  删除 [dev-dependencies].vector-client（:219）
crates/graphdb-sync/src/sync.rs                   修改  #[cfg(vector-qdrant)] pub use vector_client::{VectorClientConfig, VectorManager}
crates/graphdb-sync/src/sync/vector_sync.rs       修改  再导出扩展 SearchQuery/SearchResult（:19）
                                                        create_vector_index 配置构造按 backend.is_local() 条件化（:480）
crates/graphdb-api/src/api/core/vector_api.rs     修改  create_index 回退分支同上（:77-90）
src/test_utils/sync_helpers.rs                    修改  两处门控 vector-qdrant → vector；类型改道 graphdb::sync::...
                                                        with_vector() 注释更正
tests/sync/vector_sync.rs                         修改  use vector_client::... → use graphdb::sync::...
tests/sync/vector_transaction.rs                  修改  同上
```

验证命令（每步后跑）：

```shell
cargo check --workspace                        # 默认 feature
cargo check --workspace --features vector-qdrant
cargo test --no-default-features --features server   # 无向量全量回归
```

### 3.2 文档处置（C4）

按 §2-C4 矩阵执行；全部用 `git mv` 保留历史。横幅格式统一：

```markdown
> **[已废弃 / 已取代]** 本文描述的 xxx 方案已被
> `docs/plan/vector_local_engine_plan.md`（及 phase_b/phase_c 系列）取代，
> 仅作历史参考。
```

完成后全文检索 `docs/vector/testing-guide|implementation-checklist` 引用并修正。

### 3.3 CI 工作流（C5）

新建 `.github/workflows/ci.yml`，骨架见 §2-C5。实施顺序：
1. 先本地跑通 `cargo fmt --all -- --check` 与
   `cargo clippy --workspace --all-targets -- -D warnings`，清零存量告警；
2. 落 yaml，确认四个 job 在 PR 上全绿；
3. 故意注入一个 SIMD 断言失败验证 test-default 能拦截（一次性演练，不入库）。

### 3.4 查询层 e2e（C6）

新增 `tests/vector_query_e2e.rs`（复用 `tests/vector_local_startup_e2e.rs` 的
GraphService 启动模式 + `GraphService::execute(session_id, stmt)` 入口）：

用例组：
1. **DDL→DML→查询往返**：建 space/tag（vector 属性列）→ `CREATE VECTOR INDEX`
   → 经 VectorApi 批量插入点 → `SEARCH VECTOR <collection> WITH [<vec>] LIMIT k`
   → 断言 top-k 顺序与得分；
2. **过滤 + LIMIT**：带 payload 的点集 + `WHERE group_id = ...` 后过滤语义断言；
3. **删除可见性**：`delete_vector` 后同语句结果收敛；
4. **DROP INDEX 后报错语义**：查询返回明确错误而非 panic/空结果。

注意：语句级入口与 VectorApi 入口共享同一 coordinator 实例，用例 1 中两种
入口混插即可顺带验证会话/权限层不吞向量上下文。

---

## 4. 验收标准

- [ ] 根 `[dev-dependencies]` 不再包含 vector-client；`cargo tree` 确认默认
      测试构建不含 tonic/prost/reqwest；
- [ ] 三种 feature 组合编译 + 测试通过（§3.1 验证命令）；
- [ ] `grep -rn "IndexType::HNSW" crates/` 仅剩 qdrant 条件分支内出现；
- [ ] `docs/vector/README.md` 为新索引；归档文档有取代横幅；无死链；
- [ ] `.github/workflows/ci.yml` 四 job 全绿一次完整运行记录；
- [ ] `tests/vector_query_e2e.rs` 至少覆盖 §3.4 用例组 1~3；
- [ ] 总方案 Phase C 清单勾选，头部引用路径更新；C6 后续项已登记。

## 5. 工作量估算

| 内容 | 估时 |
|------|------|
| C1~C3 代码清理 + 回归 | 0.5 天 |
| C4 文档处置 | 0.5 天 |
| C5 CI 搭建 + 存量告警清零 | 0.5~1 天 |
| C6 查询层 e2e | 0.5 天 |
| 合计 | 约 2 天 |
