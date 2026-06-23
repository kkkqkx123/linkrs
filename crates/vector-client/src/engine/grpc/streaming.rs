use futures::Stream;
use std::pin::Pin;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tracing::{debug, error};

use crate::engine::grpc::convert::scored_point_from_proto;
use crate::engine::grpc::proto;
use crate::error::Result;
use crate::types::*;

type BoxStream<T> = Pin<Box<dyn Stream<Item = T> + Send>>;

pub struct StreamingEngine {
    client: proto::points_client::PointsClient<tonic::transport::Channel>,
}

impl StreamingEngine {
    pub fn new(channel: tonic::transport::Channel) -> Self {
        Self {
            client: proto::points_client::PointsClient::new(channel),
        }
    }

    pub async fn stream_search(
        &self,
        collection: &str,
        query: SearchQuery,
        batch_size: usize,
    ) -> Result<BoxStream<Vec<SearchResult>>> {
        debug!(
            "Streaming search in collection '{}' with batch_size={}",
            collection, batch_size
        );

        let (tx, rx) = mpsc::channel::<Vec<SearchResult>>(100);
        let mut client = self.client.clone();
        let collection = collection.to_string();

        let query_proto =
            crate::engine::grpc::convert::search_query_to_proto(collection.as_str(), &query);

        tokio::spawn(async move {
            let request = proto::SearchPoints {
                collection_name: collection,
                vector: query_proto.vector,
                filter: query_proto.filter,
                limit: query_proto.limit,
                with_payload: query_proto.with_payload,
                params: query_proto.params,
                score_threshold: query_proto.score_threshold,
                offset: query_proto.offset,
                vector_name: None,
                with_vectors: query_proto.with_vectors,
                read_consistency: None,
                timeout: None,
                shard_key_selector: None,
                sparse_indices: None,
            };

            match client.search(request).await {
                Ok(response) => {
                    let result = response.into_inner().result;
                    let results: Vec<SearchResult> =
                        result.into_iter().map(scored_point_from_proto).collect();

                    if tx.send(results).await.is_err() {
                        error!("Failed to send search results to channel");
                    }
                }
                Err(e) => {
                    error!("Streaming search failed: {}", e);
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    pub async fn stream_scroll(
        &self,
        collection: &str,
        batch_size: usize,
        with_payload: bool,
        with_vector: bool,
    ) -> Result<BoxStream<Vec<VectorPoint>>> {
        debug!(
            "Streaming scroll in collection '{}' with batch_size={}",
            collection, batch_size
        );

        let (tx, rx) = mpsc::channel::<Vec<VectorPoint>>(100);
        let mut client = self.client.clone();
        let collection = collection.to_string();

        tokio::spawn(async move {
            let mut offset: Option<proto::PointId> = None;
            let mut has_more = true;

            while has_more {
                let request = proto::ScrollPoints {
                    collection_name: collection.clone(),
                    filter: None,
                    offset: offset.clone(),
                    limit: Some(batch_size as u32),
                    with_payload: Some(proto::WithPayloadSelector {
                        selector_options: Some(
                            proto::with_payload_selector::SelectorOptions::Enable(with_payload),
                        ),
                    }),
                    with_vectors: Some(proto::WithVectorsSelector {
                        selector_options: Some(
                            proto::with_vectors_selector::SelectorOptions::Enable(with_vector),
                        ),
                    }),
                    read_consistency: None,
                    shard_key_selector: None,
                    order_by: None,
                    timeout: None,
                };

                match client.scroll(request).await {
                    Ok(response) => {
                        let scroll_result = response.into_inner();
                        let points: Vec<VectorPoint> = scroll_result
                            .result
                            .into_iter()
                            .map(crate::engine::grpc::convert::retrieved_point_from_proto)
                            .collect();

                        if points.is_empty() {
                            has_more = false;
                        } else {
                            if tx.send(points).await.is_err() {
                                error!("Failed to send scroll results to channel");
                                break;
                            }
                            offset = scroll_result.next_page_offset;
                            has_more = offset.is_some();
                        }
                    }
                    Err(e) => {
                        error!("Streaming scroll failed: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    pub async fn stream_upsert(
        &self,
        collection: &str,
        points: Vec<VectorPoint>,
        batch_size: usize,
    ) -> Result<BoxStream<UpsertResult>> {
        debug!(
            "Streaming upsert {} points to collection '{}' with batch_size={}",
            points.len(),
            collection,
            batch_size
        );

        let (tx, rx) = mpsc::channel::<UpsertResult>(100);
        let mut client = self.client.clone();
        let collection = collection.to_string();

        let proto_points: Vec<proto::PointStruct> = points
            .iter()
            .map(crate::engine::grpc::convert::point_struct_to_proto)
            .collect();

        tokio::spawn(async move {
            for chunk in proto_points.chunks(batch_size) {
                let request = proto::UpsertPoints {
                    collection_name: collection.clone(),
                    wait: Some(true),
                    ordering: None,
                    shard_key_selector: None,
                    points: chunk.to_vec(),
                };

                match client.upsert(request).await {
                    Ok(response) => {
                        let result = response.into_inner().result;
                        if let Some(op_result) = result {
                            let upsert_result =
                                crate::engine::grpc::convert::upsert_result_from_proto(op_result);
                            if tx.send(upsert_result).await.is_err() {
                                error!("Failed to send upsert result to channel");
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        error!("Streaming upsert failed: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }

    pub async fn stream_delete(
        &self,
        collection: &str,
        point_ids: Vec<String>,
        batch_size: usize,
    ) -> Result<BoxStream<DeleteResult>> {
        debug!(
            "Streaming delete {} points from collection '{}' with batch_size={}",
            point_ids.len(),
            collection,
            batch_size
        );

        let (tx, rx) = mpsc::channel::<DeleteResult>(100);
        let mut client = self.client.clone();
        let collection = collection.to_string();

        tokio::spawn(async move {
            for chunk in point_ids.chunks(batch_size) {
                let ids: Vec<proto::PointId> = chunk
                    .iter()
                    .map(|id| crate::engine::grpc::convert::point_id_to_proto(id))
                    .collect();

                let selector = proto::PointsSelector {
                    points_selector_one_of: Some(
                        proto::points_selector::PointsSelectorOneOf::Points(proto::PointsIdsList {
                            ids,
                        }),
                    ),
                };

                let request = proto::DeletePoints {
                    collection_name: collection.clone(),
                    wait: Some(true),
                    points: Some(selector),
                    ordering: None,
                    shard_key_selector: None,
                };

                match client.delete(request).await {
                    Ok(response) => {
                        let result = response.into_inner().result;
                        if let Some(del_result) = result {
                            let delete_result = DeleteResult {
                                operation_id: del_result.operation_id,
                                deleted_count: chunk.len() as u64,
                            };
                            if tx.send(delete_result).await.is_err() {
                                error!("Failed to send delete result to channel");
                                break;
                            }
                        }
                    }
                    Err(e) => {
                        error!("Streaming delete failed: {}", e);
                        break;
                    }
                }
            }
        });

        Ok(Box::pin(ReceiverStream::new(rx)))
    }
}

pub fn create_streaming_engine(channel: tonic::transport::Channel) -> StreamingEngine {
    StreamingEngine::new(channel)
}
