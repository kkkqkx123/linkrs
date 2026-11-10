# Go 绑定

目录 `bindings/go`，模块 `github.com/kkkqkx123/linkrs/bindings/go`，
cgo 薄封装，只发布纯 Go 代码，需 C 工具链。

## 前置要求

```shell
cargo build --features c_api
```

编译期与运行期搜索路径默认指向工作区 `target/debug` 与 `target/release` 下的
`liblinkrs`，保持绑定与动态库来自同一检出（见 `database.go` 头部 cgo 段与 `doc.go`）。

## 用法

```go
import linkrs "github.com/kkkqkx123/linkrs/bindings/go"

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
```

## API 一览（`database.go`）

- `Config`：`NewConfig/ConfigForFile/MemoryConfig/Close/Closed`。
- `Database`：`Open/OpenMemory/OpenWithConfig/Close/Closed`、
  `LastCode/LastMessage/LibVersion`、`Session`、
  `CreateSpace/DropSpace/ListSpaces/GetSpace`、
  `CreateTag/DropTag/ListTags/GetTag`、
  `CreateEdgeType/DropEdgeType/ListEdgeTypes/GetEdgeType`
 （属性定义为 `any` 走 JSON）。
- `Session`：`UseSpace/CurrentSpace`、`Execute/ExecuteParams`、
  `ValidateQuery/ExplainQuery`、`Statistics`、`Begin/BeginReadonly`。
- `Result`：`ColumnCount/RowCount/ColumnNames`、`Rows/Row(index)`。
- `Transaction`：`Execute/ExecuteParams`、`Commit/Rollback`、
  `Savepoint/ReleaseSavepoint/RollbackToSavepoint`。

参数打包 `packValues` 支持空、布尔、整数、浮点、字符串；
错误见 `errors.go`（七组具名 + 基类保留数字码）。

## 测试

```shell
cd bindings/go && go test ./...
```
