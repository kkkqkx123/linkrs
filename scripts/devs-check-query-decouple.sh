#!/usr/bin/env bash
# devs 远端全文解耦验证脚本：在 /data/kkkqkx/linkrs/repo 下执行。
# 用法（本地）：scp scripts/devs-check-query-decouple.sh devs:/tmp/ && ssh devs 'nohup bash /tmp/devs-check-query-decouple.sh > /data/kkkqkx/linkrs/<log> 2>&1 &'
# 覆盖 query 可选依赖全文后的验收：最小闭包 tree 无全文、query 双向编译、
# api 双向编译、query 单测（含门控全文测试与新增校验测试）、server 无回归。
set -u
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_NET_OFFLINE=true
cd /data/kkkqkx/linkrs/repo
echo "=== T1 tree minimal: expect no linkrs-fulltext ==="
cargo tree -p linkrs-api --no-default-features --features embedded -e normal 2>&1 | grep -E "linkrs-migration|linkrs-fulltext|tantivy |simvec|vector-client|jieba" || echo "T1 clean: none of the trimmed crates present"
echo "=== C1 check query minimal ==="
cargo check -p linkrs-query --no-default-features 2>&1 | tail -n 3
echo "=== C2 check query fulltext ==="
cargo check -p linkrs-query --no-default-features --features fulltext 2>&1 | tail -n 3
echo "=== C3 check api minimal ==="
cargo check -p linkrs-api --no-default-features --features embedded 2>&1 | tail -n 3
echo "=== C4 check api fulltext ==="
cargo check -p linkrs-api --no-default-features --features embedded,fulltext 2>&1 | tail -n 3
echo "=== T2 test query fulltext lib ==="
cargo test -p linkrs-query --features fulltext --lib 2>&1 | tail -n 6
echo "=== C5 check server ==="
cargo check -p linkrs-server 2>&1 | tail -n 3
echo "ALL DONE"
