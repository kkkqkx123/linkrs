# Fulltext Receiver 状态序列化说明

`FulltextReceiver` 的提交状态保存在 Tantivy 的 commit payload 中。Tantivy
的 payload 接口只接受 UTF-8 字符串，而 postcard 输出的是原始二进制数据，
因此这里继续使用 JSON，而不是直接写入 postcard 字节。

这是内部持久化格式统一规则下的明确例外：

- Vector receiver 的独立状态文件使用 postcard，并保存为
  `vector_receiver_state.bin`。
- Fulltext receiver 的状态跟随 Tantivy commit payload 保存，使用 JSON 以满足
  payload 接口约束。

如果未来 Tantivy 提供二进制 payload 接口，再评估迁移为 postcard；在此之前，
不应把 Fulltext receiver 的 JSON 使用视为遗漏的统一化任务。
