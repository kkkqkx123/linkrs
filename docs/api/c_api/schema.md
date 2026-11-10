# C API 模式

源码：`crates/linkrs-api/src/embedded/c_api/schema.rs`。
属性定义为 JSON 数组穿越；`get_*` 不存在时返回空而非报错；
无属性 tag 按引擎主键规则被拒绝并透出存储错误。

## 空间

| 函数 | 说明 |
|---|---|
| `linkrs_create_space(db, name, partitions, replicas, vid_type, comment)` | 创建空间 |
| `linkrs_drop_space(db, name)` | 删除空间 |
| `linkrs_list_spaces(db)` | 空间名列表堆 JSON |
| `linkrs_get_space(db, name)` | 空间详情堆 JSON，不存在返回空 |

## 标签

| 函数 | 说明 |
|---|---|
| `linkrs_create_tag(db, space, name, properties_json)` | 创建标签，属性为 JSON 数组 |
| `linkrs_drop_tag(db, space, name)` | 删除标签 |
| `linkrs_list_tags(db, space)` | 标签列表堆 JSON |
| `linkrs_get_tag(db, space, name)` | 标签详情堆 JSON，不存在返回空 |

## 边类型

| 函数 | 说明 |
|---|---|
| `linkrs_create_edge_type(db, space, name, properties_json)` | 创建边类型 |
| `linkrs_drop_edge_type(db, space, name)` | 删除边类型 |
| `linkrs_list_edge_types(db, space)` | 边类型列表堆 JSON |
| `linkrs_get_edge_type(db, space, name)` | 边类型详情堆 JSON，不存在返回空 |

## 版本历史读

| 函数 | 说明 |
|---|---|
| `linkrs_label_version_history(db, space, label, is_edge)` | 标签版本历史堆 JSON，未知标签返回空 |
| `linkrs_applied_versions(db, space)` | 已应用版本堆 JSON |

堆 JSON 出参统一以 `linkrs_free_string` 释放。
