# Linkrs 数据操作语言 (DML)

## 概述

数据操作语言 (DML) 用于对图数据库中的数据进行增删改操作，包括插入、更新、删除、合并、批量导入/导出等功能。

> 本文描述当前代码实现的行为契约：可用的写路径、语法边界，以及被显式拒绝的形式与对应报错。
> 凡未声明可用的写路径均以明确错误收敛，不存在静默丢弃写入的路径。

---

## 1. INSERT - 插入数据

### 功能
插入新的顶点或边。

### 语法结构
```cypher
INSERT VERTEX [IF NOT EXISTS] <tag_name> (<prop_list>) [VALUES] <vid>: (<value_list>) [, <vid>: (<value_list>) ...]
INSERT VERTEX [IF NOT EXISTS] ON <tag_name> [VALUES] <vid> [, <vid> ...]
INSERT EDGE [IF NOT EXISTS] <edge_type> [(<prop_list>)] [VALUES] <src_vid> -> <dst_vid> [@<rank>]: [(<value_list>)] [, <src_vid> -> <dst_vid> [@<rank>]: ...]
```

### 关键特性
- 支持批量插入（逗号分隔多行）；边插入的 `@<rank>` 端到端生效于写入、去重与删除
- **每语句仅单个标签**：存储层顶点恰好挂一个 tag；多标签写法会被解析器显式拒绝
- 顶点形式支持空括号（`person()` 等价于只写 VID，与 `ON <tag>` 简写一致）；边形式支持 `()` 与 `:()`
- **缺省属性名列表时只写入 VID**：省略括号或使用 `ON <tag>` 简写形式时只写 VID；
  若同时给出值组，绑定层报
  `INSERT VERTEX without a property name list only writes vertex ids; specify property names to write properties`
- **属性名与值数量必须一致**：不匹配时报
  `INSERT VERTEX property value count (N) does not match property name count (M) for tag '<tag>'`
  （边形式为 `INSERT EDGE property value count ...`）
- 支持 `IF NOT EXISTS`：顶点已存在则跳过且不计入插入数；边仅探测同 rank 的既有边
- `VALUES` 关键字可省略
- 顶点 VID 和属性值均为表达式

### 示例
```cypher
-- 基本顶点插入
INSERT VERTEX person(name, age) VALUES "101": ("Alice", 25), "102": ("Bob", 30)

-- 带IF NOT EXISTS的插入（幂等操作）
INSERT VERTEX IF NOT EXISTS person(name, age) VALUES "101": ("Alice", 25)

-- ON简写形式（只写 VID，不得携带值组）
INSERT VERTEX ON person VALUES "104"

-- 边插入（rank 生效）
INSERT EDGE follow(degree) VALUES "101" -> "102" @0: (0.8)

-- 带IF NOT EXISTS的边插入
INSERT EDGE IF NOT EXISTS follow(degree) VALUES "101" -> "102" @0: (0.8)

-- 批量边插入 / 空属性列表
INSERT EDGE follow(degree) VALUES "101" -> "102" @0: (0.8), "102" -> "103" @0: (0.9)
INSERT EDGE follow() VALUES "101" -> "102": ()
```

---

## 2. CREATE - Cypher 风格创建

### 功能
Schema 形式（`CREATE TAG` / `CREATE EDGE TYPE` / `CREATE INDEX` 等）由维护语句（DDL）链路执行，
行为见 DDL 文档。**数据形式（节点、边、路径模式）在绑定层显式拒绝**，写入请使用 INSERT / UPSERT。

### 语法结构
```cypher
-- Schema 形式（可用，走 DDL）
CREATE TAG [IF NOT EXISTS] <tag_name> (<col_name> <type> [, ...])
CREATE EDGE [IF NOT EXISTS] <edge_type> (<col_name> <type> [, ...])
CREATE INDEX [IF NOT EXISTS] <index_name> ON <tag_name>(<col_name>)

-- 数据形式（绑定层拒绝）
CREATE (<variable>:<Label> [{<prop>: <value>, ...}])
CREATE (<src>)-[:<EdgeType> [{<prop>: <value>, ...}]]->(<dst>)
CREATE <pattern1>, <pattern2>, ...
```

### 关键特性
- 节点/边写入仅支持 `INSERT VERTEX` / `INSERT EDGE` 系列语句
  （幂等与条件写入复用其写入能力的 `UPSERT` / `MERGE` 见第 4、6 节）；
  数据形式 `CREATE (...)` 永不执行写入，下述拒绝无任何副作用
