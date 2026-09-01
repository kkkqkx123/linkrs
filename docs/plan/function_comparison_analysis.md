# Ladybug vs LinkRS 函数支持对比分析

## 分析概述

本文档对比了ref/ladybug项目（C++图数据库）与当前LinkRS项目（Rust图数据库）在查询语言函数支持方面的差异，分析需要补充的功能。

## Ladybug项目函数分类

### 1. 算术函数 (Arithmetic Functions)
- 基础运算：`+`, `-`, `*`, `/`, `%`, `^`
- 绝对值：`abs`
- 三角函数：`acos`, `asin`, `atan`, `atan2`, `cos`, `sin`, `tan`, `cosh`, `sinh`, `tanh`
- 对数函数：`ln`, `log`, `log10`, `log2`
- 幂函数：`sqrt`, `cbrt`, `pow`
- 舍入函数：`ceil`, `floor`, `round`, `sign`, `even`
- 数学常数：`pi`, `e`
- 角度转换：`degrees`, `radians`
- 位运算：`bitwise_xor`, `bitwise_and`, `bitwise_or`, `bit_shift_left`, `bit_shift_right`
- 特殊函数：`factorial`, `gamma`, `lgamma`, `negate`, `rand`, `set_seed`

### 2. 字符串函数 (String Functions)
- 大小写：`upper`, `lower`, `initcap`
- 长度和截取：`length`, `left`, `right`, `substring`, `substr`
- 搜索：`contains`, `starts_with`, `ends_with`, `position`, `indexOf`
- 替换：`replace`, `translate`
- 填充：`lpad`, `rpad`
- 修剪：`trim`, `ltrim`, `rtrim`
- 分割：`split`, `string_split`, `split_part`, `regexp_split_to_array`
- 连接：`concat`, `concat_ws`
- 重复：`repeat`
- 反转：`reverse`
- 格式化：`format`
- 相似度：`levenshtein`
- 正则表达式：`regexp_full_match`, `regexp_matches`, `regexp_replace`, `regexp_extract`, `regexp_extract_all`

### 3. 数组函数 (Array Functions)
- 创建：`array_value`, `array_cross_product`
- 相似度：`array_cosine_similarity`, `array_distance`, `array_squared_distance`, `array_inner_product`, `array_dot_product`

### 4. 列表函数 (List Functions)
- 创建：`list_creation`, `list_range`
- 访问：`list_extract`, `list_element`
- 操作：`list_concat`, `list_append`, `list_prepend`, `list_position`, `list_indexof`
- 检查：`list_contains`, `list_has`
- 切片：`list_slice`
- 排序：`list_sort`, `list_reverse_sort`
- 聚合：`list_sum`, `list_product`
- 去重：`list_distinct`, `list_unique`
- 其他：`list_any_value`, `list_reverse`, `size`, `list_to_string`
- 高阶函数：`list_transform`, `list_filter`, `list_reduce`, `list_any`, `list_all`, `list_none`, `list_single`, `list_has_all`

### 5. 类型转换函数 (Cast Functions)
- 日期转换：`cast_to_date`, `cast_to_timestamp`, `cast_to_interval`
- 字符串转换：`cast_to_string`
- 二进制转换：`cast_to_blob`
- UUID转换：`cast_to_uuid`
- 数值转换：`cast_to_double`, `cast_to_float`, `cast_to_int64`, `cast_to_int32`, `cast_to_int16`, `cast_to_int8`
- 无符号整数：`cast_to_uint64`, `cast_to_uint32`, `cast_to_uint16`, `cast_to_uint8`
- 布尔转换：`cast_to_bool`

### 6. 比较函数 (Comparison Functions)
- `equals`, `not_equals`, `greater_than`, `greater_than_equals`, `less_than`, `less_than_equals`

### 7. 日期函数 (Date Functions)
- 提取：`date_part`, `day_name`, `month_name`
- 截断：`date_trunc`
- 特殊日期：`greatest`, `least`, `last_day`
- 创建：`make_date`, `current_date`

### 8. 时间戳函数 (Timestamp Functions)
- `century`, `epoch_ms`, `to_timestamp`, `current_timestamp`, `to_epoch_ms`

### 9. 时间间隔函数 (Interval Functions)
- `to_years`, `to_months`, `to_days`, `to_hours`, `to_minutes`, `to_seconds`, `to_milliseconds`, `to_microseconds`

### 10. 二进制函数 (Blob Functions)
- `octet_length`, `encode`, `decode`

### 11. UUID函数 (UUID Functions)
- `gen_random_uuid`

### 12. 结构体函数 (Struct Functions)
- `struct_pack`, `struct_extract`, `keys`

### 13. 映射函数 (Map Functions)
- `map_creation`, `map_extract`, `element_at`, `cardinality`, `map_keys`, `map_values`

