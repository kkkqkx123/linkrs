# design/

本目录存放跨客户端共享的设计 token 数据文件。

## `tokens.json`

`tokens.json` 是 Web 端（`frontend/src/app.css`）与桌面端（`crates/linkrs-gpui`）之间共享的设计 token 单一数据源，**不是设计稿或原型残留，不能删除**。

实际使用位置：

- 桌面客户端编译期嵌入：`crates/linkrs-gpui/src/design_tokens.rs`
  ```rust
  const TOKEN_JSON: &str = include_str!("../../../design/tokens.json");
  ```
  桌面端的主题、颜色、字号、阴影、布局等都从该文件读取。
- 前端同步检查：`frontend/scripts/check-design-tokens.mjs`
  作为 `npm run check:tokens` / `npm run check` 的一部分，校验 `tokens.json` 与 `frontend/src/app.css` 中的 CSS 变量保持一致。

其中 `theme`、`root`、`dense`、`dark` 四块与 `frontend/src/app.css` 同步；`desktop` 块存放桌面端专属的字体栈与按下态颜色。

如需修改设计 token，请同时更新 `frontend/src/app.css` 与本文件，并运行：

```sh
cd frontend && npm run check:tokens
```

以及桌面端构建检查：

```sh
cargo check --manifest-path crates/linkrs-gpui/Cargo.toml
```