- 数据形式在绑定层报错：
  `CREATE data patterns are not supported; use INSERT VERTEX / INSERT EDGE (see docs/release/02_dml_data_manipulation.md)`
- 该拒绝发生在绑定阶段，不会进入规划与执行，也不产生任何写入副作用
- Schema 形式不经过数据 CREATE 链路，与 INSERT 的标签/边类型定义由 DDL 负责这一模型保持一致：
  先以 DDL 定义 tag 与列，再用 INSERT / UPSERT 写数据

### 示例
```cypher
-- 先定义 schema
CREATE TAG person(name STRING, age INT)
CREATE EDGE follow(degree DOUBLE)

-- 再写入数据
INSERT VERTEX person(name, age) VALUES "101": ("Alice", 25)
INSERT EDGE follow(degree) VALUES "101" -> "102" @0: (0.8)

-- 数据形式被拒绝
-- CREATE (n:Person {name: 'Alice'})
--   报错：CREATE data patterns are not supported; use INSERT VERTEX / INSERT EDGE (see docs/release/02_dml_data_manipulation.md)
```

---

## 3. UPDATE - 更新数据

### 功能
更新节点或边的属性。

### 语法结构
```cypher
UPDATE [VERTEX] <vid> ON <tag> SET <prop> = <value> [, ...] [WHERE | WHEN <condition>]
UPDATE EDGE OF <edge_type> FROM <src_vid> TO <dst_vid> [@<rank>] SET <prop> = <value> [, ...] [WHERE <condition>]
UPDATE [EDGE] <src_vid> -> <dst_vid> [@<rank>] OF <edge_type> SET <prop> = <value> [, ...] [WHERE <condition>]
UPDATE TAG <tag> SET <prop> = <value> [, ...] WHERE <condition>   -- 解析支持，计划层拒绝
```

### 关键特性
- **顶点更新必须带标签限定**：`UPDATE VERTEX "101" SET age = 26`（无 `ON <tag>`）在计划层报
  `UPDATE vertex requires a tag qualifier (use UPDATE <vid> ON <tag> SET ...)`；
  正确写法是 `UPDATE VERTEX "101" ON person SET ...` 或短形式 `UPDATE "101" ON person SET ...`
- vid 仅允许字面量、参数（`@p`）或会话变量（`$v`），否则计划期报错
- **`UPDATE TAG <tag> ...` 批量形式在计划层被拒绝**：
  `UPDATE TAG <tag> bulk update is not supported yet; use UPDATE <vid> ON <tag> SET ...`
- **YIELD 在绑定层被拒绝**：`YIELD is not supported for UPDATE/UPSERT yet`；
  需要读取更新后的值请再执行 LOOKUP / MATCH
- `WHERE` 与 `WHEN` 同义；条件与右值表达式读取的是**更新前**的行属性，
  支持 `SET age = age + 1` 这类自引用计算
- 更新目标写成变量名（`UPDATE v ON ...`）且该变量未被外层作用域绑定时，
  右值与条件中的 `<v>.<prop>` 按行属性解析（与裸属性名等价）；其他限定符保持报错
- **整映射覆盖（整行替换语义）**：`SET v = {age: 20}` 将目标属性行整体替换
  （非逐属性合并），`SET v = {}` 清空全部属性；整映射覆盖可与逐属性赋值混写，
  逐属性赋值在替换后的行上继续叠加
- 边更新支持两种语法：`OF <edge_type> FROM <src> TO <dst>`（边类型在前）与
  `<src> -> <dst> [@rank] OF <edge_type>`（字面量/短形式）；`@rank` 端到端生效

### 示例
```cypher
-- 基本顶点更新（必须带 ON 标签限定）
UPDATE VERTEX "101" ON person SET age = 26, name = "Alice Smith"

-- 带条件的顶点更新（WHEN 与 WHERE 同义）
UPDATE "101" ON person SET age = age + 1 WHERE age > 20

-- 整映射覆盖（整行替换）与空映射清空
UPDATE "101" ON person SET v = {age: 20}
UPDATE "101" ON person SET v = {}

-- 混写：先整行替换为 {age: 20}，再叠加 name = 'Bob'
UPDATE "101" ON person SET v = {age: 20}, name = 'Bob'

-- 边更新（OF ... FROM ... TO ... 语法）
UPDATE EDGE OF follow FROM "101" TO "102" SET degree = 0.9

-- 边更新（短形式语法，rank 生效）
UPDATE EDGE "101" -> "102" @0 OF follow SET degree = degree + 0.1 WHERE degree < 1.0
```

