# 结果处理模块 (Result Processing Module)

本模块包含查询结果处理相关的核心执行器，负责对查询引擎执行后得到的原始数据进行进一步处理和优化，包括筛选、投影、聚合、排序、限制等功能。

## 模块概述

结果处理模块提供了以下功能：

- **投影 (Projection)**: 选择和转换查询结果的列
- **排序 (Sorting)**: 对结果进行单列或多列排序
- **限制 (Limiting)**: 限制返回结果的数量（LIMIT）和偏移（OFFSET）
- **聚合 (Aggregation)**: 计算 COUNT、SUM、AVG、MAX、MIN 等聚合函数
- **去重 (Deduplication)**: 移除结果中的重复项
- **过滤 (Filtering)**: 对结果应用条件过滤（HAVING子句）
- **采样 (Sampling)**: 随机采样结果集（SAMPLING）
- **TopN (TopN)**: 返回排序后的前N项（优化的排序+限制）

## 文件说明

### 1. `mod.rs`
模块定义文件，导出所有结果处理执行器，并提供模块整体的文档说明。

### 2. `traits.rs`
**功能**: 统一的执行器接口定义。
- 定义 `ResultProcessor` trait，提供统一的结果处理接口
- 实现 `BaseResultProcessor`，提供通用的基础功能
- 提供 `ResultProcessorFactory` 工厂类，方便创建各种执行器
- 支持流式处理和并行处理的扩展接口

### 3. `projection.rs`
**功能**: 列投影执行器，负责选择和计算查询结果的列。
- 实现 `ProjectExecutor`，执行列选择、表达式计算和列重命名
- 支持对数据集、顶点、边、路径等多种数据类型的投影
- 提供表达式求值功能，支持复杂的列转换

### 4. `sort.rs`
**功能**: 排序执行器，对查询结果进行排序。
- 实现 `SortExecutor`，支持多列排序和升序/降序
- 提供灵活的排序键定义，支持自定义排序规则
- 支持内存限制和磁盘溢出优化

### 5. `limit.rs`
**功能**: 结果数量限制执行器，控制返回结果的数量。
- 实现 `LimitExecutor`，支持 LIMIT 和 OFFSET 操作
- 提供分页查询的基础支持
- 支持多种数据类型的限制操作

### 6. `aggregation.rs`
**功能**: 聚合函数执行器，计算各种聚合函数的结果。
- 实现 `AggregateExecutor`，支持 COUNT、SUM、AVG、MAX、MIN 等聚合函数
- 提供 `GroupByExecutor` 和 `HavingExecutor`，支持完整的分组聚合操作
- 实现聚合函数的增量计算，优化性能

### 7. `dedup.rs`
**功能**: 去重执行器，移除结果集中的重复项。
- 实现 `DedupExecutor`，支持多种去重策略
- 提供基于键的去重、完全去重等不同策略
- 高效去重算法，保证结果的唯一性

### 8. `filter.rs`
**功能**: 过滤执行器，对结果应用条件过滤。
- 实现 `FilterExecutor`，支持复杂的条件表达式
- 适用于 HAVING 子句和结果后过滤
- 支持多种数据类型的过滤操作

### 9. `sample.rs`
**功能**: 采样执行器，从结果集中随机采样。
- 实现 `SampleExecutor`，支持多种采样方法（随机、蓄水池、系统采样）
- 使用蓄水池采样算法确保均匀分布
- 支持指定随机种子以保证结果的重现性

### 10. `topn.rs`
**功能**: TopN 执行器，优化的排序+限制操作。
- 实现 `TopNExecutor`，使用堆数据结构优化性能
- 支持 OFFSET 功能，提供更灵活的 TopN 查询
- 针对大数据集优化，避免全量排序

## 设计特点

1. **统一接口**: 所有执行器都实现 `ResultProcessor` trait，提供一致的使用方式
2. **链式处理**: 执行器支持链式连接，可以组合多个处理步骤
3. **异步执行**: 所有执行器都支持异步执行，提高并发性能
4. **内存效率**: 针对大数据集进行内存优化，支持磁盘溢出
5. **错误处理**: 完善的错误处理机制，提供详细的错误信息
6. **性能优化**: 使用堆数据结构、蓄水池采样等算法优化性能

## 使用场景

- **SELECT子句**: 投影执行器处理列选择
- **ORDER BY子句**: 排序执行器处理结果排序
- **LIMIT/OFFSET子句**: 限制执行器控制结果数量
- **GROUP BY子句**: 聚合执行器处理分组聚合
- **HAVING子句**: 过滤执行器处理分组后过滤
- **DISTINCT子句**: 去重执行器移除重复项
- **TOP N查询**: TopN执行器优化前N项查询
- **随机采样**: 采样执行器执行随机抽样

## 迁移说明

本模块已从 `data_processing` 模块迁移以下执行器：
- `filter` -> `result_processing/filter.rs`
- `dedup` -> `result_processing/dedup.rs`
- `sample` -> `result_processing/sample.rs`
- `aggregation` -> `result_processing/aggregation.rs`
- `sort` -> `result_processing/sort.rs`
- `pagination` -> `result_processing/limit.rs`

这些执行器现在都实现了统一的 `ResultProcessor` 接口，提供更好的性能和一致性。