# linkrs-query 超长文件职责分析与拆分建议

统计对象：`crates/linkrs-query/src` 下全部源码文件。
统计口径：文件总行数超过 800 行；再剔除文件尾部测试模块后得到生产代码行数，
只有生产代码仍超过 800 行的文件才进入拆分候选。测试模块的起点以
`mod *tests` 声明所在行判定。

使用命令：

- 初筛：`find crates/linkrs-query/src -type f -name "*.rs" | xargs wc -l | sort -n | awk '$1>800'`
- 剔除测试模块后复核：对初筛文件查找测试模块起始行，比较生产代码行数
- 职责判定：逐文件查看公开类型、函数清单与大函数行跨度，判断是否存在多个可独立命名职责

结论摘要：初筛 50 个文件，剔除纯测试文件与测试膨胀文件后，
生产代码超 800 行的文件共 27 个（含 1 个静态数据表）。
其中 15 个存在多个职责建议拆分，12 个为单一职责建议不拆。

## 建议拆分的文件

以下文件生产代码超过 800 行且内部包含两个以上可独立命名职责。
拆分方式遵循按职责拆分子模块、原文件保留为分发门面的原则，
不新建替代性模块。

### 聚合算子：一文件容纳四种算子

`executor/streaming/operators/blocking/aggregate_operator.rs`（1017 行，无内置测试）

现状：Aggregate、GroupBy、PartialAggregate、FinalAggregate 四种算子的
open、next、close、spill 全部放在同一文件。
其中按行跨度最大的两个 next 函数分别约 390 行与约 220 行，
各自维护独立的状态机与溢写路径，彼此几乎不共享逻辑，
仅共享 group key 求值一个小辅助。

拆分建议：按算子种类拆为四个子模块，共享的 group key 求值保留在公共位置，
原文件只保留按种类的分发。这是全仓内职责混杂最严重的文件，建议最先处理。

### 向量算子：单个 next 方法分发三类语义

`executor/streaming/operators/vector_operator.rs`（875 行）

现状：单一算子结构，但 next 方法独占约 600 行，
内部按 VectorManage、VectorSearch、VectorLookup 等变体分支，
分别承担索引 DDL 管理、向量检索、点查三类互不相关的语义，
检索分支还包含文本转向量、过滤、分页等自有逻辑。

拆分建议：按 manage、search、lookup 三类语义拆为三个子模块，
原文件保留算子结构与 next 分发。

### COPY：导入与导出两个方向

`executor/streaming/operators/copy.rs`（生产约 872 行）

现状：COPY FROM 导入路径（CSV 解析、列映射、顶点刷盘、边刷盘、
属性收集、VID 解析）与 COPY TO 导出路径（值序列化、CSV 行写出）
共处一文件，另有 CsvSource 与 MergedCsvSource 两套数据源抽象。

拆分建议：拆为导入、导出、CSV 数据源三个子模块。

### 累加器：生命周期、统计算法、分布式编解码三合一

`executor/streaming/helpers/accumulator_states.rs`（生产约 953 行）

现状：AggregateAccumulator 的生命周期方法（构造、累加、合并、终结）、
统计算法（Welford 在线算法、分位数、中位数、众数）与
分布式 partial 结果的编码解码（与 Value 互转、按函数恢复累加器）混杂。

拆分建议：拆为累加器主体、统计数学函数、partial 编解码三个子模块。

### 图函数：纯值函数与存储后端函数双轨并存

`executor/expression/functions/builtin/graph.rs`（生产约 923 行）

现状：id、tags、labels、properties、src、dst、rank 等纯值函数，
与 neighbors、degree、出入边、最短路、广度优先、连通分量、
变长路径、PageRank 等存储后端函数共处一文件，
后者每个函数还有纯内存与 storage 两个版本，文件内形成双轨重复。

拆分建议：按是否依赖存储拆为纯函数与存储后端两个子模块，
消除双轨并列。

### 连接重排：物理计划与逻辑计划两套重复路径

`optimizer/cost_based/join_order_rewriter/reorder.rs`（841 行）