---

## 4. UPSERT - 插入或更新数据

### 功能
如果数据存在则更新，不存在则插入（探测后写入，适合幂等写入场景）。

### 语法结构
```cypher
UPSERT VERTEX <vid> ON <tag_name> SET <prop> = <value> [, ...] [WHERE <condition>]
UPSERT VERTEX ON <tag_name> SET <prop> = <value> [, ...] WHERE id(vid) == <vid_value>
UPSERT [EDGE] <src_vid> -> <dst_vid> [@<rank>] OF <edge_type> SET <prop> = <value> [, ...] [WHERE <condition>]
UPSERT EDGE ON <edge_type> SET <prop> = <value> [, ...] WHERE id(src) == <src_value> AND id(dst) == <dst_value> [@<rank>]
```

### 关键特性
- 与 UPDATE 相同的顶点目标规则：`UPSERT VERTEX <vid>`（不带 `ON <tag>`）同样被计划层拒绝
  （要求标签限定）
- **`UPSERT VERTEX ON <tag>` 形式要求 WHERE 中含 `id(vid) == <字面量>` 合取项**，
  缺失时报 `UPSERT VERTEX ON form requires WHERE id(vid) == <literal>`
- **`UPSERT EDGE ON <type>` 形式要求 WHERE 中含 `id(src) == <字面量>` 与 `id(dst) == <字面量>`
  两个合取项**，缺失时报
  `UPSERT EDGE ON form requires WHERE id(src) == <literal> AND id(dst) == <literal>`
- **剩余谓词被保留并继续作为过滤条件生效**：`id(...) == <字面量>` 合取项被消费用于定位目标，
  其余谓词与原条件合并后作用于写入判定，不会被丢弃
- 可选 `WHEN` 子句（两种 ON 形式中）：解析后丢弃，不影响插入或更新判定
- 整映射覆盖（`SET v = {...}`）行为与 UPDATE 一致（整行替换 / 空映射清空）
- YIELD 在绑定层被拒绝（与 UPDATE 相同）
- 支持表达式右值与目标别名自引用（同第 3 节）
- `@rank` 可写在 WHERE 之后、YIELD 之前，端到端生效

### 示例
```cypher
-- 顶点UPSERT（存在则更新，不存在则插入；必须 ON 标签）
UPSERT VERTEX "101" ON person SET name = "Alice", age = 25

-- ON tag + WHERE 指定 vid 的UPSERT
UPSERT VERTEX ON person SET age = 26 WHERE id(vid) == "101"

-- ON tag 形式携带额外过滤谓词（一并生效）
UPSERT VERTEX ON person SET age = 26 WHERE id(vid) == "101" AND age < 20

-- 边UPSERT
UPSERT EDGE "101" -> "102" OF follow SET degree = 0.9

-- 带rank的边UPSERT
UPSERT EDGE "101" -> "102" @1 OF follow SET degree = 0.9

-- UPSERT EDGE ON + WHERE 指定端点
UPSERT EDGE ON follow SET degree = 0.9 WHERE id(src) == "101" AND id(dst) == "102"
```

---

## 5. DELETE - 删除数据

### 功能
删除节点或边。

### 语法结构
```cypher
DELETE VERTEX <tag> FROM <vid> [, <vid> ...] [WITH EDGE]
DETACH DELETE VERTEX <tag> FROM <vid> [, <vid> ...]
DELETE EDGE <edge_type> <src_vid> -> <dst_vid> [@<rank>] [, ...]
DELETE EDGE <src_vid> -> <dst_vid> [@<rank>] OF <edge_type> [, ...]
```

### 关键特性
- **DELETE 之后必须紧跟 `VERTEX` 或 `EDGE` 关键字**：`DELETE VERTEX "101"`、
  `DETACH DELETE "101"` 等裸 ID 写法会被解析器直接拒绝