### 14. 联合类型函数 (Union Functions)
- `union_value`, `union_tag`, `union_extract`

### 15. 节点/关系函数 (Node/Rel Functions)
- `offset`, `row_id`, `id`, `start_node`, `end_node`, `label`, `labels`, `cost`

### 16. 路径函数 (Path Functions)
- `nodes`, `rels`, `relationships`, `properties`, `is_trail`, `is_acyclic`, `length`

### 17. 哈希函数 (Hash Functions)
- `md5`, `sha256`, `hash`

### 18. 标量工具函数 (Scalar Utility Functions)
- `coalesce`, `if_null`, `constant_or_null`, `count_if`, `error`, `null_if`, `typeof`

### 19. 序列函数 (Sequence Functions)
- `curr_val`, `next_val`

### 20. 聚合函数 (Aggregate Functions)
- `count`, `count_star`, `sum`, `avg`, `min`, `max`, `collect`

### 21. 表函数 (Table Functions)
- 系统信息函数：`current_setting`, `catalog_version`, `db_version`, `show_tables`, `show_graphs`, `free_space_info`, `show_warnings`, `table_info`, `show_connection`, `stats_info`, `storage_info`, `show_attached_databases`, `show_sequences`, `show_functions`, `bm_info`, `file_info`, `disk_size_info`, `show_loaded_extensions`, `show_official_extensions`, `show_indexes`, `show_projected_graphs`, `projected_graph_info`, `show_macros`

### 22. 扫描函数 (Scan Functions)
- `parquet_scan`, `npy_scan`, `serial_csv_scan`, `parallel_csv_scan`

### 23. 导出函数 (Export Functions)
- `export_csv`, `export_parquet`

## 当前项目函数支持情况

### 1. 数学函数 (MathFunction)
**已实现**：`abs`, `sqrt`, `pow`, `log`, `log10`, `sin`, `cos`, `tan`, `round`, `ceil`, `floor`, `asin`, `acos`, `atan`, `cbrt`, `hypot`, `sign`, `rand`, `rand32`, `rand64`, `e`, `pi`, `exp2`, `log2`, `radians`, `bit_and`, `bit_or`, `bit_xor`, `atan2`, `sinh`, `cosh`, `tanh`, `degrees`, `gcd`, `lcm`

**缺失**：`factorial`, `gamma`, `lgamma`, `negate`, `even`, `set_seed`, `bit_shift_left`, `bit_shift_right`

### 2. 字符串函数 (StringFunction)
**已实现**：`length`, `upper`, `lower`, `trim`, `substring`, `concat`, `replace`, `contains`, `starts_with`, `ends_with`, `split`, `lpad`, `rpad`, `concat_ws`, `strcasecmp`, `levenshtein`, `split_part`, `initcap`, `repeat`, `position`, `left`, `right`, `insert`, `translate`, `format`, `string_split`

**缺失**：`reverse`, `regexp_full_match`, `regexp_matches`, `regexp_replace`, `regexp_extract`, `regexp_extract_all`（正则表达式函数在RegexFunction中单独实现）

### 3. 正则表达式函数 (RegexFunction)
**已实现**：`regex_match`, `regex_replace`, `regex_find`

**缺失**：`regexp_full_match`, `regexp_extract`, `regexp_extract_all`, `regexp_split_to_array`

### 4. 类型转换函数 (ConversionFunction)
**已实现**：`to_string`, `to_int`, `to_float`, `to_bool`

**缺失**：`to_date`, `to_timestamp`, `to_interval`, `to_blob`, `to_uuid`, `to_int8/16/32/64`, `to_uint8/16/32/64`

### 5. 日期时间函数 (DateTimeFunction)
**已实现**：`now`, `date`, `time`, `datetime`, `year`, `month`, `day`, `hour`, `minute`, `second`, `timestamp`, `date_add`, `date_sub`, `date_diff`, `date_trunc`, `current_date`, `current_timestamp`, `to_char`, `to_date`, `age`, `last_day`, `generate_series`

**缺失**：`date_part`, `day_name`, `month_name`, `greatest`, `least`, `make_date`, `century`, `epoch_ms`, `to_timestamp`, `to_epoch_ms`, `to_years/months/days/hours/minutes/seconds/milliseconds/microseconds`

### 6. 地理空间函数 (GeographyFunction)
**已实现**：`st_point`, `st_geog_from_text`, `st_as_text`, `st_centroid`, `st_is_valid`, `st_intersects`, `st_covers`, `st_covered_by`, `st_dwithin`, `st_distance`

**缺失**：无重大缺失

### 7. 实用函数 (UtilityFunction)
**已实现**：`coalesce`, `hash`, `json_extract`, `json_build_object`, `json_build_array`, `json_object_keys`, `nullif`, `greatest`, `least`, `gen_random_uuid`, `json_each`, `json_typeof`, `json_strip_nulls`, `ifnull`, `typeof`, `version`, `current_user`, `current_database`, `corr`, `covar_pop`, `covar_samp`

