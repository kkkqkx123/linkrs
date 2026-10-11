# P1 [fulltext] 编译依赖漂移 + index_build 零缩放（可疑）

## 现象（实测）

`linkrs-fulltext` bench（`--features fulltext`，沙箱本地补丁后跑通）：

| bench | 中位时间 |
|---|---:|
| `fulltext_index_build/100` | [30.639, 30.892, 31.129] ms |
| `fulltext_index_build/500` | [31.135, 31.362, 31.635] ms |
| `fulltext_search/search_rust` | [1.1118, 1.1192, 1.1269] µs |
| `fulltext_search/search_database_performance` | [3.5296, 3.5452, 3.5632] µs |
| `fulltext_search/search_search_query` | [3.0090, 3.0258, 3.0461] µs |

数据来源：`bench_results/linkrs-fulltext-full.log:48-100`

- **index_build/100 与 /500 均 ~31ms**：数据量 100→500（5×）但构建时间几乎不变（31.1→31.4ms）——**零缩放 / 固定开销主导**，需解释。
- search 三类均健康（1.1–3.5µs 量级）。

## 基础设施问题（影响复现性）

- 上游 `tantivy` 子模块**未 pin commit**，fork 默认分支移除了 `Bm25Params` 与 `IndexSettings.bm25_params` 字段；
- 沙箱为跑通 fulltext bench 做了**本地临时补丁**（`crates/linkrs-fulltext/src/tantivy_index.rs`：import 替换 + 去字段），补丁**未纳入任何 patch**；
- 根因修复应为在 `Cargo.toml` / `.gitmodules` 用 `rev` / 精确 commit **固定 tantivy 子模块**，否则 fulltext 编译随上游漂移、bench 不可复现。

## 影响

- **P1（基础设施）**：fulltext 编译依赖未固定，CI / 他人环境随时可能编译失败；bench 不可复现。
- index_build 100/500 零缩放需进一步确认：是文档数未真 5×，还是构建被固定开销（schema 构建 / 磁盘初始化）主导——当前数据无法区分。

## 建议

1. **pin 住 tantivy 子模块**（rev/commit），消除 API 漂移；
2. 排查 index_build 100/500 零缩放：确认文档数是否真 5×、构建是否受固定开销主导，必要时拆出"纯索引写入"与"初始化"两段计时；
3. 确认默认 features 下 bench 是否真正覆盖（既有 `docs/issue/fulltext-bench-defects.md` 记录默认 features 下为惰性占位）——本次 1 个 bench 跑通，需确认覆盖充分性。

## 严重级
**P1（基础设施 / 复现性）**

## 数据来源
`bench_results/linkrs-fulltext-full.log:48-100`；基础设施见 `SYNC_HOWTO.md` 与既有 `docs/issue/tantivy-bench-infra.md`
