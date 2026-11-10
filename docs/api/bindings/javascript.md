# JavaScript 绑定

目录 `bindings/js`，包名 `linkrs`，基于 `node:ffi`（Node 22+ 实验特性），
零依赖，只发布纯 JavaScript 代码。

## 前置要求

```shell
cargo build --features c_api
export LINKRS_LIB=/path/to/liblinkrs.so   # 可选，默认搜 target/debug|release
node --experimental-ffi ...
```

`src/native.js` 定位规则与 Python 一致；找不到抛 `LinkrsError` 并列出尝试过的路径。

## 用法

```js
import { Database } from "linkrs";

using db = Database.openMemory();
db.createSpace("g");
using session = db.session();
session.useSpace("g");
using result = session.execute("SHOW SPACES");
console.log(result.rows());
using txn = session.begin();
txn.execute("SHOW SPACES");
txn.commit();
```

所有句柄类支持显式资源管理（`using` 声明）与可重复调用的 `close()`，
并暴露 `closed` 属性。

## API 一览（`src/database.js`）

- `Config`：`create/forFile/memory/close`。
- `Database`：`open/openMemory/openWithConfig/close`、错误码/文本/版本、
  `session`、空间/标签/边类型增删查（`createSpace/dropSpace/listSpaces/getSpace` 等，
  属性定义走 JSON）。
- `Session`：`useSpace/currentSpace`、`execute/executeParams`、
  `validateQuery/explainQuery`、`statistics`、`begin/beginReadonly`。
- `Transaction`：`execute/executeParams`、`commit/rollback`、
  `savepoint/releaseSavepoint/rollbackToSavepoint`。
- `Result`：`columnCount/rowCount/columnNames`、`rows/row(index)`。

查询参数支持空、布尔、整数、浮点、字符串、BigInt（`src/errors.js` 同 Python 的
七组具名划分）。

## 测试

```shell
cd bindings/js && npm test
```
