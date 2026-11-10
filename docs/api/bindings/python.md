# Python 绑定

目录 `bindings/python`，包名 `linkrs`，基于标准库 `ctypes`，
无需编译器，只发布纯 Python 代码。

## 前置要求

```shell
cargo build --features c_api
export LINKRS_LIB=/path/to/liblinkrs.so   # 可选，默认搜 target/debug|release
```

`linkrs/_native.py` 按 `LINKRS_LIB`、工作区 `target/debug|release` 下的
`liblinkrs.so/.dylib/.dll` 顺序定位；找不到抛 `LinkrsError` 并列出尝试过的路径。

## 用法

```python
from linkrs import Database

with Database.open_memory() as db:
    db.create_space("g")
    with db.session() as session:
        session.use_space("g")
        with session.execute("SHOW SPACES") as result:
            print(result.rows())
        with session.begin() as txn:
            txn.execute("SHOW SPACES")
```

文件库 `Database.open(path)`，配置打开以 `Config.memory()` /
`Config.for_file(path)` / `Config.new()` 配合 `Database.open_with_config(config)`。

## API 一览（`linkrs/database.py`）

- `Config`：`new/for_file/memory/close`（上下文管理）。
- `Database`：`open/open_memory/open_with_config/close`、`errcode/last_message/libversion`、
  `session`、`create_space/drop_space/list_spaces/get_space`、
  `create_tag/drop_tag/list_tags/get_tag`、
  `create_edge_type/drop_edge_type/list_edge_types/get_edge_type`。
- `Session`：`use_space/current_space`、`execute/execute_params`、
  `validate_query/explain_query`、`statistics`、`begin/begin_readonly`。
- `Transaction`：`execute/execute_params`、`commit/rollback`、
  `savepoint/release_savepoint/rollback_to_savepoint`（异常退出 `with` 自动回滚）。
- `Result`：`column_count/row_count/column_names`、`rows/row(index)`。

查询参数仅空、布尔、整数、浮点、字符串（`_pack_values`）。

## 错误（`linkrs/errors.py`）

`LinkrsError` 基类（保留数字码）与
`BusyError/ReadonlyError/NotFoundError/SchemaError/ConstraintError/TypeMismatchError/MisuseError`
七组具名，扩展码与错误位置为异常属性。

## 测试

```shell
cd bindings/python && python -m unittest discover -s tests
```
