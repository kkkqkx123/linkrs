# 编译服务器基准测试报告总览（2c9b92f8）

## 运行条件

- 提交：2c9b92f8，工具链 rustc 1.99.1，release + bench profile
- 机器：Xeon E5-2667 v3，基准绑定 NUMA node1（`taskset -c 8-15,24-31` + `membind 1`），编译在 node0
- 方法：criterion 套件必须带隐藏参数 `--bench` 直接执行二进制（不带则只跑一次 smoke，不测量；本次已纠正），plain-main 套件跑自身中位数逻辑
- 数据集：冒烟量级（20k 顶点 / 50k 行），结论为防回归口径，非权威吞吐

## 总体结果

32 个套件中 31 个通过（含全部回归门 PASS），rollback B4 的 25000 边行因超时未完成（两行有效数据已记录）：

| 状态 | 套件 | 说明 |
|---|---|---|
| 通过 | 其余 31 个 | 含回归门全部 PASS |
| 已修复并重跑 | crash_recovery_bench | 同目录并发 flush 的 `*.tmp` 竞态 + checkpoint tmp 目录竞态 + scheduler 自动 checkpoint 与显式 checkpoint 互斥；进程内按目录串行锁 + bench 用 `begin_bulk_load` 会话抑制 trigger；clean reopen 109.6ms，WAL-tail reopen 1143ms，见 `storage.md` |
| 已修复并重跑 | storage_bench | 单 store 无限增长触发 CapacityExceeded；bench 改为每 100 次轮换 store；全绿（持久单边写 1.94ms/edge），见 `storage.md` |
| 环境失败→已解决 | hnsw_build_bench | 默认 `ulimit -n 1024` 下 EMFILE；放开到 65536 后 6 项全部完成（1000 点 workers_4 加速 2.4x，2000 点 3.1x），见 `vector.md` |
| 部分数据 | rollback_bench | B4 5000/10000 边行有效（写入 72/47ms per edge，commit/abort 可忽略）；25000 边行 90 分钟超时未完成（median-of-2 墙钟翻倍 + HDD fsync-bound），见 `storage.md` |
| 已修复并重跑 | fulltext_bench | tantivy 闭包 patch 扩大到 7 个 crate + receiver 闭包修复 + bench 内嵌 `block_on` 改同步；index 约 1.3s/iter，search 1.7~5.0µs，见 `core-fulltext.md` |

## 干扰窗口说明

约 14:38（本地时）起有他人 buildroot 任务同机运行，系统 load 升至 8~14。此前完成的 11 个套件（alloc 至 edge_scan）为干净数据；之后完成的套件与之共享 CPU/磁盘，criterion 中位数与离群剔除可抑制波动，数字仍可作回归基线，但方差偏大，不宜与干净窗口数字做小数点级对比。rollback 受影响最大（/tmp 在被争抢的根分区上），需在安静窗口重跑。

## 文件索引

- `query.md`：查询层 7 套件
- `storage.md`：存储层 13 套件
- `transaction.md`：事务层 2 套件
- `vector.md`：向量层 9 套件
- `core-fulltext.md`：serde 与 fulltext 缺失说明
- 原始日志：`raw-2c9b92f8/`（已 gitignore，含 quick/full 日志、`results/rerun-fix1-2c9b92f8/` 14 套件重跑日志与 RUN_INFO、构建日志、criterion 全量明细 556 组 estimates.json、plain-main 的 `results.txt`）

回归门结论：olap Q1~Q5、traversal 两项阈值全部 PASS。server 二进制端到端测试列为后续阶段，本次未执行。
