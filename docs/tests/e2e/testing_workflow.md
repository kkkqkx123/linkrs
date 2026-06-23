# GraphDB 测试工作流指南

本文档总结了完整的 GraphDB 测试操作流程，包括服务器启动测试、E2E 验证和完整测试套件执行。

## 前置条件

1. 确保 Qdrant 已启动（如果需要向量功能）
2. 确保 GraphDB 已编译（`cargo build --release --features server`）
3. 确保配置文件 `config.toml` 存在

## 工作流一：服务器启动集成测试

### 目的

验证服务器能否正常启动、响应健康检查并优雅关闭。

### 执行步骤

```powershell
# 1. 进入项目目录
cd d:\项目\database\graphDB

# 2. 运行服务器启动测试
python tests\server_startup_test.py
```

### 预期输出

```
============================================================
GraphDB Server Startup Integration Test
============================================================
✓ PASS: test_01_server_binary_exists - OK
✓ PASS: test_02_config_file_exists - OK
✓ PASS: test_03_port_available - OK
✓ PASS: test_04_start_server - OK
✓ PASS: test_05_health_endpoint - OK
✓ PASS: test_06_api_endpoints - OK
✓ PASS: test_07_graceful_shutdown - OK

Total: 7 tests, 7 passed, 0 failed
```

### 测试内容说明

| 测试项     | 说明                                  |
| ---------- | ------------------------------------- |
| 二进制检查 | 验证 `bin/graphdb-server.exe` 存在    |
| 配置检查   | 验证 `config.toml` 存在               |
| 端口检查   | 验证端口 9758 未被占用                |
| 启动测试   | 启动服务器并等待就绪                  |
| 健康检查   | 访问 `/v1/health` 端点                |
| API 端点   | 测试 `/v1/health` 和 `/v1/auth/login` |
| 关闭测试   | 发送终止信号，验证优雅关闭            |

---

## 工作流二：E2E 基础验证

### 目的

验证服务器核心功能：启动、认证、查询执行。

### 执行步骤

```powershell
# 1. 进入项目目录
cd d:\项目\database\graphDB

# 2. 运行 E2E 基础验证
python tests\e2e_verify.py
```

### 预期输出

```
============================================================
E2E Verification Summary
============================================================
✓ PASS: Server Startup
✓ PASS: Health Check
✓ PASS: Data Generation
✓ PASS: Basic Query
✓ PASS: Cleanup

Total: 5/5 steps passed
✓ E2E Verification PASSED
```

### 测试内容说明

| 步骤            | 说明                                                         |
| --------------- | ------------------------------------------------------------ |
| Server Startup  | 启动 GraphDB 服务器                                          |
| Health Check    | 验证健康端点返回 200                                         |
| Data Generation | 生成 E2E 测试数据                                            |
| Basic Query     | 执行 6 个基础查询（CREATE/USE/CREATE TAG/INSERT/FETCH/DROP） |
| Cleanup         | 停止服务器并清理资源                                         |

---

## 工作流三：完整 E2E 测试套件

### 目的

运行完整的 E2E 测试套件，覆盖社交网络的完整场景。

### 执行步骤

```powershell
# 1. 进入项目目录
cd d:\项目\database\graphDB

# 2. 确保没有残留的服务器进程
Get-Process graphdb-server -ErrorAction SilentlyContinue | Stop-Process -Force

# 3. 启动服务器（后台运行）
Start-Process -FilePath ".\bin\graphdb-server.exe" -ArgumentList "serve", "--config", ".\config.toml" -WindowStyle Hidden

# 4. 等待服务器启动
Start-Sleep -Seconds 5

# 5. 运行完整 E2E 测试
python tests\e2e\run_tests.py

# 6. 测试完成后，停止服务器
Get-Process graphdb-server -ErrorAction SilentlyContinue | Stop-Process -Force
```

### 测试套件内容

| 测试套件                   | 测试用例数 | 说明                            |
| -------------------------- | ---------- | ------------------------------- |
| Social Network Basic       | 5          | 基础功能：创建 space、tag、edge |
| Social Network Data        | 5          | 数据操作：插入 vertex、edge     |
| Social Network Queries     | 6          | 查询功能：MATCH、GO、LOOKUP     |
| Social Network Explain     | 3          | 执行计划：EXPLAIN、PROFILE      |
| Social Network Transaction | 2          | 事务：COMMIT、ROLLBACK          |
| Social Network Cleanup     | 1          | 清理：删除测试 space            |

