# Linkrs Java 绑定

基于 JNI 薄桥接层的嵌入式直调绑定。调用者先构建动态库与桥接库并指定路径，本包只发布纯 Java 代码；启用 C 工具链与 JDK 头文件为前置要求。

## 前置要求

```shell
cargo build --features c_api
sh bindings/java/scripts/build-native.sh
```

桥接加载器按系统属性（`-Dlinkrs.jni.lib=<path>`）、环境变量（`LINKRS_JNI_LIB`）、工作区构建产物、系统库路径的顺序定位桥接库，并预加载 `LINKRS_LIB` 指定的 Rust 动态库。调用者保持绑定、桥接库、动态库三者来自同一检出。

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

文件数据库用 `Database.open(path)`，配置文件打开用 `Config.create()`、`Config.forFile(path)`、`Config.memory()` 配合 `Database.openWithConfig(config)`。所有句柄类实现 `AutoCloseable`，支持 try-with-resources 写法与可重复关闭。

## 范围

首版覆盖数据库、会话、事务、结果集、空间、标签、边类型、查询执行、参数化执行、查询校验与解释、会话统计。查询参数主形态为 `String`，另接受空、布尔、整数、浮点与原始字节数组；文本穿越统一走标准 UTF-8 字节形态。函数注册、扩展装载、各类钩子回调、繁忙处理器不在首版范围内。

错误映射与其它绑定一致：高频主码抛出具名异常（`BusyError`、`ReadonlyError`、`NotFoundError`、`SchemaError`、`ConstraintError`、`TypeMismatchError`、`MisuseError`），其余降级为 `LinkrsError` 基类并保留数字码与 `getCode()` 访问，另提供 `isNotFound` 系列与 `codeOf` 辅助判断。

## 资源与线程

子对象持有父引用，父先关闭则子调用报误用异常，回收器提供兜底。会话方法互斥串行化，会话不得跨线程并发共享。

## 测试

```shell
cd bindings/java && mvn -o test
```

冒烟测试覆盖建模、写入、查询、事务、保存点、错误映射全链路，另加重复关闭用例。
