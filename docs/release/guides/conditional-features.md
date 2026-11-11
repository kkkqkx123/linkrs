# 条件编译特性与缺失期行为说明

特性分两层，正交生效：编译期特性决定有没有这份代码与依赖，
运行期配置决定开不开。特性开了、配置没开，等于没开；
配置开了、特性没编，调用点必报错——两种"没开"都不会静默成功。

## 特性一览

| 特性 | 拉入内容 | 默认 |
|---|---|---|
| `embedded` | Rust 嵌入 API（`GraphDatabase` 等） | 关（服务端默认开 `server`+`vector`） |
| `c_api` | C FFI 符号，隐含 `embedded` | 关 |
| `fulltext` | Tantivy 后端 + 全文同步/执行链 | 关（默认构建不含，包体积原因） |
| `vector` | 本地向量引擎 + 向量同步/执行链 | 开（在默认组合里） |
| `vector-qdrant` | 远端 Qdrant 客户端（含 `vector`） | 关 |
| `migration` | 在线迁移编排（计划生成/围栏执行/回滚） | 关；服务端无条件启用 |
| `embedding` / `rerank` | 向量 embedding / 重排基础设施 | 关 |
| `grpc` / `jieba` | gRPC 层 / 中文分词 | 关 |

`fulltext`、`vector`、`migration` 在 api/query/sync 三层之间透传：
顶层开一个特性，执行链各段一起开，不会出现"解析认得、执行没编"
之外的中间态。特性在整个工作区是加法叠加的，任何一处开启即全局生效。

## 组合速查

- 纯图嵌入（Rust）：`--no-default-features --features embedded`
- 加全文：另加 `fulltext`，并把 `FulltextConfig.enabled` 置 true
- 加本地向量：另加 `vector`（默认组合已含），内存库需另配向量目录
- 加远端向量：`vector-qdrant`（已含 `vector`）
- 加迁移：另加 `migration`
- C 嵌入：在上述组合上另加 `c_api`（符号层，不改变后端有无）
- 服务端：默认特性即可；`migration` 不用加，服务端侧恒开

## 缺失期行为

原则：解析/绑定/计划三层永远可用（语法面不随特性消失），
缺的只在执行或调用点以明确错误暴露。

| 缺的是 | Rust API | C API |
|---|---|---|
| `fulltext` 未编 | 全文语句照常解析，执行报 `feature_disabled("fulltext", …)`，线 extended 码为语义错误 | 符号存在，返回 `LINKRS_NOT_IMPLEMENTED`，`errmsg` 写明未编译 |
| `vector` 未编 | 同上（`feature_disabled("vector", …)`）；`search_vector` 等会话方法编译期不存在 | 同上（`LINKRS_NOT_IMPLEMENTED`） |
| `migration` 未编 | 计划/执行/回滚方法编译期不存在，加特性即恢复 | 符号存在，返回空指针，`errmsg` 写明未编译 |
| `c_api` 未编 | 无影响（Rust 路径不走 FFI） | 符号不存在，链接期即失败——这是唯一的编译期缺失 |

配置开了但特性没编，走上面同一套错误，不会退化成空结果。
特性编了但配置没开（如全文默认 `enabled = false`、
内存库没配向量目录）：建库成功，首次使用时报
"coordinator/sync manager 不可用"类错误，同样不静默。

## 反模式

- 开了特性就认为功能可用：全文/向量还要显式启用配置并建好索引，
  默认全是关闭态，查不到数据时先看配置而非怀疑裁剪。
- 靠"返回空"判断后端缺失：本项目缺失一律是错误（执行错/未实现码/
  编译错），空结果只代表"查无数据"，两者不要混用。
- C API 符号存在即认为后端存在：符号层与后端是两回事，
  以返回码和 `errmsg` 为准。
- 为省事给嵌入包全开特性：嵌入闭包按需组装是刻意设计，
  不用的后端不要编译进来，有问题看 `cargo tree -e normal` 验证。
