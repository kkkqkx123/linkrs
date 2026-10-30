# Web API 功能概览

本文档整理了 `src\api\server\web` 目录提供的所有 API 及功能。

## 目录

- [API 架构](#api-架构)
- [认证机制](#认证机制)
- [API 分类](#api-分类)
  - [1. 查询历史和收藏夹 API](#1-查询历史和收藏夹-api)
  - [2. Schema 扩展 API](#2-schema-扩展-api)
  - [3. 数据浏览 API](#3-数据浏览-api)
  - [4. 图形数据 API](#4-图形数据-api)
- [数据模型](#数据模型)
- [错误处理](#错误处理)
- [存储实现](#存储实现)

---

## API 架构

Web 模块基于 Axum 框架构建，提供 RESTful API 接口，主要包含以下组件：

- **Handlers**: 处理 HTTP 请求的业务逻辑
- **Models**: 定义请求和响应的数据结构
- **Services**: 提供业务逻辑层
- **Storage**: 元数据持久化层（基于 SQLite）
- **Middleware**: 认证和授权中间件

### 路由结构

```
/v1/queries     - 查询历史和收藏夹
/v1/schema      - Schema 扩展管理
/v1/data        - 数据浏览
/v1/graph       - 图形数据查询
```

---

## 认证机制

所有 Web API 都需要通过认证中间件验证。

### 认证方式

- **Header**: `X-Session-ID`
- **类型**: `i64` (会话 ID)
- **验证**: 会话 ID 必须在 Session Manager 中存在且有效

### 认证流程

1. 客户端在请求头中携带 `X-Session-ID`
2. 中间件验证会话 ID 是否有效
3. 如果有效，将 `session_id` 注入到请求扩展中
4. Handler 可以通过 `Extension<session_id>` 获取会话 ID

---

