# 已编译计划 memo 后续集成与 DML 缓存重构分析

## 1. 基线与结论

前一任务已提交（`2f828f18 add compiled-plan-memo`）。devs 上已完成：

- `scripts/devs-compiled-plan-memo.sh` 全部步骤退出码 0；
- 目标查询 A/B：`distinct_limit` 热编译 0.706 ms → 0.017 ms，`group_by_count` 2.154 ms → 0.060 ms，结果逐值一致；
- 非分区查询 memo 命中 0/5，继续走 plain cache，`plan_cache_hits` 两模式均为 5/5；
- 补充基线：`dml_shape` 单测 22/22、`memo_skip_bind` 4/4 通过，日志 `/data/kkkqkx/linkrs/dml-shape-baseline.log`。

结论：**后续应做集成重构，DML 缓存应纳入重构目标，但只重构缓存机制，不重写 `dml_shape.rs` 的归一化语义。** 原因是只读 compiled memo 与 DML memo 已经出现同构的 key、find/store、checked-serve、清除与失效逻辑；继续各自演进会产生语义漂移。两类 memo 仍应保留独立实例与独立计数，避免互相挤占容量。

## 2. DML 缓存现状

当前 DML 复用由三层组成：

1. **模板层**：`planning/dml_shape.rs` 把字面量替换为 `@__dml_N`，生成规范化文本与有序字面量值；`dml_template_ast` 按规范化文本缓存重解析后的 AST。该缓存与 schema 无关，DDL 后保留。
2. **预绑定计划层**：`DmlPlanMemoKey` 用规范化文本加 `PlanCacheContext` 构造 fingerprint-less plan key；prepare 阶段命中可跳过 bind；编译阶段经 `serve_checked` 后 serving。容量 16，独立命中/失败/bind-skip 计数。
3. **计划缓存层**：`QueryPlanCache` 保存物理计划，DML 条目以 `is_dml` 标记；DML memo miss 时 plain cache 可作为后备并回填 memo。

已有覆盖：冷/热 bind-skip、DDL 失效、空间切换、参数类型变化、批量 20000 条 DML 的命中数、模板 AST 解析次数上界、各类 DML 语句的归一化往返测试。

未覆盖或需要显式确认：staged write 与普通读上下文切换、read/DML 交错时的容量隔离、checked-serve 失败注入、ANALYZE 对 DML memo 的影响、DDL 后模板 AST 保留行为与新计数器的联动。

## 3. 是否纳入重构目标

**纳入。** 但范围限定如下：

### 3.1 应重构

- 统一从 `PlanCacheContext` 派生预绑定 key 的构造入口；继续复用 `PlanCacheKey::fingerprint_less_from_context`，不得再出现第二套 key 语义。
- 统一预绑定 memo 的 find/store/contains/clear 容器逻辑，抽成按职责命名的通用组件；保留 read 与 DML 两个独立实例、独立容量与独立计数。
- 统一 prepare 阶段探测与编译阶段 serving 的调用形态：prepare 只做无副作用存在性探测，编译阶段统一经 `serve_checked` 出口。
- 统一失效入口：DDL/全局切换清所有预绑定实例；完整清缓存时再按既有策略清模板 AST。
- 统一指标命名结构：保留 `dml_plan_memo_hits`、`compiled_plan_memo_hits`、两类失败计数；bind-skip 计数按 namespace 拆分，避免把 read 命中混入 DML 指标。

### 3.2 不重构

- `planning/dml_shape.rs` 的渲染规则：已有 22 个单测覆盖 INSERT/DELETE/UPDATE/MERGE/SET/REMOVE 及不可渲染表达式的拒绝路径，不是当前漂移源。
- 模板 AST 缓存的生命周期：它解决 parse 成本，且 key 仅含规范化文本，必须继续在 DDL 后保留；不能与计划 memo 共用失效策略。
- 不把 read 侧的统计 stamp、统计代次、反馈指纹强加到 DML key。DML planner 当前不读取 `StatsView` 或 `CostCalculator`，加入这些维度只会无谓降低批量 DML 命中率。若将来 DML planner 开始使用统计，再单独评估。
- 不把两类 memo 合并成一张 LRU 表。批量 DML 与只读热查询交错时会互相淘汰；采用“一个组件、两个实例”。

## 4. 前置验证步骤

重构前先在 devs 上建立可对照基线。按 `docs/dev/build/devs-sync.md` 同步当前 HEAD（已提交，可走压缩包全量同步），记录 HEAD 与构建日志。

```bash
ssh devs 'export PATH="$HOME/.cargo/bin:$PATH"; export CARGO_NET_OFFLINE=true; \
  cd /data/kkkqkx/linkrs/repo && bash scripts/devs-compiled-plan-memo.sh \
  > /data/kkkqkx/linkrs/compiled-plan-memo-baseline.log 2>&1'
```

