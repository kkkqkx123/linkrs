# Linkrs 多语言绑定

嵌入式直调绑定：每个语言都是 C API 上的薄封装，复用 `crates/simvec` 绑定的约定（状态码、借用入参、库分配出参、JSON 穿越复杂值、环境变量优先的库定位、纯代码包）。

## 目录

| 语言 | 目录 | 形态 |
|---|---|---|
| Python | `bindings/python` | 标准库直调，无需编译器 |
| JavaScript | `bindings/js` | `node:ffi`，零依赖 |
| Go | `bindings/go` | cgo，需 C 工具链 |
| Java | `bindings/java` | JNI 薄桥接，需 C 工具链与 JDK |
| R | `bindings/r` | C 桥接返回码三元组，需 C 工具链 |

WASM 不在范围内：存储引擎无法装入浏览器，见设计文档。

## 能力矩阵口径

以 `include/linkrs.h` 与 `crates/linkrs-api/src/embedded/c_api` 对照为准，两侧一致由 `tests/c_api_header_consistency.rs` 保证。符号说明：有表示该层已封装可用，缺表示未封装。

### 生命周期与配置

| 能力 | C API | Python | JS | Go | Java | R |
|---|---|---|---|---|---|---|
| 打开文件库 | `linkrs_open` | 有 | 有 | 有 | 有 | 有 |
| 打开内存库 | `linkrs_open_memory` | 有 | 有 | 有 | 有 | 有 |
| 配置打开 | `linkrs_open_with_config` | 有 | 有 | 有 | 有 | 有 |
| 关闭 | `linkrs_close` | 有 | 有 | 有 | 有 | 有 |
| 配置构造 | `linkrs_config_new/file/memory/free` | 有 | 有 | 有 | 有 | 有 |
| 错误码/文本 | `linkrs_errcode/errmsg/errstr` | 有 | 有 | 有 | 有 | 有 |
| 版本 | `linkrs_libversion` | 有 | 有 | 有 | 有 | 缺 |

### 会话与查询

| 能力 | C API | Python | JS | Go | Java | R |
|---|---|---|---|---|---|---|
| 会话创建关闭 | `linkrs_session_create/close` | 有 | 有 | 有 | 有 | 有 |
| 空间切换与查询 | `use_space/current_space` | 有 | 有 | 有 | 有 | 有 |
| 直接执行 | `linkrs_execute` | 有 | 有 | 有 | 有 | 有 |
| 参数化执行 | `linkrs_execute_params` | 有 | 有 | 有 | 有 | 有 |
| 查询校验/解释 | `validate/explain_query` | 有 | 有 | 有 | 有 | 有 |
| 会话统计 | `session_get_statistics` | 有 | 有 | 有 | 有 | 有 |
| 会话变量 | `session_set/get_variable` | 缺 | 缺 | 缺 | 缺 | 缺 |

### 结果集

| 能力 | C API | Python | JS | Go | Java | R |
|---|---|---|---|---|---|---|
| 释放 | `linkrs_result_free` | 有 | 有 | 有 | 有 | 有 |
| 行列计数 | `column/row_count` | 有 | 有 | 有 | 有 | 有 |
| 列名 | `linkrs_column_name` | 有 | 有 | 有 | 有 | 有 |
| 整集导出 | `linkrs_result_to_json` | 有 | 有 | 有 | 有 | 有 |
| 单行导出 | `linkrs_row_to_json` | 有 | 有 | 有 | 有 | 有 |
| 逐格访问器 | `linkrs_get_*` 系列 | 缺 | 缺 | 缺 | 缺 | 缺 |

### 事务

| 能力 | C API | Python | JS | Go | Java | R |
|---|---|---|---|---|---|---|
| 开始/只读开始 | `txn_begin/begin_readonly` | 有 | 有 | 有 | 有 | 有 |
| 事务内执行 | `txn_execute/execute_params` | 有 | 有 | 有 | 有 | 有 |
| 提交/回滚/释放 | `commit/rollback/free` | 有 | 有 | 有 | 有 | 有 |
| 保存点 | `savepoint/release/rollback_to` | 有 | 有 | 有 | 有 | 有 |

### 模式

| 能力 | C API | Python | JS | Go | Java | R |
|---|---|---|---|---|---|---|
| 空间增删查 | `create/drop/list/get_space` | 有 | 有 | 有 | 有 | 有 |
| 标签增删查 | `create/drop/list/get_tag` | 有 | 有 | 有 | 有 | 有 |
| 边类型增删查 | `create/drop/list/get_edge_type` | 有 | 有 | 有 | 有 | 有 |

### 明确排除（首版）

函数注册与聚合、扩展装载卸载、提交/回滚/更新钩子、跟踪回调、繁忙处理器、预处理语句、批量写入器、全文/向量检索、迁移、会话变量、逐格访问器：均不在首版范围内，见 `docs/plan/linkrs-bindings-design.md` 的分层说明。

## 行数据形态

结果集整集导出是 `{columns, rows, metadata}` 信封，各绑定的取行接口直接返回其中的行列表；单行导出即单行对象。单元格值保持引擎的标签序列化形态（如 `{"String": ...}`、`{"BigInt": ...}`），绑定层不做静默类型擦除。

## 错误映射

主码决定异常大类（忙、只读、未找到、模式、约束、类型失配、误用七组具名，其余归入基类并保留数字码），扩展码与错误位置作为属性保留。五端划分一致。

## 构建与测试

```shell
cargo build --features c_api          # 产生 target/{debug,release}/liblinkrs
cargo test --test c_api_header_consistency
cd bindings/python && python -m unittest discover -s tests
cd bindings/js && npm test
cd bindings/go && go test ./...
cd bindings/java && mvn -o test       # 需先 sh scripts/build-native.sh
R CMD INSTALL bindings/r              # 需先设 LINKRS_LIB
```

所有绑定与动态库保持同一检出，跨版本混用不受支持。
