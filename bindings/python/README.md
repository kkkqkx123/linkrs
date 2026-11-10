# Linkrs Python 绑定

基于标准库 `ctypes` 的嵌入式直调绑定，无需编译器。调用者先构建动态库并指定路径，本包只发布纯 Python 代码。

## 前置要求

```shell
cargo build --features c_api
```

默认在仓库 `target/debug` 或 `target/release` 下查找 `liblinkrs`；也可用环境变量显式指定：

```shell
export LINKRS_LIB=/path/to/liblinkrs.so
```

找不到动态库时抛出 `LinkrsError` 并列出尝试过的路径。

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

文件数据库用 `Database.open(path)`，配置文件打开用 `Config.memory()`、`Config.for_file(path)`、`Config.new()` 配合 `Database.open_with_config(config)`。

## 范围

首版覆盖数据库、会话、事务、结果集、空间、标签、边类型、查询执行、参数化执行、查询校验与解释、会话统计。查询参数仅支持空、布尔、整数、浮点、字符串；函数注册、扩展装载、各类钩子回调、繁忙处理器不在首版范围内。

错误为两级：主码决定异常大类（`BusyError`、`ReadonlyError`、`NotFoundError`、`SchemaError`、`ConstraintError`、`TypeMismatchError`、`MisuseError`，其余归入 `LinkrsError`），扩展码与错误位置作为异常属性保留。

## 资源与线程

`Database`、`Session`、`Transaction`、`Result` 均支持上下文管理器与可重复关闭。子对象持有父引用，父先关闭则子调用报误用异常。会话不得跨线程并发共享，每个线程使用独立会话。

## 测试

```shell
cd bindings/python && python -m unittest discover -s tests
```

冒烟测试覆盖建模、写入、查询、事务、保存点、错误映射全链路。
