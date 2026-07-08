# GraphDB E2E 测试文档

本文档描述了 GraphDB 端到端 (E2E) 测试的完整方案，包括测试架构、执行流程和扩展方法。

## 目录

1. [测试架构](#测试架构)
2. [目录结构](#目录结构)
3. [快速开始](#快速开始)
4. [测试数据生成](#测试数据生成)
5. [执行测试](#执行测试)
6. [测试套件说明](#测试套件说明)
7. [扩展测试](#扩展测试)
8. [故障排查](#故障排查)

## 测试架构

### 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│                     E2E Test Framework                       │
├─────────────────────────────────────────────────────────────┤
│  Test Runner (run_tests.py)                                  │
│  ├── Test Suite: Social Network                              │
│  ├── Test Suite: Optimizer                                   │
│  └── Test Suite: Extended Types                              │
├─────────────────────────────────────────────────────────────┤
│  GraphDB Client (graphdb_client.py)                          │
│  ├── HTTP API Wrapper                                        │
│  ├── Connection Management                                   │
│  └── Result Parsing                                          │
├─────────────────────────────────────────────────────────────┤
│  Test Data Generator (scripts/generate_e2e_data.py)          │
│  ├── Social Network Data                                     │
│  ├── E-commerce Data                                         │
│  ├── Geography Data                                          │
│  ├── Vector Data                                             │
│  ├── Full-text Data                                          │
│  └── Optimizer Data                                          │
└─────────────────────────────────────────────────────────────┘
                              │
                              ▼
┌─────────────────────────────────────────────────────────────┐
│                     GraphDB Server                           │
│  (HTTP API on port 9758)                                     │
└─────────────────────────────────────────────────────────────┘
```

### 测试流程

```
1. 启动 GraphDB 服务器
        │
        ▼
2. 生成测试数据 (可选)
        │
        ▼
3. 执行测试套件
   ├── 连接服务器
   ├── 创建测试空间
   ├── 执行 Schema DDL
   ├── 插入测试数据
   ├── 执行查询验证
   ├── 验证 EXPLAIN/PROFILE
   └── 清理测试空间
        │
        ▼
4. 生成测试报告
```

## 目录结构

```
graphDB/
├── scripts/
│   ├── generate_e2e_data.py      # 测试数据生成脚本
│   └── graphdb.ps1               # 服务管理脚本
│
├── tests/
│   └── e2e/
│       ├── __init__.py
│       ├── graphdb_client.py     # GraphDB HTTP 客户端
│       ├── run_tests.py          # 测试运行主入口
│       ├── test_social_network.py    # 社交网络测试套件
│       ├── test_optimizer.py         # 优化器测试套件
│       ├── test_extended_types.py    # 扩展类型测试套件
│       ├── data/                 # 生成的测试数据
│       │   ├── social_network_data.gql
│       │   ├── ecommerce_data.gql
│       │   ├── geography_data.gql
│       │   ├── vector_data.gql
│       │   ├── fulltext_data.gql
│       │   └── optimizer_data.gql
│       └── reports/              # 测试报告
│           ├── e2e_test_report.json
│           └── e2e_test_report.xml
│
└── docs/
    └── tests/
        └── e2e/
            ├── README.md         # 本文档
            ├── design.md         # 基础 E2E 测试设计
            ├── extend.md         # 扩展类型测试设计
            └── optimizer_explain.md  # 优化器测试设计
```

## 快速开始

### 前置条件

1. 编译 GraphDB:
```powershell
# 使用 VS 环境编译
& 'D:\softwares\Visual Studio\Common7\Tools\Launch-VsDevShell.ps1'
cargo build --release
```

2. 启动 GraphDB 服务器:
```powershell
# 方式1: 使用 PowerShell 脚本
.\scripts\graphdb.ps1 start

# 方式2: 直接运行
cargo run --release
```

3. 验证服务器状态:
```powershell
curl http://127.0.0.1:9758/v1/health
```

### 运行测试

```powershell
# 进入 E2E 测试目录
cd tests\e2e

# 运行所有测试
python run_tests.py

# 生成测试数据并运行测试
python run_tests.py --generate-data

# 运行指定测试套件
python run_tests.py --suite social
python run_tests.py --suite optimizer
python run_tests.py --suite extended

# 生成 JUnit XML 报告
python run_tests.py --report junit --report-file test_report

# 指定服务器地址
python run_tests.py --host 192.168.1.100 --port 9758
```

### 生成测试数据

```powershell
# 生成所有场景的测试数据
python scripts\generate_e2e_data.py --output-dir tests\e2e\data

# 生成指定场景的测试数据
python scripts\generate_e2e_data.py --scenario social
python scripts\generate_e2e_data.py --scenario geography
python scripts\generate_e2e_data.py --scenario vector

# 使用自定义随机种子
python scripts\generate_e2e_data.py --seed 12345
```

## 测试数据生成

### 数据生成器架构

```python
TestDataGenerator (基类)
    ├── SocialNetworkGenerator    # 社交网络场景
    ├── ECommerceGenerator        # 电商场景
    ├── GeographyGenerator        # 地理空间场景
    ├── VectorGenerator           # 向量搜索场景
    ├── FullTextGenerator         # 全文检索场景
    └── OptimizerGenerator        # 优化器测试场景
```

### 各场景数据规模

| 场景 | 顶点数 | 边数 | 说明 |
|------|--------|------|------|
| 社交网络 | 25 | 65 | 20人+5公司，朋友/工作/居住关系 |
| 电商 | 800 | 7500 | 100用户+200商品+500订单 |
| 地理空间 | 210 | 500 | 10城市+200地点，邻近关系 |
| 向量搜索 | 3500 | 0 | 1000商品向量+500图像+2000文本 |
| 全文检索 | 1500 | 0 | 500文章+1000商品描述 |
| 优化器 | 10100 | 10000 | 10000人+100公司+工作关系 |

### 数据生成示例

```python
from scripts.generate_e2e_data import SocialNetworkGenerator

# 创建生成器
gen = SocialNetworkGenerator(
    num_persons=50,      # 50个人
    num_companies=10,    # 10个公司
    num_friend_edges=80, # 80条朋友关系
    seed=42              # 随机种子
)

# 生成 GQL 脚本
gql_script = gen.generate()

# 保存到文件
gen.save(Path("tests/e2e/data/custom_social.gql"))
```

## 执行测试

### 测试执行模式

#### 1. 完整测试流程

```python
# run_tests.py 自动执行以下步骤:
1. 检查服务器连接
2. 生成测试数据 (如果指定 --generate-data)
3. 按顺序执行测试套件:
   - Social Network Tests
   - Optimizer Tests
   - Extended Types Tests
4. 汇总测试结果
5. 生成测试报告 (如果指定 --report)
```

#### 2. 独立测试执行

```python
# 直接运行单个测试文件
python tests\e2e\test_social_network.py
python tests\e2e\test_optimizer.py
python tests\e2e\test_extended_types.py
```

#### 3. 使用 unittest 框架

```python
# 运行特定测试类
python -m unittest tests.e2e.test_social_network.TestSocialNetworkBasic

# 运行特定测试方法
python -m unittest tests.e2e.test_social_network.TestSocialNetworkBasic.test_001_connect_and_show_spaces

#  verbose 模式
python -m unittest -v tests.e2e.test_social_network
```

### 测试报告

#### JSON 格式报告

```json
{
  "start_time": "2026-04-27T10:00:00",
  "end_time": "2026-04-27T10:05:30",
  "duration_seconds": 330.5,
  "suites": [
    {
      "name": "Social Network",
      "total": 25,
      "passed": 25,
      "failed": 0,
      "errors": 0,
      "skipped": 0,
      "success": true
    }
  ],
  "summary": {
    "total": 75,
    "passed": 75,
    "failed": 0,
    "errors": 0,
    "skipped": 0
  }
}
```

#### JUnit XML 格式报告

```xml
<?xml version="1.0" encoding="utf-8"?>
<testsuites time="330.5" tests="75" failures="0" errors="0">
  <testsuite name="Social Network" tests="25" failures="0" errors="0" skipped="0">
    <!-- test cases -->
  </testsuite>
</testsuites>
```

## 测试套件说明

### 1. 社交网络测试 (test_social_network.py)

测试范围:
- 基础连接与会话管理
- Schema 管理 (Space/Tag/Edge/Index)
- 数据操作 (INSERT/FETCH/UPDATE/DELETE)
- 查询语句 (MATCH/GO/LOOKUP/FIND PATH)
- EXPLAIN/PROFILE 分析
- 事务管理 (BEGIN/COMMIT/ROLLBACK)

测试用例示例:
```python
class TestSocialNetworkBasic(unittest.TestCase):
    def test_001_connect_and_show_spaces(self):
        """TC-001: 连接服务器并列出空间"""
        result = self.client.execute("SHOW SPACES")
        self.assertTrue(result.success)
```

### 2. 优化器测试 (test_optimizer.py)

测试范围:
- 索引选择优化
- 连接算法选择 (HashJoin/IndexJoin/NestedLoop)
- 聚合策略 (HashAggregate/SortAggregate/StreamingAggregate)
- TopN 优化
- EXPLAIN 输出格式验证
- PROFILE 性能分析

测试用例示例:
```python
def test_idx_001_index_scan_for_equality(self):
    """TC-IDX-001: 等值查询应使用 IndexScan"""
    result = self.client.execute('''
        EXPLAIN MATCH (p:person {name: "Alice"}) RETURN p.age
    ''')
    plan = json.dumps(result.data)
    self.assertIn("IndexScan", plan)
```

### 3. 扩展类型测试 (test_extended_types.py)

测试范围:
- 地理空间类型 (GEOGRAPHY/POINT)
  - ST_Point/ST_GeogFromText
  - ST_Distance/ST_DWithin
  - 范围查询
- 向量搜索 (VECTOR)
  - 向量插入
  - cosine_similarity/l2_distance
  - 带过滤的向量搜索
- 全文检索 (FULLTEXT INDEX)
  - BM25/Inversearch 索引
  - SEARCH 语句
  - 布尔查询

## 扩展测试

### 添加新的测试场景

#### 1. 创建数据生成器

```python
# scripts/generate_e2e_data.py

class MyScenarioGenerator(TestDataGenerator):
    def __init__(self, seed=42):
        super().__init__(seed)

    def generate_schema(self):
        self.add("CREATE SPACE IF NOT EXISTS e2e_myscenario")
        self.add("USE e2e_myscenario")
        self.add("CREATE TAG mytag(name: STRING)")
        # ...

    def generate(self) -> str:
        self.generate_schema()
        # ... generate data
        return super().generate()
```

#### 2. 创建测试文件

```python
# tests/e2e/test_myscenario.py

import unittest
from graphdb_client import GraphDBClient

class TestMyScenario(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.client = GraphDBClient()
        cls.client.connect()
        # setup data

    def test_my_feature(self):
        """TC-MY-001: Test my feature"""
        result = self.client.execute("MATCH (n:mytag) RETURN n")
        self.assertTrue(result.success)
```

#### 3. 注册到测试运行器

```python
# tests/e2e/run_tests.py

from test_myscenario import TestMyScenario

# 在 run_all_tests() 中添加:
if not self.run_suite("My Scenario", [TestMyScenario]):
    all_passed = False
```

### 添加新的测试用例

```python
# 在现有测试类中添加新方法

class TestSocialNetworkQueries(unittest.TestCase):
    def test_xxx_my_new_query(self):
        """TC-XXX: 我的新查询测试"""
        self.client.execute(f"USE {self.space_name}")

        result = self.client.execute('''
            MATCH (p:person)-[:friend*2]->(f:person)
            WHERE p.name == "Alice"
            RETURN f.name
        ''')
        self.assertTrue(result.success)
        # 添加更多断言
```

## 故障排查

### 常见问题

#### 1. 无法连接服务器

```
Error: Not connected to server
```

解决方案:
```powershell
# 1. 检查服务器是否运行
curl http://127.0.0.1:9758/v1/health

# 2. 检查端口配置
python run_tests.py --port 8080  # 如果使用了不同端口

# 3. 检查防火墙设置
```

#### 2. Schema 创建超时

```
Error: Request timeout
```

解决方案:
```python
# 增加客户端超时时间
client = GraphDBClient(timeout=60)  # 默认 30 秒

# 或者在测试中添加延迟
import time
time.sleep(2)  # 等待 schema 传播
```

#### 3. 测试数据加载失败

```
Error: Failed to insert vertex
```

解决方案:
```powershell
# 1. 重新生成测试数据
python run_tests.py --generate-data

# 2. 检查数据文件是否存在
ls tests\e2e\data\

# 3. 手动加载测试数据
python -c "
from graphdb_client import GraphDBClient, TestDataLoader
client = GraphDBClient()
client.connect()
loader = TestDataLoader(client)
loader.load_from_file('tests/e2e/data/social_network_data.gql')
"
```

#### 4. 索引未生效

```
EXPLAIN 显示 SeqScan 而不是 IndexScan
```

解决方案:
```python
# 1. 确保索引已创建
client.execute("SHOW INDEXES")

# 2. 等待索引构建完成
import time
time.sleep(2)

# 3. 检查统计信息是否更新
```

### 调试技巧

#### 1. 启用详细日志

```python
# 在测试中添加调试输出
result = self.client.execute("MATCH (n) RETURN n")
print(f"Result: {result.data}")
print(f"Error: {result.error}")
print(f"Time: {result.execution_time_ms}ms")
```

#### 2. 使用 EXPLAIN 分析查询

```python
# 查看查询计划
result = self.client.explain("MATCH (p:person) RETURN p.name")
print(json.dumps(result.data, indent=2))
```

#### 3. 逐步执行测试

```python
# 在 setUpClass 中设置断点
@classmethod
def setUpClass(cls):
    import pdb; pdb.set_trace()  # 设置断点
    cls.client = GraphDBClient()
    cls.client.connect()
```

### 性能调优

#### 1. 批量插入数据

```python
# 使用批量插入代替单条插入
statements = []
for i in range(1000):
    statements.append(f'INSERT VERTEX ...')

# 每 10 条执行一次
for batch in chunks(statements, 10):
    for stmt in batch:
        client.execute(stmt)
```

#### 2. 调整超时设置

```python
# 对于大数据集测试，增加超时
client = GraphDBClient(timeout=120)
```

#### 3. 并行执行测试

```powershell
# 使用 pytest-xdist 并行执行
pip install pytest-xdist
pytest tests/e2e -n auto
```

## 附录

### 环境变量

| 变量名 | 说明 | 默认值 |
|--------|------|--------|
| GRAPHDB_HOST | GraphDB 服务器地址 | 127.0.0.1 |
| GRAPHDB_PORT | GraphDB 服务器端口 | 9758 |

### 命令行参数

| 参数 | 说明 | 示例 |
|------|------|------|
| --suite | 指定测试套件 | --suite social |
| --generate-data | 生成测试数据 | --generate-data |
| --report | 生成报告格式 | --report junit |
| --report-file | 报告文件名 | --report-file myreport |
| --host | 服务器地址 | --host 192.168.1.100 |
| --port | 服务器端口 | --port 8080 |

### 相关文档

- [design.md](design.md) - 基础 E2E 测试设计
- [extend.md](extend.md) - 扩展类型测试设计
- [optimizer_explain.md](optimizer_explain.md) - 优化器测试设计
