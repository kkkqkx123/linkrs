# P2 [dcl] DCL 测试脚本沿用已废弃的 `CREATE SPACE ... WITH DIMENSION` 写法，整套用例长期红

## 现象

`crates/linkrs-query/tests/dcl/` 下的用户/角色测试大量使用
`CREATE SPACE <name> WITH DIMENSION=128` 建空间。该写法在 DDL 语法收敛后已不受支持：

- 解析器只认可选括号参数 `(vid_type=..., comment=...)`
  （`crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs:172-199`，
  未知选项显式报 "Unsupported space option; supported options: vid_type, comment"）；
- `WITH DIMENSION=` 由更早的 `ddl phase2` 提交
  （`git log -S DIMENSION -- crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs`）
  从语法中移除，但 DCL 测试未同步。

其后所有需要空间的用例（GRANT/REVOKE/级联/一致性等）在建空间一步即失败，
整套 60+ 用例以解析错误收场：

```text
DBError { ... "[parse] Parse error at line 1, column 42: Too many parse errors, aborting" }
```

由于失败信息是解析错误、且每个用例都在同一句倒下，很容易被误判为"与本次改动无关"
而放过，实际上它把整个 DCL 测试面（含新增的 REVOKE role 校验、IF NOT EXISTS、
自助改密等回归保护）一起挡在门外。

## 影响

DCL 测试套件长期整体失效，任何 DCL 语义回归都得不到信号；修复 DCL 相关缺陷时
缺乏可信的回归基线（例如 `REVOKE` 角色校验、`CREATE USER IF NOT EXISTS` 语义）。

## 根因

测试脚本与解析语法各改各的：语法侧删除 `WITH DIMENSION` 时没有同步 `tests/dcl/`。
DCL 测试建空间用的是 DDL 语法，二者耦合点没有集中维护（各文件逐处硬编码）。

## 建议

1. 已处理：把 `tests/dcl/**` 中的 `CREATE SPACE <name> WITH DIMENSION=128`
   统一改为受支持的 `CREATE SPACE <name>`（本次随 DCL 语义修复一并完成）。
2. 长期：把 DCL 测试里"建空间/建用户"这类前置语句提取为共享 helper
   （`tests/common`），避免同一语法在数十处硬编码，语法变更时只需改一处。
3. 回归保护：DCL 套件必须整体绿才算完成；不允许"用解析错误掩盖"的用例存在。

## 严重级

**P2（测试基础设施失效，非产品行为缺陷；但遮蔽 DCL 全部回归信号）**

## 代码位置

`crates/linkrs-query/tests/dcl/*.rs`（共 67 处 `WITH DIMENSION=128`）、
`crates/linkrs-query/src/parser/parsing/ddl_parser/create_parser.rs:172-199`
