# devs 基准 Python 环境（lbug / neug）

**落成日期**: 2026-11-12
**原则**: uv 创建独立 venv，离线安装，不动系统包（系统 Python 3.6.9 无 pip，不可用）。

使用前只需：`source /data/kkkqkx/graphdb-ben/bench-env.sh`（含 `LD_LIBRARY_PATH`、`DRY_RUN=true`）。

## 落盘位置

| 路径 | 内容 | 体积 |
|------|------|------|
| `/data/kkkqkx/tools/python3.11` | CPython 3.11.18（uv standalone 打包） | 95MB |
| `/data/kkkqkx/tools/uv` | uv 0.13.7（单二进制，venv 创建与离线安装用） | 36MB |
| `/data/kkkqkx/tools/openssl3` | OpenSSL 3.0.16（源码构建，`lib64/`） | 27MB |
| `/data/kkkqkx/tools/brotli` | brotli 1.2.0（gcc 直编，`lib/`） | <1MB |
| `/data/kkkqkx/graphdb-ben/.venv-bench` | 基准 venv（`--python python3.11`） | 106MB |
| `/data/kkkqkx/graphdb-ben/wheels/wheelhouse311/` | cp311 wheelhouse（本地 `pip download`） | 31MB |
| `/data/kkkqkx/graphdb-ben/packages/` | 传输包缓存：`py311.tar.gz`、`wheelhouse311.tar.gz`、`uv-x86_64`、`openssl-3.0.16.tar.gz`、`brotli-1.2.0.tar.gz`、`bench-env.SHA256SUMS`、`build-deps.log`、`smoke_bench.py` | — |
| `/data/kkkqkx/graphdb-ben/bench-env.sh` | 环境一键生效脚本 | — |
| `/data/kkkqkx/graphdb-ben/data/` | 现有数据：`social-network-sf0.1-projected-fk`（52MB） | — |

## 版本清单（venv，`uv pip list`）

`ladybug==0.22.3`、`neug==0.3.1`、`protobuf==5.29.6`（精确 pin）、`psutil==7.2.2`、
`packaging/click/prompt_toolkit/tabulate/PyYAML/tqdm/Flask/Flask-Cors/requests` 等依赖随 wheelhouse 全量锁定。

## 构建记录（2026-11-12）

- 背景：ladybug wheel 的扩展缺 `libssl.so.3` 与 `libbrotli*.so.1`；Arch 预编译 .so 需 GLIBC_2.33+，devs（2.27）不可用。
- OpenSSL 3.0.16：`./Configure --prefix=… no-tests` + `make build_sw` + `make install_sw`。
  注意 3.0 的 Configure **不支持** `no-docs`（3.2+ 才有），用 `build_sw/install_sw` 跳过文档。
- brotli 1.2.0：cmake 要求 ≥3.15（devs 为 3.10），改 gcc 对 `c/common`、`c/dec`、`c/enc` 直编三个 `.so.1` 并建软链。
- 自构建物最高仅需 GLIBC_2.14，兼容 devs；构建限 node0 后台执行，日志见 `packages/build-deps.log`。

## 已知问题

1. **导入名**：PyPI wheel 顶层模块为 `ladybug`，`import lbug` 失败；vendored `lbug-ben` 的 `import lbug` 在 wheel 路径下不可用，动用前须加垫片并更正 `VENDORED.md`。
2. **neug 计时**：`QueryResult` 无 compiling/execution 内计时，用外部 wall-clock 包裹全物化迭代；`to_arrow()` 需 pyarrow（未装），用原生 `__iter__` 物化。
3. **neug 线程**：`max_thread_num` 仅约束 TP 服务槽位，AP 查询当前单线程实现，报告中如实声明。
4. **ladybug wheel** 标注 `manylinux_2_27`，恰卡 devs glibc 下限——已验证可建库执行；若后续换版先做同款冒烟。

## 验证记录（LSQB-sf0.1，Person+knows）

脚本：`packages/smoke_bench.py`（DDL→COPY→COUNT→1 跳 COUNT）。

| 引擎 | Person COUNT | knows COUNT | 备注 |
|------|--------------|-------------|------|
| neug | 1700（21.2ms wall） | 18135（2.6ms wall） | 与 CSV 行数-表头一致 |
| ladybug | 1700（exec 2.54ms） | 18135（exec 0.26ms） | `get_compiling_time/get_execution_time` 正常 |

## 重建步骤（本地有外网处备包 → devs 离线恢复）

1. 本地：`uv python install 3.11` 打包安装目录；`pip download ladybug==0.22.3 neug==0.3.1 psutil` 备 wheelhouse；`SHA256SUMS` 记录。
2. 传输至 `packages/` 并校验。
3. devs：解压至 `tools/` 与 `wheels/`；`uv venv --python tools/python3.11/bin/python3.11 .venv-bench`；
   `uv pip install --offline --no-index --find-links wheels/wheelhouse311 ladybug neug psutil`。
4. 按需重建 openssl/brotli（源码包已在 `packages/`，构建脚本见 `build-deps.log` 头部思路与本文构建记录）。
5. `source bench-env.sh` 后跑 `smoke_bench.py` 复验。
