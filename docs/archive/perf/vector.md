# 向量层基准数据（2c9b92f8，编译服务器 node1，AVX2 生效）

## distance_bench（距离内核）

128 维：Dot 约 13.1ns，余弦/欧氏约 14ns；256 维约 22~25ns；512 维约 56~77ns。随维度近线性增长。

## vector_scan_bench（暴力扫描）

n 规模：余弦 50.2µs（39.8 Melem/s），欧氏 36.6µs（54.6 Melem/s），点积 34.5µs（58.0 Melem/s）；10k 规模：余弦 286µs（35 Melem/s），欧氏 180µs，点积保持最高吞吐。

## hnsw_ingest_bench

256 维约 38.7ms（6.6 Kelem/s），512 维约 42.7ms（6.0 Kelem/s）。

## ivf_bench

索引构建约 233ms；查询随规模从 96µs（小）涨到 1.5ms~56ms（大）。

## persist_crc_bench

10k 点 CRC 约 3.2µs；大批量段约 47µs。

## concurrent_search_bench

单 worker 约 7.8ms（8.2 Kelem/s），扩展后约 2.4ms（26 Kelem/s）、1.2ms（55 Kelem/s），扩展性良好。

## concurrent_write_bench（写入端扩展性存疑）

store/engine/engine+pl 三种配置下，1→8 writer 吞吐从约 10k 上到 25k~35k，但 4 writer 起成功率从 10/10 跌到 10/6，8 writer 跌到 10/3。读扩展没问题，写扩展伴随成功率下降，需结合业务语义确认是否为预期背压。

## alloc_stats_bench（1 万点 64 维 + 200 查询）

ingest 约 13 万次分配（31.9MB），HNSW 构建约 20.8 万次（168MB），单次未过滤搜索约 5444 次（2.7MB）。构建期分配是大头。

## hnsw_build_bench（fd 上限已解决，重跑完成）

首次运行在默认 `ulimit -n 1024` 下报 EMFILE 中断；放开到 65536 后 6 项全部完成：1000 点 sequential 337ms（2.97 Kelem/s）、single_pool 341ms（基本无提升）、workers_4 140ms（7.14 Kelem/s，加速 2.4x）；2000 点 sequential 790ms（2.53 Kelem/s）、single_pool 802ms、workers_4 255ms（7.85 Kelem/s，加速 3.1x）。结论：构建期并行有效（3x），单池与串行无差别。
