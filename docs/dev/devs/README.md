# devs 机器环境速查

**机器**: `ssh devs`（编译 + 基准执行机，无外网，无 sudo）
**快照日期**: 2026-11-12（实测值，后续变更请更新本页）

## 机器快照

| 项目 | 值 |
|------|-----|
| 系统 / 内核 | Ubuntu 18.04.5 LTS，`4.15.0-213-generic`，x86_64 |
| CPU | 2× Xeon E5-2667 v3，32 逻辑 CPU，3.2GHz，均支持 AVX2；NUMA 2 节点 |
| 内存 | 31GB |
| 磁盘 | `/data` 9.1T，可用 7.7T；家目录配额小，大文件一律放 `/data` |
| glibc | 2.27（选择预编译包时的下限红线） |
| Rust | cargo/rustc 1.99.1（`~/.cargo/bin`，非登录 shell 需手动加 PATH，`CARGO_NET_OFFLINE=true` 构建） |
| C 工具链 | gcc 7.5.0、cmake 3.10.2、make/perl 具备（C++20 源码构建不够，cmake 3.15+ 需求满足不了） |
| 系统 Python | 3.6.9 且无 pip（**不可用**，见 bench-env.md） |
| 频率策略 | 全核 `powersave`，不可调；ASLR 全局开（单进程可用 setarch 关） |
| 共享说明 | 与他人共用（rabbitmq/mongod 常驻、他人构建任务偶发）；编译与基准互斥，负载高时等待 |

## `/data/kkkqkx` 布局

```
/data/kkkqkx/
├── linkrs/          # linkrs 源码 + 构建（repo/、build-*.log），见 devs-sync.md
├── graphdb-ben/     # 基准区：代码/数据/wheels/venv/结果
└── tools/           # 用户态工具：python3.11、uv、openssl3、brotli（见 bench-env.md）
```

## 文档索引

| 文档 | 内容 |
|------|------|
| `docs/dev/devs/bench-env.md`（本目录） | 基准 Python 环境：版本清单、构建记录、已知问题、重建步骤 |
| `docs/dev/build/devs-sync.md` | linkrs 代码同步规范（日常包排除 `graphdb-ben/`） |
| `docs/tests/remote_bench_server_design.md` | 绑核隔离与运行流程（基准独占 node1、构建限 node0） |
| `docs/tests/compile_server_build_env.md` | 远端编译规范与 sudo 政策 |
| `docs/tests/benches/devs_lbug_neug_parity_design.md` | 三引擎对比总体设计 |

## 硬约束（速记）

- 大包非高峰传，超约 2GB 用 `rsync --partial --append-verify`。
- 预编译二进制/.so 必须兼容 glibc 2.27；Arch/新发行版产物大概率不可直接用。
- Python 相关一律走 `/data/kkkqkx/tools` + venv，不动系统 Python。