- 顶点删除形式为 `DELETE VERTEX <tag> FROM <vid>[, ...]`：tag 限定符用于定位顶点唯一的标签，
  语义是**删除整个顶点行**（存储层顶点恰好单标签），不是仅剥离该 tag 的数据；
  单独的 `DELETE TAG ... FROM ...` 语法不存在（解析拒绝）
- `WITH EDGE` 或 `DETACH` 级联删除关联边；两者等价，缺省时保留关联边（产生悬挂边）
- 边删除支持两种语法（边类型在前，或 `OF <edge_type>` 后置）；
  **`@rank` 端到端生效**，按给定 rank 删除对应边
- 独立 DELETE 不支持 WHERE 条件
- 支持在 MATCH 语句后直接接 DELETE 子句（见下文）

### 示例
```cypher
-- 仅删除顶点，保留关联边（可能产生悬挂边）
DELETE VERTEX person FROM "101", "102"

-- 删除顶点及其所有关联边
DELETE VERTEX person FROM "101" WITH EDGE

-- Cypher风格级联删除（仍需 VERTEX <tag> FROM 限定）
DETACH DELETE VERTEX person FROM "101"

-- 删除边（按 rank 删除）
DELETE EDGE follow "101" -> "102" @0

-- 删除边（OF后置语法）
DELETE EDGE "101" -> "102" @0 OF follow

-- 批量删除边
DELETE EDGE follow "101" -> "102" @0, "102" -> "103" @0
```

### MATCH + DELETE

MATCH 语句支持直接跟随 DELETE 子句，删除匹配到的数据。DELETE 由绑定结果驱动规划，
校验与执行使用同一份绑定输出。DELETE 之后同样必须跟 `VERTEX` / `EDGE` 关键字，
再接已绑定的变量或边引用表达式：

```cypher
-- 删除匹配到的顶点（级联删除边）
MATCH (p:Person {name: 'Alice'}) DELETE VERTEX p WITH EDGE

-- 删除匹配到的边
MATCH (a)-[e:KNOWS]->(b) DELETE EDGE e

-- 删除指定端点的边
MATCH (a:Person) DELETE EDGE a -> b @0
```

- 裸变量写法 `MATCH ... DELETE p` / `DELETE e` 会被解析拒绝
- MATCH 形式中 `WITH EDGE` 提供级联；`DELETE VERTEX <expr>` 要求表达式求值为带标签的
  顶点值（裸 vid 表达式会在执行期报错）

### 悬挂边说明
- 当删除顶点时不使用 WITH EDGE / DETACH，与该顶点关联的边会变成悬挂边（边的起点或终点不存在）
- 悬挂边可以通过GO语句查询到边属性，但点属性为空
- MATCH语句不会返回悬挂边
- 可以使用存储层API检测和修复悬挂边

---

## 6. MERGE - 合并数据

### 功能
如果存在则更新，不存在则创建（基于模式匹配）。点模式的条件执行已实现：
同一事务快照内先按标签加全属性相等做存在性探查，再执行命中的分支。
边模式携带动作子句会显式拒绝，不会静默丢弃。

### 语法结构
```cypher
MERGE (<variable>:<Label> {<prop>: <value>}) [ON CREATE SET <prop> = <value>] [ON MATCH SET <prop> = <value>] [SET <prop> = <value>]
MERGE EDGE|VERTEX ON <tag_or_edge_type> [SET <prop> = <value>] WHERE id(vid|src) == <value> [AND id(dst) == <value>]
```

### 关键特性
- 幂等操作
- 支持存在时更新（ON MATCH）、不存在时创建（ON CREATE）
- 双动作子句可按任意顺序各出现至多一次，重复出现报语法错误
- **`ON` 之后必须紧跟 `CREATE` 或 `MATCH` 关键字**，否则解析层报
  `Expected CREATE or MATCH after ON in MERGE statement`
- 无动作的合并保持 `IF NOT EXISTS` 插入路径
- 支持 UPSERT 风格的 `MERGE VERTEX/EDGE ON ...` 语法（内部按 UPSERT 目标规则执行，
  含第 4 节所述的 `id(...)` 提取与剩余谓词规则）

- 模式后的裸 `SET` 子句同时作用于创建与匹配两条分支，
  其赋值会合并进已有的 `ON CREATE` / `ON MATCH` 子句
- 条件分支复用插入与更新写入能力，不新增流式算子；
  无论走哪条分支，均返回统一的 `merged` 列，不透出内部分支列名差异