补充 DML 专项基线：

```bash
ssh devs 'export PATH="$HOME/.cargo/bin:$PATH"; export CARGO_NET_OFFLINE=true; \
  cd /data/kkkqkx/linkrs/repo && { \
    cargo test -p linkrs-query --lib -- dml_shape; \
    cargo test -p linkrs-query --test dml -- memo_skip_bind; \
    cargo test -p linkrs-query --lib -- cache; \
    cargo test -p linkrs-api --lib; \
  } > /data/kkkqkx/linkrs/dml-memo-baseline.log 2>&1'
```

基线通过标准为：所有步骤退出码 0；`dml_shape` 22/22、`memo_skip_bind` 4/4；只读 memo A/B 结果与前次一致。

建议在重构前把以下场景补成永久测试或临时验证用例：

1. **读上下文切换**：同一规范化 DML 形状在普通 autocommit 与显式写事务/group commit 下各执行一次；第二次必须 miss，写入结果不得串上下文。
2. **ANALYZE 不 invalidate DML**：同一形状 DML 命中后执行 `ANALYZE SPACE`，再次执行仍命中，`dml_plan_memo_hits` 不因统计刷新而重置。
3. **命名空间隔离**：交替执行两个只读分区查询和两个 DML 形状；两个实例的命中数分别增长，不因对方进入而淘汰。
4. **checked-serve 失败注入**：提升 schema/layout 代次使缓存计划失效；read 与 DML 的失败计数分别增加，下一次执行重编译且结果正确。
5. **DDL 后模板 AST 保留**：DDL 后 `dml_template_ast_parse_count` 不再增长，`dml_plan_memo_hits` 先归零再重新升温。

## 5. 集成实施顺序

1. **抽取通用预绑定 memo**：在既有缓存职责范围内定义 namespace、key、entry 与有界 LRU 操作；删除 `find_dml_plan` / `store_dml_plan` / `find_compiled_plan` / `store_compiled_plan` 的重复实现。若 `pipeline.rs` 继续膨胀，再按职责拆到 `cache/prebind_plan_memo.rs`，不创建“新版”模块。
2. **收敛 key 构造**：read 实例保留统计 stamp、统计代次、反馈指纹、layout version；DML 实例保留规范化文本 + `PlanCacheContext`。两者共用同一 fingerprint-less 构造入口。
3. **收敛 serving**：一个 `serve_prebind` 调用 `serve_checked`，按 namespace 累加命中与校验失败计数；prepare 的探测保持非 serving，实际计数只在编译路径发生。
4. **收敛失效**：DDL、全局 catalog 变更、完整 clear 走统一入口；模板 AST 继续按原生命周期处理。
5. **收敛可观测性**：bench JSON 增加 `dml_plan_memo_hits`，与既有 `compiled_plan_memo_hits` 并列；保留现有公开计数器语义，不引入兼容别名。
6. **保留开关**：read 侧 `LINKRS_NO_COMPILED_PLAN_MEMO` 与 DML 侧 `with_dml_shape_cache(false)` 都保留，便于分 namespace A/B 与故障隔离。

## 6. 重构后验证

- 重跑第 4 节的全部基线命令与新增场景。
- 重跑第 1 节的 A/B：目标查询热编译下降幅度与基线一致（允许采样噪声），结果逐值一致；`plan_cache_hits` 与两个 memo 命中计数之和可解释。
- DML 批量门：`dml_plan_memo_hits > 15000`，`dml_template_ast_parse_count <= 3` 仍成立。
- 失效门：DDL、空间切换、参数类型切换、staged write 切换、checked-serve 失败注入后下一次均重编译并得到正确结果。
- 计划形状门：冷执行 EXPLAIN 与重构前一致；热执行结果与冷执行逐值一致。

## 7. 风险与控制

- 最大风险是共享 LRU 造成 read/DML 互相淘汰；用同组件双实例规避。
- 第二个风险是把 read 的统计/反馈维度误加到 DML，降低批量命中率；以 DML planner 不读统计的现状为依据保持 DML key 不变。
- 不得把模板 AST 缓存并入计划 memo；两者失效语义不同。
- 不得出现第二套 checked-serve；所有 serving 仍走 `QueryPlanCache::serve_checked`。
- 不做向后兼容层；旧字段和旧方法在统一完成后直接删除。
- 不引入“version 2”模块；新组件按职责命名。

## 8. 验收判定

全部前置基线与重构后验证通过、A/B 收益不退、DML 批量门保持时，正式应用该重构；任一门失败则只回滚对应步骤，不撤销已提交的只读 compiled memo 功能。