现状：优化器输入构建、连接树重建、键解析、walk 优化四组逻辑
各有物理计划与逻辑计划两个版本，行为相近但类型不同，
是明显的按计划表示重复。

拆分建议：按物理与逻辑拆为两个子模块，键解析等无表示依赖的
公共逻辑下沉为共享模块。

### 维护执行器：十二个 SHOW 与维护语句共处

`executor/streaming/operators/ddl_operator/maintenance_executor.rs`（830 行）

现状：九个 execute_show 系列、LOAD FROM 导入、
IN 查询 CALL 执行、ANALYZE 四类语句执行器共处一文件，
每个执行器独立构造结果集，彼此无共享。

拆分建议：按语句族拆为 SHOW 语句族、LOAD FROM、CALL 与 ANALYZE 三个子模块，
列名与 JSON 转换等小辅助保留在公共位置。

### 分区规划器：多种分区策略共处

`optimizer/partitioning/planner.rs`（937 行）

现状：decide 总入口之下有多扫描、锚定遍历、边扫描等多种
分区决策策略，另有顶点链收集、独立分支切分、写操作与
事务边界守卫等辅助，各策略可独立命名与测试。

拆分建议：按策略拆分子模块，总入口保留策略选择与回退逻辑。

### DDL 规格：七类管理对象转换共处

`executor/streaming/plan/arena_builder/specs/ddl.rs`（生产约 988 行）

现状：space、tag、edge、index、user、fulltext、vector 七类管理对象
到命令规格的转换函数，以及向量查询转向量、全文查询字段校验等
各自独立的逻辑共处一文件。

拆分建议：按管理对象族分组拆分子模块，参数解析与字段校验等
通用逻辑保留在公共位置。

### 统计收集器：采集对象与采样数学混杂

`optimizer/stats/collector.rs`（生产约 836 行）

现状：collect_space 编排之下有 tag 采样、边类型采样、
属性统计三类采集逻辑，另有旋转偏移、窗口基数估计、
标准差与基尼系数等采样数学辅助。

拆分建议：按采集对象拆为 tag、边、属性三个子模块，
采样偏移与统计数学抽为独立辅助模块。

### 选择率估计：缓存键、估计器、表达式分发三合一

`optimizer/cost/selectivity.rs`（生产约 817 行）

现状：条件缓存键构造（条件键、范围分桶、边界量化）、
十余种谓词估计方法（等值、范围、大小比较、LIKE、IN、空值、
否定、函数表达式）与表达式树分发、统计抽取辅助共处一文件。

拆分建议：拆为缓存键、谓词估计器、表达式分发三个子模块。

### 全文算子：DDL 管理与检索执行两类语义

`executor/streaming/operators/fulltext_operator.rs`（生产约 888 行）

现状：全文索引管理命令的结果集构造，与检索执行
（引擎查询构建、引擎配置映射、引擎类型映射）共处一文件。

拆分建议：按管理与检索拆为两个子模块。

### 存储扫描：顶点与边、行与列四条路径

`executor/streaming/operators/source_operator/storage_scan.rs`（生产约 922 行）

现状：open 与 next 分发之下有顶点行路径、边行路径、
顶点列式块构建、边列式块构建四条相对独立的读取路径，
另有全局列块开关。

拆分建议：按顶点与边拆分，行路径与列式构建分离为各自辅助。

### 构建元数据：三种布局辅助共处

`executor/streaming/plan/arena_builder/metadata.rs`（生产约 974 行）

现状：expand 输出布局重命名与 bypass 列处理、
flat scan 列名集合、选择率回退与决策原因说明三组辅助
服务于不同算子族。

拆分建议：按服务的算子族拆为 expand 布局、scan 列名、
决策说明三个子模块。

### 编译器：元数据上下文构建过于庞大

`pipeline/compiler.rs`（941 行）

现状：执行计划优化编排、物理计划构建、计划缓存服务之外，
元数据上下文构建独占约 250 行，另有参数 schema 与
依赖表收集等相对独立的辅助。

拆分建议：将元数据上下文构建与依赖表收集抽为独立子模块，
编译器保留优化与构建编排。

