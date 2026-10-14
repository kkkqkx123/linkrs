# P2 [query] 向量索引的适用范围与文档冲突，且 compression 存在静默无效分支

## 现象

### 1. 文档说支持边类型，代码明确拒绝

文档 §7 语法写 `ON <tag_or_edge_name>`，功能描述写「在标签或边类型的向量属性上创建向量索引」。

代码两处显式拒绝：

- `crates/linkrs-query/src/planning/vector_planner.rs:86`、`186`
  `"Vector indexes are vertex-only: '{}' is an edge type, create the index on a vertex tag instead"`
- `crates/linkrs-query/src/executor/streaming/operators/vector_operator.rs:288-308`（无 metadata 路径的复核）

注释说明了理由：点 ID、payload、rebuild 源都是按顶点建模的。这看起来是有意设计，**文档写错了**。

### 2. `compression` 对非 PRODUCT 量化静默无效

`vector_operator.rs:357-386`：`compression` 只在 `QuantizationKind::Product` 分支被读取。

- `quantization=SCALAR, compression=x16` → `x16` 被静默忽略（标量量化没有压缩比概念）。
- `quantization=BINARY, compression=x8` → 同样被忽略。

文档 §7 只说 compression「需配合 quantization」，未限定 quantization 的种类，读者会以为任意组合都生效。

### 3. HNSW 参数校验结果被丢弃

`vector_operator.rs:328-338`：
```rust
if hnsw_m.is_some() || hnsw_ef_construct.is_some() {
    let mut hnsw = simvec::HnswConfig::default();
    if let Some(m) = hnsw_m { hnsw.m = *m; }
    if let Some(ef) = hnsw_ef_construct { hnsw.ef_construct = *ef; }
    let _ = hnsw.validate();        // 返回值丢弃
    config = config.with_hnsw(hnsw);
}
```
非法 HNSW 参数（如 `hnsw_m=1` 或 `hnsw_ef_construct=0`）不会报错，构造出一个无效配置。

### 4. `index_name` 与实际索引键脱钩

`vector_operator.rs:392-403` 创建时键是 `(space_id, tag_name, field_name)`，`index_name` 未使用。
`DROP VECTOR INDEX <name>`（`spec/vector.rs:34-44`）却按预解析的 `(space_id, tag_name, field_name)` 找。
即用户声明的索引名不参与创建，只在 planning 期用于反查位置。

## 影响

1. §7 的适用范围描述错误，用户对边类型建向量索引会得到"vertex-only"错误。
2. `compression` 配错量化类型时静默无效。
3. 非法 HNSW 参数无报错，可能在下层触发意外的性能或内存行为。

## 根因

1. 文档编写时按 DDL 语法对称性推断（TAG/EDGE 索引都类似），未核对向量索引的实际数据模型约束。
2. executor 把 `compression` 只映射在乘积量化分支，未在别处报"此组合不支持"。
3. `validate()` 的 `Result` 被 `let _ =` 消费，属于"有校验但不使用校验结论"。

## 建议

1. **文档改为真相**：§7 适用范围改为「仅顶点标签（TAG）」；`ON <tag_or_edge_name>` 改为 `ON <tag_name>`；
   若未来要支持边，需先解决 payload 与 ID 建模，不应只在文档上放行。
2. `compression` 在 `quantization` 为 SCALAR/BINARY 时**显式报错**（"compression requires quantization=PRODUCT"），不要静默忽略。
3. `hnsw.validate()` 的返回值转为错误传播；同样处理 `quant_cfg.validate()`（`:384` 处也是 `let _ =`）。
4. 明确索引命名语义：要么让 `index_name` 真正参与创建，要么文档写明"向量索引无独立名称，按 (tag, field) 定位"。

## 严重级

**P2（文档误导 + 静默无效配置）**

## 代码位置

- `crates/linkrs-query/src/planning/vector_planner.rs:76,86,179,186`
- `crates/linkrs-query/src/executor/streaming/operators/vector_operator.rs:271-403`
- `docs/release/03_ddl_schema_definition.md` §7
