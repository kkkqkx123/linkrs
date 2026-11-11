#!/usr/bin/env bash
# devs 远端裁剪验证脚本：在 /data/kkkqkx/linkrs/repo 下执行。
# 用法（本地）：scp scripts/devs-check-trimming.sh devs:/tmp/ && ssh devs 'nohup bash /tmp/devs-check-trimming.sh > /data/kkkqkx/linkrs/<log> 2>&1 &'
# 覆盖 embedded 裁剪的验收命令：最小闭包 tree 检查 + 各 feature 组合 check + server 编译。
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo
echo "=== T1 tree minimal ==="
cargo tree -p linkrs-api --no-default-features --features embedded -e normal 2>&1 | grep -E "linkrs-migration|linkrs-fulltext|tantivy |simvec|vector-client|jieba"
echo "T1 done"
echo "=== C1 check api minimal ==="
cargo check -p linkrs-api --no-default-features --features embedded 2>&1 | tail -n 4
echo "=== C2 check api all features ==="
cargo check -p linkrs-api --no-default-features --features embedded,migration,fulltext,c_api 2>&1 | tail -n 4
echo "=== C3 check api embedded c_api ==="
cargo check -p linkrs-api --no-default-features --features embedded,c_api 2>&1 | tail -n 4
echo "=== C4 check server ==="
cargo check -p linkrs-server 2>&1 | tail -n 4
echo "ALL DONE"
