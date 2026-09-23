# 顶点设计剩余问题一次性清理清单

本文是对 `docs/analysis/vertex-design-comparison.md` 六项结论中尚未根本解决部分的完整施工清单。
目标是下一阶段一次性完成全部修改，不留半成品。只用自然语言描述现状、终态与范围，不含完整代码片段。
行号以当前工作树为基线，实施前若树有变化需重新核对。

## 一、总则

开发阶段无任何旧数据，不保留任何向后兼容、迁移修复、静默回退分支。
生产代码禁止 `unwrap`、`expect`、`panic`（含断言式构造器），所有失败经 `Result` 或 `Option` 按函数契约正常传播。
测试代码允许 `expect`，沿用仓库现有约定。
只修改原模块，不新增替代模块。实施合并为尽量少的一次编译测试批次，并附格式化检查与零残留门禁。

## 二、当前树上的半成品（下一阶段必须返工或验证）

此前分阶段改动已落入工作树但未经编译验证，且其中部分违反总则，必须在一次性实施中处理：

1. 列存路由修复（`vertex/column.rs` 判定改全枚举否定、`variable_width.rs` 读写扩展、`column.rs` 与 `column/column.rs` 注释、`column/tests.rs` 新增用例）。方向正确，留待编译验证，无需推倒。
2. 存活枚举收敛（`vertex_table/core.rs`、`sharded/read.rs`、`cursor_impl/vertex.rs`、`serial.rs`、`core_tests.rs`、`id_indexer.rs` 注释）。方向正确，留待编译验证。
3. 标识构造器收紧（`storage_ids.rs` 中 `from_int64` 判负、`from_string` 判超长）。其失败形态是断言，违反生产代码禁 `panic` 总则，下一阶段必须改为删除旁路构造器、调用方全部走 `try_` 并传播错误。
4. 读路径回放的 `expect`（`vertex_table/core.rs` 三处索引键回放、`cursor_impl/edge.rs` 的 `make_vid`）。违反禁 `expect` 总则，必须改为正常错误传播。
5. 索引入口的 `try_` 化（`index/helpers.rs`、`index/edge_index_manager.rs`、`index/vertex_index_manager.rs`）。方向正确，留待编译验证。
6. 缓存回放的 `?` 化（`context/vertex_ops/resolve.rs`）。缓存缺失后重读是正常逻辑，予以保留。
7. `storage_ids.rs` 单测中负数排序用例已改走显式类型字节构造，予以保留。

## 三、A 组：顶点标识构造器与解析回退

终态是标识只有一种合法来源：边界校验，内部只有一种失败形态：正常返回错误。无截断、无负数旁路、无空标识伪造、无回退参数。

需要清理与替换的代码：

1. `graphdb-core/src/types/storage_ids.rs` 的 `from_int64`（判负旁路）与 `from_string`（截断或断言）两个非受检构造器直接删除，仅保留 `try_from_int64`、`try_from_string`、`from_u64`、`from_typed_bytes`、`normalize_for_vid_type`。`From<i64>` 实现随 `from_int64` 一并删除，调用方改显式 `try_`。
2. `from_int64` 的全部生产调用方改为 `try_from_int64` 并传播错误，重点是用户输入可达的三处索引入口（`storage/src/index/helpers.rs` 的顶点引用构造、`storage/src/index/edge_index_manager.rs` 的值到顶点标识转换、`storage/src/index/vertex_index_manager.rs` 的顶点标识转换），负数与超长一律映射为无匹配而非伪造标识。其余调用方（边端点编解码、内部行号转换、事务日志回放、同步链路的已校验标识）同样改显式 `try_`。
3. `storage/src/engine/graph_storage/ops.rs` 的 `decode_external_str_or_record` 删除回退参数，改为返回 `Option`，调用方边记录转边函数同步改为 `Option` 并向上传播，不再用存储记录补画端点。
4. `storage/src/engine/graph_storage/reader/utils.rs` 的 `vid_from_str` 改为返回 `Option`，`internal_to_external_vertex_id` 删除回退参数并返回 `Option`，其在 `reader/edge.rs` 的四个端点解析调用方按缺失端点正常处理，不再回解空标识。
5. `storage/src/engine/graph_storage/cursor_impl/edge.rs` 的 `make_vid` 删除，边物化直接使用校验后的标识转换并传播错误。
6. `storage/src/engine/graph_storage/context/vertex_ops/read.rs` 的索引键回放分支改显式 `try_` 并传播错误，不再使用截断构造。
7. `storage/src/engine/graph_storage/context/vertex_ops/resolve.rs` 的缓存回放保持 `Option` 传播（缺失即回表重读），解析失败不再回退旧值。
8. `storage/src/vertex/vertex_table/core.rs` 的三处索引键回放（批量投影、有效标识解析、单行投影）改显式 `try_` 并按函数契约处理（`Option` 函数返回 `None`，不得伪造行）。