## 明确不拆的文件

### 单一函数族注册表：体积来自同类小函数数量

- `executor/expression/functions/builtin/container.rs`（生产约 889 行）：
  单一容器函数枚举加三十余个 execute 系列小函数，
  全部服务于同一函数族分发，属单一职责。
- `executor/expression/functions/builtin/utility.rs`（生产约 816 行）：
  同上，通用工具函数族，多个小函数之间无可独立命名的大职责。
- `executor/expression/functions/builtin/aggregate.rs`（生产约 786 行，
  接近阈值）：聚合表达式与聚合状态，十余个 calculate 系列方法
  是同一职责的不同函数变体，不拆。

### 中央分发器：拆分反而破坏内聚

- `executor/streaming/executor.rs`（940 行）：
  StreamingExecutor 中央枚举与生命周期分发，
  profiling、内存、溢写等是分发附带的横切能力，
  拆文件会把一次分发拆散，得不偿失。
  仅可在方法内提炼计时辅助。
- `executor/streaming/operators/blocking.rs`（808 行）：
  同为 blocking 算子分发壳，不拆。
- `optimizer/engine/optimizer.rs`（812 行）：
  多阶段优化编排本身就是单一职责，不拆。

### 单一算子或单一重写器：大的是方法不是职责

- `executor/streaming/chunk/columnar_batch/operations.rs`（918 行）：
  单一 BatchColumn 实现，体积主要来自按类型展开的大 match，
  应做方法内重构而非分文件。
- `optimizer/factorization/factorization_rewriter.rs`（906 行）：
  单一因子化重写器，不拆。
- `executor/streaming/operators/recursive_fragment_operator.rs`（924 行）：
  单一递归算子；fixpoint 驱动与算子外壳可在方法层面提炼，
  暂不分文件。
- `binder/bind/bind_expressions.rs`（876 行）：
  表达式绑定的唯一入口，被各语句绑定器共享；
  bind_inner_expr 的大 match 可在需要时提辅助函数，保留单文件。
- `pipeline.rs`（811 行）：
  管道管理器的 builder 外观，约四十个配置方法；
  可选将纯 setter 聚拢为 builder 子模块，非必须。
- `executor/streaming/operators/ddl_operator.rs`（811 行）：
  分发加 sequence、macro、type 三类 manage 执行，
  刚过阈值，可选下沉三类执行函数，暂不强制。

### 静态数据表：行数大但只有一个职责

- `parser/lexing/spec.rs`（生产约 1725 行）：
  几乎全部是关键字表、上下文关键字表、符号表等静态数据，
  加十余个小型查询函数，是典型的词法规格数据源。
  体积来自数据行数而非职责数量，不拆。

### 测试膨胀：生产代码实际不足阈值

以下文件总行数超 800 但生产代码不足 800，
超长部分来自同文件测试模块，无需拆分：

- unary_operator（生产约 418 行）、chunk/collector（约 492 行）、
  scan_identity（约 493 行）、match_statement_planner（约 508 行）、
  fulltext_planner（约 509 行）、apply_operator（约 604 行）、
  expression_evaluator（约 630 行，测试模块名为 higher_order_tests）、
  composite_index_analyzer（约 632 行）、vector_parser（约 662 行）、
  compiled（约 672 行）、materializer（约 674 行）、
  exchange_operator（约 705 行）、materialize_operator（约 710 行）

### 纯测试与基准文件：不计入拆分范围

- 以 tests 结尾的单元测试文件 10 个、
  `tests/` 下集成测试、`benches/` 下基准：
  行数大是测试数据与用例数量所致，不按生产代码处理。

## 落地顺序建议

先处理职责边界最清晰、收益最大的文件：
聚合算子、向量算子、COPY、累加器、图函数、连接重排。
再处理规划与统计类：维护执行器、分区规划器、DDL 规格、
统计收集器、选择率估计。
全文算子、存储扫描、构建元数据、编译器放在最后，
这些文件内部耦合相对紧，拆分时需要更小心地确认边界。
