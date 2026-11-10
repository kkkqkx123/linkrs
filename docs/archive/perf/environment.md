# 基准测试环境参数（编译服务器 devs，2c9b92f8 轮）

本文件记录全部环境参数。任何脱离本文件的数字对比均视为无效。

## 机器与系统

| 项目 | 取值 |
|---|---|
| 主机 | itc（10.10.20.116），Ubuntu-Server 18.04.5，内核 4.15.0-213-generic，x86_64 |
| CPU | 2 × Xeon E5-2667 v3 @ 3.20GHz（8 核 16 线程每颗，共 32 逻辑 CPU，AVX2，L3 20MB×2） |
| 实测频率 | powersave 策略下约 3.40GHz（基准 3.2GHz，睿频上限 3.6GHz，下限 1.2GHz），intel_pstate 驱动，可选策略仅 performance/powersave |
| 内存 | 31GB（node0 约 15.9GB，node1 约 16.1GB），无 NUMA 交错 |
| 磁盘 | sdb 9.1T HDD（WDC WD100EAGZ，/data，ext4，测试数据与仓库均在此）；sda 1.8T HDD（LVM，/ 与 /tmp，ext4）；两块皆为机械盘，无 SSD |
| 交换 | /swap.img 文件 8G（使用中约 400MB） |
| glibc / gcc | glibc 2.27，gcc 7.5.0 |
| ASLR | 全局开启（randomize_va_space=2），单进程可用 setarch 关闭（本轮未用） |
| 大页 | THP 模式 madvise，defrag madvise（默认，未调） |
| 中断 | timer/CPU0、acpi/CPU10 等集中在 node0 前段核，node1 干净 |

## NUMA 与隔离

| 项目 | 取值 |
|---|---|
| node0 | 0-7,16-23（超线程配对 0-16 … 7-23），留给系统、编译、他人任务 |
| node1 | 8-15,24-31（配对 8-24 … 15-31），基准独占 |
| 跨节点距离 | 21（节点内 10），禁止跨节点运行 |
| 绑定命令 | `numactl --cpunodebind=1 --membind=1 taskset -c 8-15,24-31` |
| 无 root 可用手段 | taskset / numactl / setarch / nice（仅调低）；cpuset、isolcpus、调频、irq 迁移、清页缓存均不可用 |

## 工具链与构建

| 项目 | 取值 |
|---|---|
| rustc / cargo | 远端 stable 1.99.1（`~/.cargo/bin`，非登录 shell 需手动加 PATH） |
| 构建配置 | `.cargo/config.toml` 基线 x86-64 + 运行时分发（本地/远端 CPU 均有 AVX2，自动用上）；无额外 RUSTFLAGS |
| profile | 无 `[profile.bench]` 覆写，bench 沿用 release 口径（opt-level 3，strip symbols）；dev 仅本地用 |
| 依赖源 | crates.io 稀疏索引直连；tantivy 六包 patch 到 `crates/tantivy/*` 本地快照（与 crates-io 同版本号、API 更新） |
| 构建约束 | 编译限定 node0（`taskset -c 0-7,16-23 --jobs 8`），后台落盘日志 |

## 运行参数

| 项目 | 取值 |
|---|---|
| 环境变量 | `QUICK=1` 仅冒烟（测量 800ms/采样≤10/预热 200ms）；全量默认（测量 10s/预热 1s/包定 sample 数）；`RUST_TEST_THREADS=1` |
| criterion | 0.5.1；二进制直接运行时必须带隐藏参数 `--bench`（否则只跑一次 smoke，不测量不存档——本轮已纠正） |
| fd 上限 | 非登录 shell 默认 1024（硬上限 1048576）；HNSW 构建类需 `ulimit -n 65536`，已写入运行脚本 |
| 临时目录 | TempDir 默认走 `/tmp`（根分区 HDD）。根分区被争抢时改设 `TMPDIR=/data/kkkqkx/linkrs/tmp` |
| 数据集 | 冒烟量级：query/parallel 20k 顶点 60k 边，ingest/rollback 50k 行，crash 2000 顶点，vector 1k~10k 点；公开数据集大规格为后续工作 |
| 超时保护 | 单套件 `timeout 2400`，超时按中断记录、保留已产出部分 |

## 干扰记录（本轮）

约 14:38（本地时）起同机有 rk3568 buildroot 构建，load 升至 8~14，根分区 util 达 95%。14:38 前完成的 11 个套件为干净数据；之后完成的套件与之共享 CPU/磁盘，criterion 中位数与离群剔除可抑制波动，仍可作回归基线，但不宜与干净窗口数字做小数点级对比。rollback 受影响最大（/tmp 在被争抢的根分区上），已在安静窗口用 TMPDIR 重跑。