## 四、B 组：顶点单标签类型重塑

终态是接口类型只能表达单标签：一个顶点即一个标签名加一份属性映射。无标签数组、无顶点级属性映射、无多标签辅助方法、无跨映射回退。运行时单标签门控随之删除（类型上已不可能）。

需要清理与替换的代码：

1. `graphdb-core/src/vertex_edge_path.rs` 的 `Vertex` 结构体：删除标签数组字段与顶点级属性映射字段，改为单个标签字段；删除 `new` 的多标签签名、`with_vid` 的无标签构造、`new_with_properties` 的双映射签名、`add_tag`、`has_tag`、`get_tag`、`get_property` 的跨标签查找、`property_value` 的跨映射回退、`get_all_properties` 的合并语义、`vertex_properties`、`set_vertex_property`、`remove_vertex_property`、`tag_count`、`has_properties`、`cmp_tags_and_properties` 中的多标签分支。保留 `Tag` 结构体作为单个标签的载体。内部行号字段同步决策去留，见第六节待决策事项。
2. 存储层构造点全部改为单标签构造：`storage/src/vertex.rs` 的记录转顶点与按标签构造、`storage/src/engine/graph_storage/ops.rs` 的记录转顶点、`storage/src/engine/graph_storage/reader/vertex.rs` 的点查与扫描构造、`storage/src/engine/graph_storage/cursor_impl/vertex.rs` 的行批与列批组装、`storage/src/client/import_export.rs` 的导入组装。
3. 存储层写入门控删除：`storage/src/engine/graph_storage/writer/vertex.rs` 的单标签检查函数及其在插入、更新、批量插入中的调用全部删除（类型已保证）；批量路径中的按标签计数与串行列扫描改为单标签直取。
4. 同步写入链路的多标签循环改单标签：`storage/src/engine/sync_wrapper/write.rs` 的标签扇出、`storage/src/engine/sync_wrapper/write_vertex.rs` 的新旧标签对照与多标签循环。
5. 查询层多标签语义收敛：`graphdb-query/src/parser/ast/stmt/dml.rs` 与 `dml_parser.rs` 的多标签插入语法、`planning/statements/dml/insert_planner.rs` 与 `create_planner.rs` 与 `merge_planner.rs` 的多标签规划、`binder/bound.rs` 的插入绑定结构、`executor/streaming/operators/sink_operator.rs` 的多标签顶点组装、`executor/streaming/operators/source_operator/util.rs` 的无标签顶点构造、`executor/streaming/helpers/conversion.rs` 与 `executor/expression/functions/builtin/graph.rs` 与 `planning/statements/seeks` 各检索器的全属性合并读取、`planning/statements/paths/shortest_path_planner.rs` 的属性合并、`pipeline.rs` 与 `pipeline/compiler.rs` 的顶点管线组装。插入语法本身去留见第六节待决策事项。
6. 服务与传输层适配：`graphdb-server/src/batch/manager.rs` 的多标签组装与 `new_with_properties` 调用、`graphdb-api/src/embedded/c_api/batch.rs` 的标签列表组装、`graphdb-wire/src/batch.rs` 的标签数组字段、`graphdb-cli` 的导出导入标签字段、`graphdb-migration/src/executor/data_apply.rs` 的标签合并函数、`graphdb-fulltext/src/manager.rs` 的标签存在检查与索引文本抽取、`graphdb-api/src/api_core/rebuild_source.rs` 的标签查找。

