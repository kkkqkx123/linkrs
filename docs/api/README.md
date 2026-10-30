# Linkrs API 文档

本目录按功能模块组织各层 API 文档，并提供跨层功能矩阵。

## 目录

- [功能矩阵](feature_matrix.md)
- [嵌入式 API](embedded/README.md)
- [服务端 API](server/README.md)
- [Web 管理 API](web/README.md)
- [客户端 API](client/README.md)
- [CLI 元命令](cli/README.md)

## 层级说明

- **嵌入式 Rust API**：进程内数据库访问，适合单机/桌面/测试。
- **服务端 HTTP/gRPC API**：网络服务，支持多客户端与管理能力。
- **Web 管理 API**：挂载于 `/api` 的管理与浏览接口。
- **客户端 API**：`linkrs-client` 对 HTTP API 的 Rust 封装。
- **CLI**：命令行工具，支持本地嵌入与远程服务两种模式。
