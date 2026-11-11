# devs 服务器同步与远程编译说明

本地 `target/` 已达数十 GB，全量编译耗时高；devs（`ssh devs`）持有暖构建缓存，
且是基准测试的执行机。代码同步到 devs 编译，本说明约定同步方式。
参考基准侧的数据同步方案见 `graphdb-ben/docs/spec/sync-plan.md`（数据包走校验清单；
本仓库同步不使用校验和，见下文）。

## 目标位置

- 同步包存放：`/data/kkkqkx/linkrs/`（与历史 `linkrs-src-*.tar.*` 同目录）
- 代码解压位置：`/data/kkkqkx/linkrs/repo/`（直接覆盖，非 git 仓库）
- 构建日志：`/data/kkkqkx/linkrs/` 下按 `build-<主题>.log` 命名

## 传输方式选择

| 变更规模 | 内容 | 方式 |
|---|---|---|
| 小规模（数个文件） | 改动的源文件 | `scp` 直接覆盖远端对应路径 |
| 大规模（多文件/重构/首次） | 整个工作树 | `tar.gz` 打包 + `scp` + 远端覆盖解压 |
| 基准准备 | `graphdb-ben`（含 `data/`） | 单独压缩包，仅在需要跑基准时传输，日常代码同步排除 |
| 任何情况 | `target/`、`node_modules`、构建产物 | 永不传输 |

构建产物永不传输：devs 靠自身暖缓存增量编译；`target/` 体积大且与本地工具链绑定，
传过去也无法复用。

不使用校验和：按项目政策，同步包不用哈希校验，改用包内文件数与解压前后体积抽查。

## 打包排除清单

以仓库根为基准，打包时排除：`target/`、各级 `.git`（含 submodule 的 gitlink）、
`graphdb-ben/`（基准包另传）、`frontend/node_modules/`、各级 `node_modules/`、
`logs/`、`benches/results/`、`crates/linkrs-grammar/outputs/`、
`docs/archive/perf/raw-*/`、`__pycache__/`、本地运行遗留（`data/`、`test_logs/`）。
`Cargo.lock` 必须进包：devs 无外网，锁文件是离线可复现构建的前提。

## 同步步骤

本地打包（包名沿用历史约定，`sha` 取本地 HEAD 短哈希）：

```bash
cd /home/kkkqkx/code
tar -zcf ~/sync-pkgs/linkrs-src-<sha>.tar.gz \
  --exclude='*/target' --exclude='*/.git' \
  --exclude='linkrs/graphdb-ben' --exclude='*/node_modules' \
  --exclude='linkrs/logs' --exclude='linkrs/benches/results' \
  --exclude='linkrs/crates/linkrs-grammar/outputs' \
  --exclude='linkrs/docs/archive/perf/raw-*' \
  --exclude='*/__pycache__' --exclude='*/.mypy_cache' \
  --exclude='*.bak' linkrs
```

用 gzip 而不用 zstd：devs 上没有 zstd，gzip 处处可用。
`*/target` 覆盖各层嵌套构建目录（根 `target/`、各 workspace 的 `target/`、
`bindings/*/target`、`frontend/dist` 同理按需加）；`graphdb-ben` 无论代码还是
`data/` 都不进日常代码包。

记录构建信息（HEAD 哈希、日期、本地工具链、未提交变更），随包传输：

```bash
{ echo "sha=<sha>"; echo "date=$(date -u +%FT%TZ)"; rustc --version | sed 's/^/local_rustc=/';
  git -C ~/code/linkrs status --short; } > ~/sync-pkgs/linkrs-src-<sha>.BUILD_INFO.txt
scp ~/sync-pkgs/linkrs-src-<sha>.tar.gz ~/sync-pkgs/linkrs-src-<sha>.BUILD_INFO.txt \
  devs:/data/kkkqkx/linkrs/
```

远端覆盖解压并抽查（文件数对得上即视为完整）：

```bash
ssh devs "cd /data/kkkqkx/linkrs && tar -xzf linkrs-src-<sha>.tar.gz -C repo --strip-components=1 \
  && cp linkrs-src-<sha>.BUILD_INFO.txt repo/BUILD_INFO.txt \
  && tar -tzf linkrs-src-<sha>.tar.gz | wc -l && du -sh repo"
```

小规模变更直接传文件，路径与仓库根一一对应：

```bash
scp crates/linkrs-api/src/embedded/database.rs devs:/data/kkkqkx/linkrs/repo/crates/linkrs-api/src/embedded/database.rs
```

## 远端编译

devs 上 cargo 不在默认 PATH，验证脚本 `scripts/devs-check-trimming.sh`
已处理 PATH 与离线变量。传脚本、后台执行、日志落盘：

```bash
scp scripts/devs-check-trimming.sh devs:/tmp/
ssh devs 'nohup bash /tmp/devs-check-trimming.sh > /data/kkkqkx/linkrs/check-trimming-<sha>.log 2>&1 &'
```

脚本覆盖裁剪类变更的验收命令（见 `docs/issue/embedded-trimming-issues.md`）：
最小闭包 `cargo tree` 检查、各 feature 组合 `cargo check`、server 编译。
单次快速验证可直接在远端执行其中一条，无需走脚本。

## 反向取回

日志和结果体积小，直接取回：

```bash
scp devs:/data/kkkqkx/linkrs/check-embedded.log ./deploy/logs/
```

## 注意事项

- 覆盖解压不删除远端多余文件：改名或删除源文件后，需手动删除远端对应文件，
  否则旧文件仍会被编译。基准性结论以前建议用空目录全新解压一次复核。
- submodule（tantivy、simvec、llm-suite、gpui-kit、graphdb-ben）以包内 checkout 为准：
  devs 无外网无法执行 submodule 更新，升级 submodule 后必须走压缩包全量同步。
- `graphdb-ben` 日常不同步；跑基准前另打基准包（含 `data/`，`graph500-27` 这类
  特大数据仍在 devs 现场生成，见 graphdb-ben 的 sync-plan）。
- 非高峰时段传大包；单包超过约 2GB 改用 `rsync --partial --append-verify` 断点续传。
