# 查询层基准数据（2c9b92f8，编译服务器 node1）

## query_bench（Criterion + 自定义 B1~B7 探针）

微基准中位数：2-hop 1.55µs，3-hop 1.65µs；2000 顶点大扫描 3.25ms，10000 顶点 8.92ms；点查 get_vertex 3.1µs。

B 组探针（列式命中率全部 1.0，零溢出）：B1 scan_filter_20k（4205 万行输出）、B2 点查邻居、B3 聚合（2800 万行）、B4 sort_limit_10（峰值内存 2.3MB）、B5 two_hop_count、B6 edge_scan_count、B7 dml_cycle。D1 阈值均未触发（typed_rate 低于 100 万门限）。

## query_stage_bench（阶段拆分）

point 查询：parse 约 6.2µs，bind 约 5.6µs；plan+optimize 偏大（point 约 529µs，one-hop 约 915µs，two-hop 约 934µs），执行段 point/one-hop/two-hop 分别为 5.7µs、180µs、934µs 量级。结论：plan+optimize 在小查询上占主导，是后续优化的第一候选。

## traversal_perf_bench（回归门，PASS）

锚定 1 跳中位数 3ms（门限 100ms）；非锚定 2 跳串行 83ms、2 worker 66ms、4 worker 46ms（门限 2000ms）。随 worker 增加单调下降，并行有效。

## parallel_scale_bench（20k 顶点 / 60k 边）

- Q1 scan+filter+agg：9.3ms（1 worker）；2 worker 因行数低于分区阈值保持串行；4/8 worker 反而变慢（E(n) 0.65），分区开销大于收益。
- Q2 scan+groupby：24.4ms → 11.1ms，加速比 2.2x@8 worker，并行有效。
- Q3 2-hop：5.6ms，各 worker 数基本持平（展开行数低于阈值，维持串行，正确）。
- E1 union：53.8ms → 33.0ms@4 worker（1.63x）；E2 边扫描：66.8ms → 57.6ms@2 worker 后持平。

结论：分区阈值策略工作符合预期（小规模不分区），Q2/E1 显示 4 worker 附近仍有收益，8 worker 普遍收益衰减。

## olap_e2e_bench（回归门，Q1~Q5 全部 PASS）

5000 规模下中位数：Q1 非锚定 2 跳约 78ms，Q2 锚定 2 跳约 15ms，Q3 约 182ms，Q4 约 46ms，Q5 约 3.1ms。

## executor_bench（约 108 个微基准）

表达式求值 8~55µs（随行数从 128 涨到 1024）；单列过滤全行扫描 16384 行约 348ns 级（向量化路径）。全绿。

## columnar_probe_bench

edge_count_narrow 72.7ms，edge_count_wide 27.1ms（20% 离群，多为高值），two_hop 35.6ms。