## 五、C 组：删除路径收敛

终态是删除按标签路由，无全表扇出，无多标签删除入口。无标签的删除请求在入口即被拒绝，不进入存储层。

需要清理与替换的代码：

1. `storage/src/engine/graph_storage/writer/vertex.rs` 的跨表属主探测函数删除，删除入口改为要求标签，未带标签的调用直接返回非法输入错误。
2. 同文件的多标签删除函数删除，与之配套的 `writer_api.rs`、`sync_wrapper/write.rs`、`metrics.rs`、`client/mod.rs`、`test_mock.rs` 中的同名透传一并删除。
3. 查询层删除标签计划节点族删除：`planning` 下的删除标签节点定义、枚举注册、访问器、物理规划转换、启发式优化器分支、`executor/streaming/operators/sink_operator.rs` 中的执行分支。语法层面的移除标签语句同步删除，见第六节待决策事项。
4. `storage/src/engine/graph_storage/reader/utils.rs` 的跨表外部标识查找与 `reader/edge.rs`、`reader/index_ops.rs`、`cursor_impl/edge.rs` 中的跨表回退调用改为按标签直查。

## 六、D 组与 E 组：已完成方向的验证清单

存活枚举与列存路由的改动方向已定，下一阶段只需编译验证，不再返工：

1. 存活枚举唯一入口为带时间戳版本，无参数版本已删除；游标、串行列扫描、迭代器均已切换；用例覆盖已删行过滤。
2. 定宽变宽判定已改为全枚举否定式，无零步长定宽列；变宽读写已覆盖固定字符串、向量稠密稀疏、列表映射集合、数据集、图值、区间、数值扩展类型；画像与编码选择对非标量类型保持无编码，接受为文档化局限而非缺陷。

## 七、F 组：主键镜像旧数据修复分支删除

`storage/src/vertex/vertex_table/core.rs` 的主键镜像修复函数整体删除，`storage/src/vertex/vertex_table/persistence/load.rs` 中的加载时调用一并删除。
写入时自动填充与不一致拒绝、更新时禁止主键列的逻辑保留，那是正常校验而非旧数据兼容。

## 八、错误处理规约

生产代码不得出现 `unwrap`、`expect`、`panic`、`unreachable` 与断言式构造器。
边界入口用 `try_` 构造并返回 `StorageError`；索引与查询的可选映射用 `Option` 的无匹配表达非法输入；
只读回放遇到损坏按外层函数契约返回 `None` 或 `Err`，不得伪造空标识、不得截断、不得回退旧值。
验收时以内容搜索门禁为准：非测试代码中上述四类调用残留必须为零。

## 九、决策事项（已拍板）

1. 多标签插入语法：删除，语法层只接受单标签。
2. 移除标签语句与 `delete_tags`：整体删除，只保留删顶点。
3. 顶点内部行号字段：完全删除。
4. 无标签点查与删边级联：调用方必须显式带标签，缺失即非法输入。

## 十、实施与验收

按 A 到 F 组顺序在同一阶段内完成，合并为一次编译测试批次：核心标识单测、列存储单测、顶点表与分片表单测、存储引擎测试、事务测试、查询层回归，全程附带格式化检查。
验收除测试通过外，还包括三项搜索门禁：非测试代码无 `unwrap` 与 `expect` 与 `panic` 与 `unreachable`；
无 `from_int64` 与 `from_string` 的非受检构造残留；无标签数组、顶点级属性映射、跨表探测、回退参数的残留引用。
每个阶段性文档（列存注释、顶点表注释）随代码同步更新。
