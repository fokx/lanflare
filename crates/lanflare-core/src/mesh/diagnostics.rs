//! Multi-hop mesh traceroute, RTT probe, and memory-to-memory bandwidth benchmark.
//!
//! Sub-phase 10.3: Multi-hop hop-by-hop latency measurement (`POST /api/lanflare/v1/mesh/trace`)
//! and memory-to-memory throughput streaming rate calculation (`POST /api/lanflare/v1/mesh/benchmark`)
//! across 1-hop LAN and multi-hop mesh overlays with simulated network matrix support.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use http_body_util::BodyExt;
use hyper::{Request, Response, StatusCode};
use hyper::body::Incoming;
use lanflare_proto::mesh::{
    MAX_MESH_TTL, MeshTracerouteRequest, MeshTracerouteResponse, SpeedtestChunkDto,
    SpeedtestResultDto, TracerouteHop,
};
use thiserror::Error;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::mesh::router::{MeshRouter, RouterError};
use crate::server::dufs::{empty_box_response, full_box_body, json_response, ServerResponseBody};

/// Error variants encountered during mesh diagnostics execution.
#[derive(Debug, Error)]
pub enum DiagnosticsError {
    #[error("Target unreachable: {0}")]
    Unreachable(String),

    #[error("No route to destination: {0}")]
    NoRoute(String),

    #[error("Time-To-Live (TTL) expired for trace {0}")]
    TtlExpired(String),

    #[error("Endpoint for next hop '{0}' is unresolved")]
    UnresolvedEndpoint(String),

