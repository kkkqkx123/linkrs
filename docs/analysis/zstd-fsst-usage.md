二、zstd 的使用方式
1. WAL (Write-Ahead Log) 压缩
位置: crates/graphdb-transaction/src/transaction/wal/writer/compression.rs
rust
// Zstd 压缩器实现
pub(crate) struct ZstdCompressor {
    level: i32,
    min_size: usize,  // 最小压缩阈值 (64 字节)
}

// 压缩逻辑：
// 1. 数据小于 min_size 时不压缩
// 2. 压缩后如果未变小则返回原始数据
// 3. 使用 zstd::encode_all 进行压缩

配置选项 (crates/graphdb-core/src/core/wal/types.rs):
zstd_default(): level 3 (默认)
zstd_fast(): level 1 (快速)
zstd_best(): level 22 (最佳压缩率)
zstd(level): 自定义级别 (1-22)
2. 存储层文件压缩
位置: crates/graphdb-storage/src/storage/compression.rs
文件格式:
[1-byte marker][payload]
- Marker 0x00: 原始数据
- Marker 0x01: [4-byte CRC32][4-byte compressed_len][zstd compressed data]

特点:
强制使用标记格式，不支持旧格式
包含 CRC32 校验和验证数据完整性
支持文件级压缩/解压工具函数
3. 依赖配置
toml
# Cargo.toml
zstd = "0.13"

三、FSST (Fast Static Symbol Table) 的使用方式
1. 核心实现
位置: crates/graphdb-storage/src/storage/encoding/fsst.rs
算法流程:
训练阶段: 从输入字符串中提取 2-8 字节的频繁序列作为符号
构建符号表: 最多 255 个符号，按频率×长度排序选择
编码: 贪心匹配最长符号，替换为单字节代码
解码: 查表还原，零拷贝友好
关键参数:
rust
const MAX_SYMBOL_LEN: usize = 8;
const MIN_SYMBOL_LEN: usize = 2;
const SYMBOL_TABLE_SIZE: usize = 255;
const MAX_TRAINING_SAMPLES: usize = 10000;  // 采样限制
const MAX_NGRAMS_PER_STRING: usize = 1000;  // 每字符串 ngram 上限

2. 列式存储集成
位置: crates/graphdb-storage/src/storage/vertex/column_store.rs
rust
pub fn apply_fsst_encoding(&mut self, max_symbols: usize) -> StorageResult<()> {
    // 仅支持 String 类型
    // 1. 提取所有字符串
    // 2. 训练 FSST 编码器
    // 3. 批量编码
    // 4. 转换为 FsstColumn 存储
}

FsstColumn 结构:
rust
pub struct FsstColumn {
    pub encoder: FsstEncoder,      // 符号表 + 编解码器
    pub encoded_ Vec<Vec<u8>>, // 编码后的数据
    pub null_bitmap: NullBitmap,    // 空值位图
}

3. 自动编码选择器
位置: crates/graphdb-storage/src/storage/encoding/selector.rs
rust
12345
触发条件:
行数 ≥ 50 (string_min_rows)
平均长度 ≥ 20 字节
基数率 > 0.5 (高基数场景)
四、设计合理性分析
✅ 合理的设计点
方面
评价
说明
分层压缩策略
✅ 优秀
WAL 用 zstd (通用)，列存用 FSST (字符串专用)，各司其职
FSST 适用场景
✅ 准确
针对高基数长字符串，与 Dictionary 形成互补
zstd 级别可调
✅ 灵活
提供 fast/default/best 三档，支持自定义
数据完整性
✅ 完善
zstd 压缩包含 CRC32 校验
小数据优化
✅ 合理
WAL 压缩有 64 字节阈值，避免负优化
训练采样
✅ 必要
FSST 限制 10000 样本，防止内存爆炸
自动选择器
✅ 智能
基于统计信息自动选择最优编码
⚠️ 潜在问题与改进建议
问题
严重性
建议
1. FSST 符号表持久化
中
当前 FsstColumn 包含 encoder，但需确认落盘时符号表是否正确序列化
2. FSST 动态更新缺失
中
apply_fsst_encoding 是一次性的，新增数据时符号表不会更新，可能导致新数据压缩率低。建议：定期重建或增量训练
3. 硬编码阈值
低
max_symbols: 1024 在多处硬编码，应通过配置管理
4. FSST 仅支持 String
低
当前只支持 DataType::String，但 Vector/Json 等也可能受益，可扩展
5. 缺少压缩率监控
低
无运行时压缩率统计，难以评估效果和优化
6. 并发训练风险
低
train 方法非线程安全，多线程环境下需加锁
🔧 代码质量问题
rust
// fsst.rs:142 - 潜在的 code 溢出风险
for (code, (ngram, _freq)) in (1_u8..).zip(ngrams) {
    if code as usize >= max_symbols.min(SYMBOL_TABLE_SIZE) {
        break;
    }
    // 如果 ngrams 超过 255 个，code 会溢出
}

建议修复:
rust
for (idx, (ngram, _freq)) in ngrams.into_iter().enumerate() {
    let code = (idx + 1) as u8;
    if idx >= max_symbols.min(SYMBOL_TABLE_SIZE) - 1 {
        break;
    }
    self.table.insert(ngram, code);
}

五、总结
整体评价: 设计合理，架构清晰
zstd 用于通用压缩 (WAL、文件)，利用其高压缩比和快速度
FSST 用于列式字符串压缩，填补了 Dictionary 编码在高基数场景的空白
两者形成互补的多层压缩体系
优先级改进建议:
🔴 添加 FSST 符号表的持久化测试验证
🟡 实现 FSST 编码的动态重建机制
🟢 将硬编码阈值改为配置项
🟢 添加压缩率指标监控
