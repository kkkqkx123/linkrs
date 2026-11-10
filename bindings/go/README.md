# Linkrs Go 绑定

基于 cgo 的嵌入式直调薄封装。调用者先构建动态库并指定路径，本包只发布纯 Go 代码；启用 cgo 与 C 工具链为前置要求。

## 前置要求

```shell
cargo build --features c_api
```

编译期与运行期搜索路径默认指向仓库 `target/debug` 与 `target/release` 下的 `liblinkrs`，调用者保持绑定与动态库来自同一检出。

## 用法

```go
package main

import (
    "fmt"

    linkrs "github.com/kkkqkx123/linkrs/bindings/go"
)

func main() {
    db, err := linkrs.OpenMemory()
    if err != nil {
        panic(err)
    }
    defer db.Close()

    if err := db.CreateSpace("g", 1, 1, nil, nil); err != nil {
        panic(err)
    }
    session, err := db.Session()
    if err != nil {
        panic(err)
    }
    defer session.Close()

    if err := session.UseSpace("g"); err != nil {
        panic(err)
    }
    result, err := session.Execute("SHOW SPACES")
    if err != nil {
        panic(err)
    }
    defer result.Close()
    rows, err := result.Rows()
    if err != nil {
        panic(err)
    }
    fmt.Println(rows)
}
```

文件数据库用 `Open(path)`，配置文件打开用 `NewConfig`、`ConfigForFile`、`MemoryConfig` 配合 `OpenWithConfig`。

## 范围

首版覆盖数据库、会话、事务、结果集、空间、标签、边类型、查询执行、参数化执行、查询校验与解释、会话统计。查询参数仅支持空、布尔、整数、浮点、字符串；函数注册、扩展装载、各类钩子回调、繁忙处理器不在首版范围内。

错误映射与 Python、JavaScript 绑定一致：高频主码返回具名包装错误，其余降级为 `LinkrsError` 基类并保留数字码，另提供 `IsNotFound` 系列与 `CodeOf`、`ExtendedOf` 辅助判断。

## 资源与线程

子对象持有父引用，父先关闭则子调用报误用错误。`Close` 可重复调用并有终结器兜底。会话内部以读写锁串行化，不同 goroutine 应优先使用独立会话。

## 测试

```shell
cd bindings/go && go test ./...
```

冒烟测试覆盖建模、写入、查询、事务、保存点、错误映射全链路，另加重复关闭用例。