    #[error("HTTP request error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("JSON serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Router error: {0}")]
    Router(#[from] RouterError),
}

/// In-memory state tracking an active throughput benchmark session on the receiver.
#[derive(Debug)]
struct ActiveBenchmark {
    start_time: Instant,
    total_bytes: usize,
    chunks_received: u32,
    total_chunks: u32,
}

/// Mesh diagnostics engine orchestrating multi-hop traceroute and throughput speedtests.
pub struct MeshDiagnostics {
    alias: RwLock<String>,
    benchmark_sessions: Arc<RwLock<HashMap<String, ActiveBenchmark>>>,
}

impl MeshDiagnostics {
    /// Creates a new `MeshDiagnostics` engine with the given device alias.
    pub fn new(alias: impl Into<String>) -> Self {
        Self {
            alias: RwLock::new(alias.into()),
            benchmark_sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Returns the current device alias.
    pub async fn alias(&self) -> String {
        self.alias.read().await.clone()
    }

    /// Sets the device alias used in traceroute hop reporting.
    pub async fn set_alias(&self, alias: impl Into<String>) {
        let mut a = self.alias.write().await;
        *a = alias.into();
    }

    /// Initiates a multi-hop traceroute to `target_fp`.
    pub async fn traceroute(
        &self,
        router: &MeshRouter,
        target_fp: &str,
    ) -> Result<MeshTracerouteResponse, DiagnosticsError> {
        let canonical_target = target_fp.to_ascii_uppercase();
        let local_fp = router.local_fingerprint();
        let now_ms = current_time_ms();

        // 1. Local loopback check
        if local_fp == canonical_target {
            let hop = TracerouteHop {
                node_fp: local_fp.to_string(),
                alias: self.alias().await,
                rtt_ms: 0,
                ingress_timestamp_ms: now_ms,
            };
            return Ok(MeshTracerouteResponse {
                trace_id: format!("trace-{}", Uuid::new_v4()),
                origin_fp: local_fp.to_string(),
                target_fp: canonical_target,
                success: true,
                total_hops: 1,
                total_rtt_ms: 0,
                hops: vec![hop],
                error: None,
            });
        }

        // 2. Resolve next-hop route
        let route = {
            let graph = router.graph().read().await;
            graph.resolve_route(&canonical_target)
        };

        let resolved_route = match route {
            Some(r) => r,
            None => return Err(DiagnosticsError::NoRoute(canonical_target)),
        };

        let trace_id = format!("trace-{}", Uuid::new_v4());
        let mut request = MeshTracerouteRequest::new(
            &trace_id,
            local_fp,
            &canonical_target,
            MAX_MESH_TTL,
            now_ms,
        );

        // Add local origin as first hop
        request.hops.push(TracerouteHop {
            node_fp: local_fp.to_string(),
            alias: self.alias().await,
            rtt_ms: 0,
            ingress_timestamp_ms: now_ms,
        });

        // 3. Apply network interceptor if active
        if let Some(interceptor) = router.interceptor().read().await.as_ref() {
            let delay = interceptor.intercept(local_fp, &resolved_route.next_hop_fp)?;
            if delay > Duration::ZERO {
                tokio::time::sleep(delay).await;
            }
        }

        // 4. Resolve next-hop HTTP endpoint
        let next_hop_url = router
            .resolve_endpoint(&resolved_route.next_hop_fp)
            .await
            .ok_or_else(|| DiagnosticsError::UnresolvedEndpoint(resolved_route.next_hop_fp.clone()))?;

        // 5. Transmit traceroute probe to next hop
        let url = format!("{}/api/lanflare/v1/mesh/trace", next_hop_url.trim_end_matches('/'));
        let response = router
            .client()
            .post(&url)
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(DiagnosticsError::Unreachable(format!(
                "HTTP status {} from next hop {}",
                response.status(),
                resolved_route.next_hop_fp
            )));
        }

        let trace_response: MeshTracerouteResponse = response.json().await?;
        Ok(trace_response)
    }

    /// Runs a memory-to-memory throughput speedtest streaming `num_chunks` chunks to `target_fp`.
    pub async fn run_speedtest(
        &self,
        router: &MeshRouter,
        target_fp: &str,
        num_chunks: u32,
        chunk_size: usize,
    ) -> Result<SpeedtestResultDto, DiagnosticsError> {
        let canonical_target = target_fp.to_ascii_uppercase();
        let local_fp = router.local_fingerprint();

        if num_chunks == 0 {
            return Err(DiagnosticsError::Unreachable("Chunk count must be > 0".into()));
        }

        // 1. Resolve route to destination
        let next_hop = if local_fp == canonical_target {
            local_fp.to_string()
        } else {
            let graph = router.graph().read().await;
            let route = graph
                .resolve_route(&canonical_target)
                .ok_or_else(|| DiagnosticsError::NoRoute(canonical_target.clone()))?;
            route.next_hop_fp
        };

        // 2. Resolve endpoint URL
        let base_url = router
            .resolve_endpoint(&next_hop)
            .await
            .ok_or_else(|| DiagnosticsError::UnresolvedEndpoint(next_hop.clone()))?;
        let url = format!("{}/api/lanflare/v1/mesh/benchmark", base_url.trim_end_matches('/'));

        // 3. Prepare payload buffer
        let test_id = format!("speed-{}", Uuid::new_v4());
        let payload = vec![0x5A; chunk_size];
        let total_bytes = chunk_size * (num_chunks as usize);

        let overall_start = Instant::now();
        let mut final_result: Option<SpeedtestResultDto> = None;

        for seq_no in 0..num_chunks {
            let is_last = seq_no + 1 == num_chunks;
            let chunk_dto = SpeedtestChunkDto {
                test_id: test_id.clone(),
                origin_fp: local_fp.to_string(),
                target_fp: canonical_target.clone(),
                seq_no,
                total_chunks: num_chunks,
                chunk_size_bytes: chunk_size,
                is_last,
                client_timestamp_ms: current_time_ms(),
                payload: payload.clone(),
            };

            // Interceptor check for each hop transmission
            if let Some(interceptor) = router.interceptor().read().await.as_ref() {
                let delay = interceptor.intercept(local_fp, &next_hop)?;
                if delay > Duration::ZERO {
                    tokio::time::sleep(delay).await;
                }
            }

            let resp = router
                .client()
                .post(&url)
                .json(&chunk_dto)
                .send()
                .await?;

            if !resp.status().is_success() {
                return Err(DiagnosticsError::Unreachable(format!(
                    "HTTP status {} during benchmark from next hop {}",
                    resp.status(),
                    next_hop
                )));
            }

            if is_last
                && let Ok(res) = resp.json::<SpeedtestResultDto>().await
            {
                final_result = Some(res);
            }
        }

        let client_duration = overall_start.elapsed();
        let client_duration_ms = (client_duration.as_millis() as u64).max(1);
        let duration_secs = client_duration.as_secs_f64().max(0.0001);
        let client_throughput_mbps = ((total_bytes as f64) * 8.0) / (duration_secs * 1_000_000.0);

        // Prefer remote measured report if available, while ensuring duration/throughput is bounded
        if let Some(mut remote_res) = final_result {
            if remote_res.duration_ms == 0 {
                remote_res.duration_ms = client_duration_ms;
                remote_res.throughput_mbps = client_throughput_mbps;
            }
            Ok(remote_res)
        } else {
            Ok(SpeedtestResultDto {
                test_id,
                origin_fp: local_fp.to_string(),
                target_fp: canonical_target,
                duration_ms: client_duration_ms,
                total_bytes,
                throughput_mbps: client_throughput_mbps,
                chunks_received: num_chunks,
            })
        }
    }

    /// Handles incoming `POST /api/lanflare/v1/mesh/trace` endpoint requests.
    pub async fn handle_trace_endpoint(
        &self,
        req: Request<Incoming>,
        router: &MeshRouter,
    ) -> Response<ServerResponseBody> {
        let body_bytes = match req.into_body().collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return empty_box_response(StatusCode::BAD_REQUEST),
        };

        let mut trace_req: MeshTracerouteRequest = match serde_json::from_slice(&body_bytes) {
            Ok(r) => r,
            Err(_) => return empty_box_response(StatusCode::UNPROCESSABLE_ENTITY),
        };

        let local_fp = router.local_fingerprint();
        let now_ms = current_time_ms();

        // Calculate hop RTT from previous hop ingress or origin start
        let prev_timestamp = trace_req
            .hops
            .last()
            .map(|h| h.ingress_timestamp_ms)
            .unwrap_or(trace_req.timestamp_ms);
        let hop_rtt = now_ms.saturating_sub(prev_timestamp) as u32;

        let hop = TracerouteHop {
            node_fp: local_fp.to_string(),
            alias: self.alias().await,
            rtt_ms: hop_rtt,
            ingress_timestamp_ms: now_ms,
        };
        trace_req.hops.push(hop);

        // Case 1: Reached destination target
        if local_fp == trace_req.target_fp {
            let total_rtt = now_ms.saturating_sub(trace_req.timestamp_ms) as u32;
            let response = MeshTracerouteResponse {
                trace_id: trace_req.trace_id,
                origin_fp: trace_req.origin_fp,
                target_fp: trace_req.target_fp,
                success: true,
                total_hops: trace_req.hops.len(),
                total_rtt_ms: total_rtt,
                hops: trace_req.hops,
                error: None,
            };
            return json_response(StatusCode::OK, &response);
        }

        // Case 2: TTL check
        if trace_req.ttl <= 1 || trace_req.hops.len() >= MAX_MESH_TTL as usize {
            let total_rtt = now_ms.saturating_sub(trace_req.timestamp_ms) as u32;
            let response = MeshTracerouteResponse {
                trace_id: trace_req.trace_id,
                origin_fp: trace_req.origin_fp,
                target_fp: trace_req.target_fp,
                success: false,
                total_hops: trace_req.hops.len(),
                total_rtt_ms: total_rtt,
                hops: trace_req.hops,
                error: Some("Time-To-Live (TTL) expired".into()),
            };
            return json_response(StatusCode::OK, &response);
        }

        trace_req.ttl -= 1;

        // Case 3: Resolve next hop in topology graph
        let next_hop = {
            let graph = router.graph().read().await;
            match graph.resolve_route(&trace_req.target_fp) {
                Some(r) => r.next_hop_fp,
                None => {
                    let total_rtt = now_ms.saturating_sub(trace_req.timestamp_ms) as u32;
                    let response = MeshTracerouteResponse {
                        trace_id: trace_req.trace_id,
                        origin_fp: trace_req.origin_fp,
                        target_fp: trace_req.target_fp,
                        success: false,
                        total_hops: trace_req.hops.len(),
                        total_rtt_ms: total_rtt,
                        hops: trace_req.hops,
                        error: Some("No route to destination".into()),
                    };
                    return json_response(StatusCode::OK, &response);
                }
            }
        };

        // Apply interceptor if present
        if let Some(interceptor) = router.interceptor().read().await.as_ref() {
            if let Ok(delay) = interceptor.intercept(local_fp, &next_hop) {
                if delay > Duration::ZERO {
                    tokio::time::sleep(delay).await;
                }
            } else {
                let total_rtt = now_ms.saturating_sub(trace_req.timestamp_ms) as u32;
                let response = MeshTracerouteResponse {
                    trace_id: trace_req.trace_id,
                    origin_fp: trace_req.origin_fp,
                    target_fp: trace_req.target_fp,
                    success: false,
                    total_hops: trace_req.hops.len(),
                    total_rtt_ms: total_rtt,
                    hops: trace_req.hops,
                    error: Some("Link partitioned or severed by interceptor".into()),
                };
                return json_response(StatusCode::OK, &response);
            }
        }

        // Forward to next hop
        let next_hop_url = match router.resolve_endpoint(&next_hop).await {
            Some(u) => u,
            None => {
                let response = MeshTracerouteResponse {
                    trace_id: trace_req.trace_id,
                    origin_fp: trace_req.origin_fp,
                    target_fp: trace_req.target_fp,
                    success: false,
                    total_hops: trace_req.hops.len(),
                    total_rtt_ms: now_ms.saturating_sub(trace_req.timestamp_ms) as u32,
                    hops: trace_req.hops,
                    error: Some(format!("Next hop endpoint '{next_hop}' unresolved")),
                };
                return json_response(StatusCode::OK, &response);
            }
        };

        let url = format!("{}/api/lanflare/v1/mesh/trace", next_hop_url.trim_end_matches('/'));
        match router.client().post(&url).json(&trace_req).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    match resp.json::<MeshTracerouteResponse>().await {
                        Ok(forwarded_resp) => json_response(StatusCode::OK, &forwarded_resp),
                        Err(_) => empty_box_response(StatusCode::BAD_GATEWAY),
                    }
                } else {
                    empty_box_response(StatusCode::BAD_GATEWAY)
                }
            }
            Err(_) => empty_box_response(StatusCode::BAD_GATEWAY),
        }
    }

    /// Handles incoming `POST /api/lanflare/v1/mesh/benchmark` endpoint requests.
    pub async fn handle_benchmark_endpoint(
        &self,
        req: Request<Incoming>,
        router: &MeshRouter,
    ) -> Response<ServerResponseBody> {
        let body_bytes = match req.into_body().collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(_) => return empty_box_response(StatusCode::BAD_REQUEST),
        };

        let chunk: SpeedtestChunkDto = match serde_json::from_slice(&body_bytes) {
            Ok(c) => c,
            Err(_) => return empty_box_response(StatusCode::UNPROCESSABLE_ENTITY),
        };

        let local_fp = router.local_fingerprint();

        // Case 1: Local node is the target destination
        if local_fp == chunk.target_fp {
            let mut sessions = self.benchmark_sessions.write().await;
            let session = sessions
                .entry(chunk.test_id.clone())
                .or_insert_with(|| ActiveBenchmark {
                    start_time: Instant::now(),
                    total_bytes: 0,
                    chunks_received: 0,
                    total_chunks: chunk.total_chunks,
                });

            session.total_bytes += chunk.chunk_size_bytes;
            session.chunks_received += 1;

            if chunk.is_last || session.chunks_received >= session.total_chunks {
                let duration = session.start_time.elapsed();
                let duration_ms = (duration.as_millis() as u64).max(1);
                let duration_secs = duration.as_secs_f64().max(0.0001);
                let throughput_mbps =
                    ((session.total_bytes as f64) * 8.0) / (duration_secs * 1_000_000.0);

                let result = SpeedtestResultDto {
                    test_id: chunk.test_id.clone(),
                    origin_fp: chunk.origin_fp,
                    target_fp: chunk.target_fp,
                    duration_ms,
                    total_bytes: session.total_bytes,
                    throughput_mbps,
                    chunks_received: session.chunks_received,
                };

                sessions.remove(&chunk.test_id);
                return json_response(StatusCode::OK, &result);
            } else {
                let ack = serde_json::json!({
                    "status": "chunk_received",
                    "seqNo": chunk.seq_no
                });
                return json_response(StatusCode::OK, &ack);
            }
        }

        // Case 2: Multi-hop relay forwarding
        let next_hop = {
            let graph = router.graph().read().await;
            match graph.resolve_route(&chunk.target_fp) {
                Some(r) => r.next_hop_fp,
                None => return empty_box_response(StatusCode::NOT_FOUND),
            }
        };

        // Interceptor check
        if let Some(interceptor) = router.interceptor().read().await.as_ref() {
            if let Ok(delay) = interceptor.intercept(local_fp, &next_hop) {
                if delay > Duration::ZERO {
                    tokio::time::sleep(delay).await;
                }
            } else {
                return empty_box_response(StatusCode::BAD_GATEWAY);
            }
        }

        let next_url = match router.resolve_endpoint(&next_hop).await {
            Some(u) => u,
            None => return empty_box_response(StatusCode::BAD_GATEWAY),
        };

        let url = format!("{}/api/lanflare/v1/mesh/benchmark", next_url.trim_end_matches('/'));
        match router.client().post(&url).json(&chunk).send().await {
            Ok(resp) => {
                let status = resp.status();
                if let Ok(bytes) = resp.bytes().await {
                    let hyper_status = StatusCode::from_u16(status.as_u16())
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                    Response::builder()
                        .status(hyper_status)
                        .header("Content-Type", "application/json")
                        .body(full_box_body(bytes))
                        .unwrap_or_else(|_| empty_box_response(StatusCode::INTERNAL_SERVER_ERROR))
                } else {
                    empty_box_response(StatusCode::BAD_GATEWAY)
                }
            }
            Err(_) => empty_box_response(StatusCode::BAD_GATEWAY),
        }
    }
}