**缺失**：`constant_or_null`, `count_if`, `error`

### 8. 图函数 (GraphFunction)
**已实现**：`id`, `tags`, `labels`, `properties`, `edge_type`, `src`, `dst`, `rank`, `start_node`, `end_node`, `neighbors`, `degree`, `out_edges`, `in_edges`, `shortest_path`, `bfs`, `connected_components`, `variable_length_path`, `page_rank`

**缺失**：`offset`, `row_id`, `label`, `labels`, `cost`

### 9. 容器函数 (ContainerFunction)
**已实现**：`head`, `last`, `tail`, `size`, `range`, `keys`, `reverse_list`, `to_set`, `list_contains`, `list_append`, `list_prepend`, `list_filter`, `list_transform`, `list_concat`, `list_sort`, `list_slice`, `list_to_string`, `list_distinct`, `list_unique`, `list_extract`

**缺失**：`list_element`, `list_position`, `list_indexof`, `list_has`, `list_sum`, `list_product`, `list_any_value`, `list_reverse`, `list_any`, `list_all`, `list_none`, `list_single`, `list_has_all`

### 10. 路径函数 (PathFunction)
**已实现**：`nodes`, `relationships`

**缺失**：`properties`, `is_trail`, `is_acyclic`, `length`

### 11. 聚合函数 (AggregateFunction)
**已实现**：`count`, `sum`, `avg`, `min`, `max`, `collect`, `collect_set`, `variance`, `median`, `mode`, `bool_and`, `bool_or`, `stddev_pop`, `stddev_samp`, `product`, `percentile_cont`, `group_concat_with_order`

**缺失**：`count_star`, `percentile`, `bit_and`, `bit_or`, `vec_sum`, `vec_avg`

### 12. 窗口函数 (WindowFunction)
**已实现**：`row_number`, `rank`, `dense_rank`, `lead`, `lag`, `first_value`, `last_value`, `nth_value`, `ntile`

**缺失**：无重大缺失

### 13. 全文搜索函数 (FulltextFunction)
**已实现**：全文搜索相关函数

**缺失**：无重大缺失

### 14. 向量函数 (VectorFunction)
**已实现**：向量相关函数

**缺失**：无重大缺失

## 功能差异总结

### 高优先级缺失功能

1. **时间间隔函数**：`to_years`, `to_months`, `to_days`, `to_hours`, `to_minutes`, `to_seconds`, `to_milliseconds`, `to_microseconds`
2. **时间戳函数**：`century`, `epoch_ms`, `to_timestamp`, `to_epoch_ms`
3. **日期提取函数**：`date_part`, `day_name`, `month_name`
4. **数组函数**：`array_value`, `array_cross_product`, `array_cosine_similarity`, `array_distance`, `array_squared_distance`, `array_inner_product`, `array_dot_product`
5. **结构体函数**：`struct_pack`, `struct_extract`
6. **映射函数**：`map_creation`, `map_extract`, `element_at`, `cardinality`, `map_keys`, `map_values`
7. **联合类型函数**：`union_value`, `union_tag`, `union_extract`
8. **二进制函数**：`octet_length`, `encode`, `decode`

### 中优先级缺失功能

1. **数学函数**：`factorial`, `gamma`, `lgamma`, `negate`, `even`, `set_seed`
2. **位运算函数**：`bit_shift_left`, `bit_shift_right`
3. **字符串函数**：`reverse`
4. **类型转换函数**：更完整的类型转换支持
5. **序列函数**：`curr_val`, `next_val`
6. **路径函数**：`properties`, `is_trail`, `is_acyclic`, `length`

### 低优先级缺失功能

1. **系统表函数**：各种系统信息查询函数
2. **扫描函数**：文件格式扫描函数
3. **导出函数**：数据导出函数

## 建议的实现计划

### 第一阶段：核心函数补全
1. 实现时间间隔函数（高优先级）
2. 实现时间戳函数（高优先级）
3. 实现日期提取函数（高优先级）
4. 实现数组相似度函数（高优先级）

### 第二阶段：数据结构函数
1. 实现结构体函数
2. 实现映射函数
3. 实现联合类型函数
4. 实现二进制函数

### 第三阶段：扩展功能
1. 实现缺失的数学函数
2. 实现位运算函数
3. 实现序列函数
4. 完善路径函数

### 第四阶段：系统功能
1. 实现系统表函数
2. 实现文件扫描函数
3. 实现数据导出函数

## 结论

当前项目已经实现了大部分核心查询函数，但在时间间隔、时间戳、数组相似度等方面存在功能缺口。建议按照优先级逐步补全这些功能，以提高查询语言的表达能力和兼容性。