- 探查语义为标签加全属性相等，数值类型宽容比较
  （SmallInt/Int/BigInt 互比按数值相等）；模式属性必须为常量表达式，
  行变量、属性引用、子查询均明确拒绝
- 仅覆盖点模式；边模式携带动作因端点解析需要数据流、标量探查不可见，显式拒绝
- 探查与分支写入在同一事务作用域内完成；仅含 `ON CREATE` 时，
  第二次命中因无匹配动作而返回 `0`，不会重复创建
- 整映射覆盖（动作子句中 `SET v = {...}`）与 UPDATE 行为一致：
  整行替换目标属性，`{}` 清空全部属性
- `ON MATCH` / `ON CREATE` 的赋值表达式支持目标别名自引用
  （如 `SET age = v.age + 5`），按命中行属性解析；其他限定符保持原报错

### 示例
```cypher
-- 基本MERGE操作
MERGE (p:Person {name: 'Alice'})
ON MATCH SET p.last_seen = timestamp()
ON CREATE SET p.created_at = timestamp()

-- 带多个属性的MERGE
MERGE (p:Person {id: '101'})
ON MATCH SET p.name = 'Alice', p.updated = true
ON CREATE SET p.name = 'Alice', p.created = timestamp(), p.status = 'active'
```

点模式之外的边模式携带动作子句会明确失败，例如
`MERGE (a)-[r:KNOWS]->(b) ON MATCH SET r.weight = 1.0` 直接返回错误，
请改用点合并加边写入的组合。

---

## 7. SET - 设置属性

### 功能
设置或更新属性值。独立 `SET` 语句可解析、可绑定，但**没有标签上下文，
在计划层被显式拒绝**；UPDATE / UPSERT / MERGE 内部的 SET 子句是实际可用的写入路径
（语义见第 3、4、6 节）。

### 语法结构（解析层）
```cypher
SET <variable>.<prop> = <value> [, <variable>.<prop> = <value> ...]
SET <variable> = {<prop>: <value> [, ...]}  -- 整映射覆盖
```

### 关键特性（独立 SET 现状）
- 独立语句在计划层报错：
  `standalone SET cannot resolve a tag qualifier; use UPDATE ... ON <tag> SET instead`
- 该拒绝覆盖属性路径形式与整映射覆盖形式：独立语句没有上游行流，
  顶点写入缺标签上下文、行变量赋值不落存储，均不产生写入副作用
- 多个赋值同样被拒绝（不会出现只取第一个赋值的静默行为）
- 右值支持表达式计算（子查询在计划期仍会被 EXISTS/IN 检查拒绝）

### 示例
```cypher
-- 独立 SET 当前被拒绝；持久化更新请使用带标签限定的 UPDATE
-- SET p.age = p.age + 1
--   报错：standalone SET cannot resolve a tag qualifier; use UPDATE ... ON <tag> SET instead

-- 正确写法
UPDATE "101" ON person SET age = age + 1
```

---

## 8. REMOVE - 移除属性

### 功能
移除节点或边的属性（独立语句）。当前仅完成语法解析与绑定，
执行链路未接入存储，**在计划层被显式拒绝**。

### 语法结构（解析层）
```cypher
REMOVE <variable>.<prop> [, <variable>.<prop> ...]
```

> 说明：`REMOVE <variable>:<Label>` 标签移除形态在表达式位置即被解析拒绝。

### 关键特性（独立 REMOVE 现状）
- 独立语句在计划层报错：`REMOVE is not wired to storage yet`
- 拒绝发生在规划阶段，不会产出只剔列不写存储的计划，也不产生任何写入副作用
- 属性移除暂无替代语句；如需清空属性，UPDATE 的整映射覆盖
  （`SET v = {}`）提供删除全部属性的能力

### 示例
```cypher
-- 语法可解析，但计划层拒绝
-- REMOVE p.temp_field, p.temp2
--   报错：REMOVE is not wired to storage yet
```

---

## 9. COPY - 批量导入/导出

### 功能
从 CSV 文件批量导入顶点/边数据，或将数据导出到 CSV 文件（导出已实现）。