fn current_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::sim::{SimulatedMeshCluster, VirtualNetworkMatrix};

    #[tokio::test]
    async fn test_multi_hop_traceroute_and_bandwidth_benchmark() {
        // Setup 3-hop simulated mesh across 3 isolated subnets:
        // Node A on SubnetA
        // Relay B bridging SubnetA and SubnetB
        // Relay C bridging SubnetB and SubnetC
        // Node D on SubnetC
        let matrix = Arc::new(VirtualNetworkMatrix::new());
        let mut cluster = SimulatedMeshCluster::new(matrix.clone());

        let fp_a = cluster
            .spawn_node("Node-A", vec!["SubnetA".into()], false)
            .await
            .expect("spawn Node-A");

        let fp_b = cluster
            .spawn_node("Relay-B", vec!["SubnetA".into(), "SubnetB".into()], true)
            .await
            .expect("spawn Relay-B");

        let fp_c = cluster
            .spawn_node("Relay-C", vec!["SubnetB".into(), "SubnetC".into()], true)
            .await
            .expect("spawn Relay-C");

        let fp_d = cluster
            .spawn_node("Node-D", vec!["SubnetC".into()], false)
            .await
            .expect("spawn Node-D");

        // Verify isolation: Node A cannot reach Relay C or Node D directly
        assert!(matrix.can_reach(&fp_a, &fp_b));
        assert!(!matrix.can_reach(&fp_a, &fp_c));
        assert!(!matrix.can_reach(&fp_a, &fp_d));
        assert!(matrix.can_reach(&fp_b, &fp_c));
        assert!(matrix.can_reach(&fp_c, &fp_d));

        // Establish direct links and exchange 3 gossip rounds for full convergence
        cluster.establish_direct_links().await;
        let _ = cluster.exchange_gossip_rounds(3).await;

        let node_a = cluster.get_node(&fp_a).expect("Node A");
        let router_a = node_a.mesh_router();

        // 1. Execute Multi-Hop Traceroute from Node A -> Node D (3 hops away)
        let trace_result = router_a
            .traceroute(&fp_d)
            .await
            .expect("traceroute from A to D");

        assert!(trace_result.success, "Traceroute should succeed: {:?}", trace_result.error);
        assert_eq!(trace_result.origin_fp, fp_a);
        assert_eq!(trace_result.target_fp, fp_d);
        // Hops traversed should be: Node-A -> Relay-B -> Relay-C -> Node-D (4 hops in sequence)
        assert_eq!(trace_result.hops.len(), 4, "Expected 4 hops in trace path: Node-A, Relay-B, Relay-C, Node-D");
        assert_eq!(trace_result.hops[0].node_fp, fp_a);
        assert_eq!(trace_result.hops[1].node_fp, fp_b);
        assert_eq!(trace_result.hops[2].node_fp, fp_c);
        assert_eq!(trace_result.hops[3].node_fp, fp_d);

        assert_eq!(trace_result.hops[0].alias, "Node-A");
        assert_eq!(trace_result.hops[1].alias, "Relay-B");
        assert_eq!(trace_result.hops[2].alias, "Relay-C");
        assert_eq!(trace_result.hops[3].alias, "Node-D");

        // 2. Execute Memory-to-Memory Speedtest / Bandwidth Benchmark from Node A -> Node D
        // Transmit 5 chunks of 64 KB (320 KB total)
        let chunk_size = 64 * 1024;
        let num_chunks = 5;

        let speedtest_result = router_a
            .run_speedtest(&fp_d, num_chunks, chunk_size)
            .await
            .expect("run speedtest from A to D");

        assert_eq!(speedtest_result.origin_fp, fp_a);
        assert_eq!(speedtest_result.target_fp, fp_d);
        assert_eq!(speedtest_result.total_bytes, chunk_size * (num_chunks as usize));
        assert_eq!(speedtest_result.chunks_received, num_chunks);
        assert!(speedtest_result.duration_ms > 0, "Duration should be non-zero");
        assert!(speedtest_result.throughput_mbps > 0.0, "Throughput should be positive");

        println!(
            "3-hop mesh speedtest: {} bytes in {} ms => {:.2} Mbps",
            speedtest_result.total_bytes, speedtest_result.duration_ms, speedtest_result.throughput_mbps
        );

        // 3. Test loopback traceroute
        let loopback_trace = router_a.traceroute(&fp_a).await.expect("loopback trace");
        assert!(loopback_trace.success);
        assert_eq!(loopback_trace.hops.len(), 1);
        assert_eq!(loopback_trace.hops[0].node_fp, fp_a);
    }
}
