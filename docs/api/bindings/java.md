# Java 绑定

目录 `bindings/java`（Maven，`io.linkrs`），JNI 薄桥接，
只发布纯 Java 代码，需 C 工具链与 JDK。

## 前置要求

```shell
cargo build --features c_api
sh bindings/java/scripts/build-native.sh
```

桥接加载器（`NativeLib`）按系统属性（`-Dlinkrs.jni.lib=<path>`）、
环境变量（`LINKRS_JNI_LIB`）、工作区构建产物、系统库路径的顺序定位桥接库，
并预加载 `LINKRS_LIB` 指定的 Rust 动态库；
绑定、桥接库、动态库三者保持同一检出。

## 用法

```java
import io.linkrs.Database;
import io.linkrs.Result;
import io.linkrs.Session;
import io.linkrs.Transaction;

try (Database db = Database.openMemory()) {
    db.createSpace("g");
    try (Session session = db.session()) {
        session.useSpace("g");
        try (Result result = session.execute("SHOW SPACES")) {
            System.out.println(result.rows());
        }
        try (Transaction txn = session.begin()) {
            txn.execute("SHOW SPACES");
            txn.commit();
        }
    }
}
```

文件库 `Database.open(path)`，配置打开以 `Config.create()` /
`Config.forFile(path)` / `Config.memory()` 配合 `Database.openWithConfig(config)`。
所有句柄类实现 `AutoCloseable`，支持 try-with-resources 与可重复关闭。

## API 一览

- `Config`：`create/forFile/memory/close`。
- `Database`：`open/openMemory/openWithConfig/close`、错误码/文本/版本、
  `session`、空间/标签/边类型增删查。
- `Session`：`useSpace/currentSpace`、`execute/executeParams`、
  `validateQuery/explainQuery`、`statistics`、`begin/beginReadonly`。
- `Transaction`：`execute/executeParams`、`commit/rollback`、
  `savepoint/releaseSavepoint/rollbackToSavepoint`。
- `Result`（`Result.java`）：`columnCount/rowCount/columnNames`、`rows/row`。
- 错误：`LinkrsError` 基类与
  `BusyError/ReadonlyError/NotFoundError/SchemaError/ConstraintError/TypeMismatchError/MisuseError`
 （`Json` 工具类负责信封解析）。
- JNI 桥接（`src/main/c/linkrs_jni.c`）：`nOpen/nOpenMemory/nOpenWithConfig/nClose`、
  配置、会话/空间/统计/错误偏移与扩展码、
  执行与参数化执行、结果释放与行列/JSON、`Txn*` 全套、模式增删查。

## 测试

```shell
cd bindings/java && mvn -o test       # 需先 sh scripts/build-native.sh
```