---

## 工作流四：重新构建并验证

### 目的

修改代码后重新构建并验证。

### 执行步骤

```powershell
# 1. 进入项目目录
cd d:\项目\database\graphDB

# 2. 停止现有服务器
Get-Process graphdb-server -ErrorAction SilentlyContinue | Stop-Process -Force

# 3. 重新构建 release 版本
& 'D:\softwares\Visual Studio\Common7\Tools\Launch-VsDevShell.ps1'
cargo build --release --features server

# 4. 删除旧文件，复制新的可执行文件到 bin 目录(直接复制会被OS拦截)
Remove-Item .\bin\graphdb-server.exe; Copy-Item .\target\release\graphdb-server.exe .\bin\graphdb-server.exe -Force

# 5. 运行启动测试验证
python tests\server_startup_test.py

# 6. 运行 E2E 验证
python tests\e2e_verify.py
```

---

## 工作流五：生成测试数据

### 目的

单独生成 E2E 测试数据。

### 执行步骤

```powershell
# 1. 进入项目目录
cd d:\项目\database\graphDB

# 2. 生成测试数据
python scripts\generate_e2e_data.py --output-dir tests\e2e\data
```

### 生成的数据文件

| 文件                  | 说明             |
| --------------------- | ---------------- |
| `social_network.gql`  | 社交网络场景数据 |
| `ecommerce.gql`       | 电商场景数据     |
| `geography.gql`       | 地理数据场景     |
| `vector_search.gql`   | 向量搜索测试数据 |
| `fulltext_search.gql` | 全文搜索测试数据 |
| `optimizer_test.gql`  | 优化器测试数据   |

---

## 故障排查

### 问题 1：端口被占用

```powershell
# 检查端口占用
Get-NetTCPConnection -LocalPort 9758

# 终止占用进程
Get-Process -Id (Get-NetTCPConnection -LocalPort 9758).OwningProcess | Stop-Process
```

### 问题 2：服务器启动超时

```powershell
# 手动检查服务器输出
.\bin\graphdb-server.exe serve --config .\config.toml
```

### 问题 3：Qdrant 连接失败

```powershell
# 检查 Qdrant 是否运行
curl http://localhost:6333/healthz

# 如果不需要向量功能，可以在 config.toml 中禁用：
# [vector]
# enabled = false
```

### 问题 4：权限错误

确保 PowerShell 执行策略允许运行脚本：

```powershell
Set-ExecutionPolicy -ExecutionPolicy RemoteSigned -Scope CurrentUser
```

---

## 快速验证清单

每次修改代码后，按以下顺序验证：

- [ ] 1. 编译成功：`cargo build --release --features server`
- [ ] 2. 启动测试通过：`python tests\server_startup_test.py`
- [ ] 3. E2E 验证通过：`python tests\e2e_verify.py`
- [ ] 4. （可选）完整 E2E 测试：`python tests\e2e\run_tests.py`

---

## 相关文件位置

| 文件             | 路径                           |
| ---------------- | ------------------------------ |
| 服务器启动测试   | `tests/server_startup_test.py` |
| E2E 基础验证     | `tests/e2e_verify.py`          |
| E2E 测试套件     | `tests/e2e/run_tests.py`       |
| E2E 客户端       | `tests/e2e/graphdb_client.py`  |
| 测试数据生成     | `scripts/generate_e2e_data.py` |
| 服务器可执行文件 | `bin/graphdb-server.exe`       |
| 配置文件         | `config.toml`                  |
| 问题文档         | `docs/issue/`                  |

---

## 注意事项

1. **服务器进程管理**：每次测试前确保没有残留的服务器进程
2. **端口冲突**：确保端口 9758 未被其他程序占用
3. **Qdrant 依赖**：向量功能需要 Qdrant 运行，否则会自动禁用
4. **测试顺序**：先运行启动测试，再运行 E2E 验证
5. **日志查看**：服务器日志位于 `log/` 目录