### 语法结构
```cypher
COPY VERTEX <tag_name> FROM '<file_path>' [, '<file_path>' ...] [BY COLUMN] [WITH (HEADER [true|false] | NO HEADER [, DELIMITER '<char>'] [, BATCH_SIZE <n>] [, CSV])]
COPY EDGE <edge_type> FROM '<file_path>' [, ...] [WITH (...)]
COPY <tag_name> FROM ('<file_path>' [, '<file_path>' ...])   -- 文件列表也可用括号包裹
COPY TAG <tag_name> FROM ...                                  -- TAG 关键字等价于 VERTEX
COPY VERTEX|EDGE <name> TO '<file_path>' [WITH (...)]
```

### 关键特性
- 支持从单个或多个 CSV 文件导入
- `FROM` 导入 / `TO` 导出；`TO` 仅支持单个文件，且不接受 `BY COLUMN`
- `BY COLUMN`：多文件按列合并导入（仅 FROM）；可写在文件列表后或选项之后
- 选项可放在括号内或直接罗列（`WITH` 与括号均可省略；选项名与值之间的 `=` 可选）：
  - `HEADER true|false` 或 `NO HEADER`（默认有表头）
  - `DELIMITER '<char>'`（默认 `,`）
  - `BATCH_SIZE <n>` / `BATCH SIZE <n>`：批量提交大小，缺省 1000；
    解析层拒绝负值，0 在规划期报 `COPY batch_size must be > 0`
  - `CSV`：显式声明 CSV 格式（语法占位，不影响行为）
- 省略 `VERTEX`/`EDGE` 关键字时默认为顶点导入
- 导出按 tag / edge_type 全量扫描，NULL 值输出为空单元格；导出列头由 `HEADER` 控制

### 示例
```cypher
-- 从CSV导入顶点（默认有表头）
COPY VERTEX person FROM '/data/person.csv'

-- 多文件导入，自定义分隔符与批次
COPY EDGE follow FROM '/data/follow_1.csv', '/data/follow_2.csv'
  WITH (HEADER true, DELIMITER '|', BATCH_SIZE 1000)

-- 按列合并多文件
COPY VERTEX person FROM '/data/a.csv', '/data/b.csv' BY COLUMN

-- 导出
COPY VERTEX person TO '/tmp/person_out.csv'
```

---

## DML 语句对比总结

| 语句 | 主要用途 | 幂等性 | 条件支持 | 返回结果 | 适用场景 |
|------|---------|--------|----------|----------|----------|
| **INSERT** | 插入新数据 | 可选(IF NOT EXISTS) | 无 | 插入数量 | 批量导入、初始化数据 |
| **UPDATE** | 更新已有数据 | 否 | WHERE/WHEN | 修改统计（YIELD 被拒绝） | 属性修改、增量更新 |
| **UPSERT** | 插入或更新 | 是 | WHERE(限定用法) | 修改统计（YIELD 被拒绝） | 幂等写入、同步场景 |
| **DELETE** | 删除数据 | 否 | MATCH模式 | 删除统计 | 数据清理 |
| **MERGE** | 匹配则更新，否则创建 | 是 | 模式匹配 | merged 列（1/0） | 复杂条件的数据合并 |
| **CREATE** | Cypher 风格创建 | — | — | 数据形式被拒绝 | Schema 定义走 DDL |
| **SET** | 属性设置 | 否 | 无 | 独立形式被拒绝 | 建议使用 UPDATE 子句形式 |
| **REMOVE** | 移除属性 | 否 | 无 | 独立形式被拒绝 | 暂不可用 |
| **COPY** | 批量导入/导出 | 否 | 无 | 导入/导出统计 | CSV数据搬迁 |

### 选择建议

- **需要幂等性**: 使用 `INSERT ... IF NOT EXISTS` 或 `UPSERT`
- **需要读取更新后的数据**: UPDATE/UPSERT 的 YIELD 当前被拒绝，请再执行 LOOKUP / MATCH 读回
- **复杂条件的数据合并**: 使用 `MERGE`（基于模式匹配，点模式 + 动作子句）
- **简单的属性更新**: 使用 `UPDATE ... ON <tag> SET ...`（顶点必须带标签限定）
- **批量属性更新**: 逐 vid 调用带标签限定的 `UPDATE`（`UPDATE TAG` 批量形式被拒绝）
- **CSV 批量搬迁**: 使用 `COPY`
- **写入数据**: 使用 `INSERT` / `UPSERT` / 带动作的 `MERGE`；数据形式 `CREATE`、独立 `SET`、
  独立 `REMOVE` 均以明确错误拒绝，不会产生写入
