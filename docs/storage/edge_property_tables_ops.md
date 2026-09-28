# 边表与属性表运维说明

> 对应分析见 `docs/analysis/edge_property_tables.md`，分阶段方案见
> `docs/plan/edge_property_tables_phases.md`。本文只写线上行为与操作，
> 不写实现细节。

## 存储方向

- 边表建表时按策略确定方向：双向、出边单向、入边单向。
- 缺失方向的邻接读返回空集，这是设计契约，不是故障。
- 计划层用 `storage_direction`、`has_out_edges`、`is_direction_available`、
  `direction_note` 区分真空与未存储，执行层不要把两者混为一谈。
- 方向读写计数见 `DirectionUsageSnapshot`，长期单腿流量考虑窄化到单向。

## 历史纪元

- 属性版本链只保存在内存里，检查点只持久化当前值。
- 每次装载开启新纪元，纪元下界为装载时最大创建时间戳。
- 严格读（边 `try_get_projected_physical_*`、顶点
  `try_get_projected_batch`）对纪元下界之前的历史查询明确报错，
  不再返回当前值冒充历史。
- 宽容读保持原语义，时间旅行查询必须走严格口。

## WAL 撕裂尾

- 默认 fail-closed：撕裂尾拒绝打开，需要离线诊断与修复。
- `diagnose_edge_wal_at` 只读诊断，`repair_edge_wal_at` 与
  `repair_edge_wal_at_reported` 离线截断并报告。
- 无人值守的单机重启用 `load_with_wal_recovery` 加
  `EdgeWalRecoveryMode::TruncateTornTail`，返回的报告必须入库审计；
  有人值守保持 `Strict`。

## 二级索引

- 两档一致性：尽力档主写权威、失败计 lag 并回退段扫描；
  强一致档索引失败即主写失败，只用于小基数关键属性。
- `lag != 0` 即不可用，查询自动回退，结果集不受影响。
- lag 水印每个维护 pass 刷新 gauges，长时间 lag 用
  `report_index_status` 观察，用 `rebuild_index_if_needed` 恢复。

## 漂移审计

- 装载、冻结、压缩、权威回收共用同一审计门，
  孤儿映射、孤儿 CSR 行、存活权威孤儿任一非零即拒绝。
- `audit_and_report` 只读并上报三个计数器，它是全表 walk，
  只在排查时手动调用，不进定时任务。

## 时间戳保留位

- 0 与顶部两个哨兵不可分配，写入边界与装载校验都会拒绝。
- 见到保留戳错误先查调用方的时间戳来源（未提交事务、手工构造），
  不要绕过校验。

## 溢出侧存

- 标有溢出的列缺 sidecar 文件即拒绝装载，
  损坏文件同样拒绝，不再降级为空值。
- 阈值 `usize::MAX` 表示关闭溢出路由。

## 回收与检查点

- 写径回收每 pass 有界，删 churn 表靠后台维护收敛，
  不要调大单 pass 上限去追延迟。
- 墓碑计数为缓存值，`tombstone_stats` 的新旧界仍走全量扫描，
  只用于观测，不进写径门限。
- 压缩按水位单 pass，跨表水位钉死时先查快照泄漏，
  不要反复手动触发压缩。
