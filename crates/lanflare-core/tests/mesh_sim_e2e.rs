//! Multi-Subnet Overlay Mesh End-to-End Simulation & Verification Suite.
//!
//! Sub-phase 7.4 integration tests:
//! - `test_two_hop_cross_subnet_discovery_and_transfer`:
//!   Node A on Subnet A and Node C on Subnet B discover each other through Relay B
//!   and complete E2EE envelope forwarding and cut-through file streaming.
//! - `test_three_hop_chain_e2ee_file_transfer`:
//!   4-node chain (`A -> B -> C -> D`) across 3 subnets performs multi-hop cut-through
//!   file streaming with bounded memory and bit-for-bit SHA-256 verification.
//! - `test_dual_relay_failover_under_simulated_link_cut`:
//!   Dual-relay diamond topology dynamically severs the primary low-latency link,
//!   triggering automatic Dijkstra route failover to the backup relay and successful delivery.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use lanflare_core::crypto::e2ee::{open_envelope, seal_envelope};
use lanflare_core::mesh::sim::{SimulatedLinkConfig, SimulatedMeshCluster, VirtualNetworkMatrix};
use lanflare_proto::mesh::RoutedEnvelope;
use sha2::{Digest, Sha256};
use std::sync::Mutex;

#[tokio::test]
async fn test_two_hop_cross_subnet_discovery_and_transfer() {
    // 1. Setup VirtualNetworkMatrix with 3 nodes across 2 subnets
    let matrix = Arc::new(VirtualNetworkMatrix::new());
    let mut cluster = SimulatedMeshCluster::new(matrix.clone());

    // Node A on SubnetA only
    let fp_a = cluster
        .spawn_node("Node-A", vec!["SubnetA".into()], false)
        .await
        .expect("spawn Node-A");

    // Relay B bridging SubnetA and SubnetB
    let fp_b = cluster
        .spawn_node("Relay-B", vec!["SubnetA".into(), "SubnetB".into()], true)
        .await
        .expect("spawn Relay-B");

    // Node C on SubnetB only
    let fp_c = cluster
        .spawn_node("Node-C", vec!["SubnetB".into()], false)
        .await
        .expect("spawn Node-C");

    // 2. Verify virtual subnet partitions at L2/L3 boundary
    assert!(matrix.can_reach(&fp_a, &fp_b), "A and B share SubnetA");
    assert!(matrix.can_reach(&fp_b, &fp_c), "B and C share SubnetB");
    assert!(
        !matrix.can_reach(&fp_a, &fp_c),
        "A and C share no subnets (partitioned)"
    );

    // 3. Connect 1-hop physical neighbors and execute 2 rounds of gossip
    cluster.establish_direct_links().await;
    cluster
        .exchange_gossip_rounds(2)
        .await
        .expect("gossip exchange");

    let node_a = cluster.get_node(&fp_a).unwrap();
    let node_c = cluster.get_node(&fp_c).unwrap();

    // 4. Verify PeerRegistry integration on Node A
    let peer_c = node_a
        .registry()
        .get(&fp_c)
        .await
        .expect("Node C must be present in Node A's peer registry");

    assert!(
        peer_c.is_mesh_peer(),
        "Node C must be identified as a mesh peer"
    );
    assert_eq!(
        peer_c.mesh_hop_count(),
        Some(2),
        "Node C hop count must be 2"
    );
    assert_eq!(
        peer_c.mesh_route_path(),
        Some(vec![fp_a.clone(), fp_b.clone(), fp_c.clone()]),
        "Route path must traverse A -> B -> C"
    );
    assert_eq!(peer_c.alias, "Node-C");

    // Verify PeerRegistry integration on Node C
    let peer_a = node_c
        .registry()
        .get(&fp_a)
        .await
        .expect("Node A must be present in Node C's peer registry");
    assert!(peer_a.is_mesh_peer());
    assert_eq!(peer_a.mesh_hop_count(), Some(2));

    // 5. Verify E2EE routed envelope delivery from Node A to Node C through Relay B
    let (tx_env, mut rx_env) = tokio::sync::mpsc::channel(4);
    node_c
        .mesh_router()
        .set_packet_sink(move |env| {
            let _ = tx_env.try_send(env);
        })
        .await;

    let test_secret_message =
        b"CONFIDENTIAL: End-to-end encrypted mesh payload across Subnet A and Subnet B!";
    let sealed_envelope = seal_envelope(
        node_a.tls_identity(),
        &fp_c,
        &node_c.tls_identity().cert_pem,
        test_secret_message,
    )
    .expect("sealing E2EE envelope must succeed");

    let packet_id = format!("pkt-{}", uuid::Uuid::new_v4());
    let routed = RoutedEnvelope {
        packet_id: packet_id.clone(),
        src_fp: fp_a.clone(),
        dst_fp: fp_c.clone(),
        ttl: 4,
        route_path: vec![fp_a.clone()],
        payload: sealed_envelope,
    };

    let fwd_resp = node_a
        .mesh_router()
        .forward_envelope(routed)
        .await
        .expect("forward envelope must succeed");

    assert_eq!(fwd_resp.status, "forwarded");
    assert_eq!(fwd_resp.next_hop, Some(fp_b.clone()));

    let delivered_env = tokio::time::timeout(Duration::from_secs(3), rx_env.recv())
        .await
        .expect("timed out waiting for envelope delivery at Node C")
        .expect("envelope must be received");

    assert_eq!(delivered_env.packet_id, packet_id);
    assert_eq!(delivered_env.src_fp, fp_a);
    assert_eq!(delivered_env.dst_fp, fp_c);

    // Decrypt payload at destination Node C
    let decrypted_bytes = open_envelope(
        node_c.tls_identity(),
        Some(&node_a.tls_identity().cert_pem),
        &delivered_env.payload,
    )
    .expect("destination Node C must successfully open E2EE envelope");
    assert_eq!(decrypted_bytes, test_secret_message);

    // 6. Verify Cut-Through Streaming from Node A to Node C through Relay B
    let delivered_chunks = Arc::new(Mutex::new(Vec::new()));
    let sink_chunks = delivered_chunks.clone();
    node_c
        .mesh_router()
        .set_stream_sink(move |_sid, chunk| {
            sink_chunks.lock().unwrap().push(chunk);
        })
        .await;

    let stream_id = format!("stream-{}", uuid::Uuid::new_v4());
    let chunks = vec![
        Bytes::from(vec![0xAA; 64 * 1024]),
        Bytes::from(vec![0xBB; 64 * 1024]),
        Bytes::from(vec![0xCC; 64 * 1024]),
    ];
    let total_stream_bytes: u64 = chunks.iter().map(|c| c.len() as u64).sum();

    let mut stream_hasher = Sha256::new();
    for c in &chunks {
        stream_hasher.update(c);
    }
    let expected_sha256 = format!("{:064X}", stream_hasher.finalize());

    let stats = node_a
        .mesh_router()
        .send_stream(&fp_c, "file-test-2hop", &stream_id, chunks)
        .await
        .expect("send_stream through Relay B must succeed");

    assert_eq!(stats.status, "delivered");
    assert_eq!(stats.bytes_received, total_stream_bytes);
    assert_eq!(stats.sha256, expected_sha256);

    {
        let recorded = delivered_chunks.lock().unwrap();
        assert!(!recorded.is_empty(), "must receive chunks");
        assert_eq!(
            recorded.iter().map(|c| c.len()).sum::<usize>(),
            192 * 1024
        );
    }

    cluster.shutdown();
}

#[tokio::test]
async fn test_three_hop_chain_e2ee_file_transfer() {
    // 1. Setup 4-node chain: Node A [Sub1] <-> Relay B [Sub1, Sub2] <-> Relay C [Sub2, Sub3] <-> Node D [Sub3]
    let matrix = Arc::new(VirtualNetworkMatrix::new());
    let mut cluster = SimulatedMeshCluster::new(matrix.clone());

    let fp_a = cluster
        .spawn_node("Chain-A", vec!["Subnet1".into()], false)
        .await
        .expect("spawn Chain-A");

    let fp_b = cluster
        .spawn_node(
            "Relay-B",
            vec!["Subnet1".into(), "Subnet2".into()],
            true,
        )
        .await
        .expect("spawn Relay-B");

    let fp_c = cluster
        .spawn_node(
            "Relay-C",
            vec!["Subnet2".into(), "Subnet3".into()],
            true,
        )
        .await
        .expect("spawn Relay-C");

    let fp_d = cluster
        .spawn_node("Chain-D", vec!["Subnet3".into()], false)
        .await
        .expect("spawn Chain-D");

    // Verify boundary isolation
    assert!(matrix.can_reach(&fp_a, &fp_b));
    assert!(matrix.can_reach(&fp_b, &fp_c));
    assert!(matrix.can_reach(&fp_c, &fp_d));
    assert!(!matrix.can_reach(&fp_a, &fp_c));
    assert!(!matrix.can_reach(&fp_a, &fp_d));
    assert!(!matrix.can_reach(&fp_b, &fp_d));

    // 2. Establish 1-hop links and propagate 3 rounds of gossip
    cluster.establish_direct_links().await;
    cluster
        .exchange_gossip_rounds(3)
        .await
        .expect("3 gossip rounds");

    let node_a = cluster.get_node(&fp_a).unwrap();
    let node_d = cluster.get_node(&fp_d).unwrap();

    // 3. Verify 3-hop route resolution on Node A
    let route_to_d = {
        let graph = node_a.mesh_router().graph().read().await;
        graph
            .resolve_route(&fp_d)
            .expect("Node A must resolve route to Node D across 3 hops")
    };

    assert_eq!(route_to_d.hop_count, 3);
    assert_eq!(route_to_d.next_hop_fp, fp_b);
    assert_eq!(
        route_to_d.full_route_path,
        vec![fp_a.clone(), fp_b.clone(), fp_c.clone(), fp_d.clone()]
    );

    // Verify Node A's PeerRegistry tracks Node D as 3 hops away
    let peer_d = node_a
        .registry()
        .get(&fp_d)
        .await
        .expect("Node D must be in Node A registry");
    assert_eq!(peer_d.mesh_hop_count(), Some(3));
    assert_eq!(
        peer_d.mesh_route_path(),
        Some(vec![fp_a.clone(), fp_b.clone(), fp_c.clone(), fp_d.clone()])
    );

    // 4. Cut-through multi-chunk streaming across 3 hops (A -> B -> C -> D)
    let stream_chunks_received = Arc::new(Mutex::new(Vec::new()));
    let sink_chunks = stream_chunks_received.clone();
    node_d
        .mesh_router()
        .set_stream_sink(move |_sid, chunk| {
            sink_chunks.lock().unwrap().push(chunk);
        })
        .await;

    // Stream 512 KB across 4 x 128 KB chunks
    let mut chunks = Vec::new();
    let mut total_hasher = Sha256::new();
    for i in 0..4 {
        let chunk = Bytes::from(vec![(i + 1) as u8; 128 * 1024]);
        total_hasher.update(&chunk);
        chunks.push(chunk);
    }
    let expected_sha256 = format!("{:064X}", total_hasher.finalize());

    let stream_id = format!("stream-3hop-{}", uuid::Uuid::new_v4());
    let stats = node_a
        .mesh_router()
        .send_stream(&fp_d, "file-512kb", &stream_id, chunks)
        .await
        .expect("cut-through stream across 3 hops must succeed");

    assert_eq!(stats.status, "delivered");
    assert_eq!(stats.bytes_received, 512 * 1024);
    assert_eq!(stats.sha256, expected_sha256);

    {
        let received = stream_chunks_received.lock().unwrap();
        assert!(!received.is_empty(), "must receive chunks");
        assert_eq!(received.iter().map(|c| c.len()).sum::<usize>(), 512 * 1024);
    }

    // 5. Test 3-hop E2EE envelope forwarding
    let (tx_env, mut rx_env) = tokio::sync::mpsc::channel(2);
    node_d
        .mesh_router()
        .set_packet_sink(move |env| {
            let _ = tx_env.try_send(env);
        })
        .await;

    let payload = b"E2EE payload across 3 hops: Node A -> Relay B -> Relay C -> Node D";
    let sealed = seal_envelope(
        node_a.tls_identity(),
        &fp_d,
        &node_d.tls_identity().cert_pem,
        payload,
    )
    .expect("sealing E2EE envelope");

    let routed = RoutedEnvelope {
        packet_id: format!("env-3hop-{}", uuid::Uuid::new_v4()),
        src_fp: fp_a.clone(),
        dst_fp: fp_d.clone(),
        ttl: 5,
        route_path: vec![fp_a.clone()],
        payload: sealed,
    };

    let fwd_resp = node_a
        .mesh_router()
        .forward_envelope(routed)
        .await
        .expect("forwarding 3-hop envelope");

    assert_eq!(fwd_resp.status, "forwarded");
    assert_eq!(fwd_resp.next_hop, Some(fp_b.clone()));

    let received_envelope = tokio::time::timeout(Duration::from_secs(3), rx_env.recv())
        .await
        .expect("timed out waiting for 3-hop envelope")
        .expect("envelope arrived");

    let opened = open_envelope(
        node_d.tls_identity(),
        Some(&node_a.tls_identity().cert_pem),
        &received_envelope.payload,
    )
    .expect("opening 3-hop envelope");
    assert_eq!(opened, payload);

    cluster.shutdown();
}

#[tokio::test]
async fn test_dual_relay_failover_under_simulated_link_cut() {
    // 1. Topology: Diamond topology with Node A, Relay B1 (primary), Relay B2 (backup), and Node C
    //    A [SubnetA] <-> B1 [SubnetA, SubnetB] <-> C [SubnetB]
    //    A [SubnetA] <-> B2 [SubnetA, SubnetB] <-> C [SubnetB]
    let matrix = Arc::new(VirtualNetworkMatrix::new());
    let mut cluster = SimulatedMeshCluster::new(matrix.clone());

    let fp_a = cluster
        .spawn_node("Failover-A", vec!["SubnetA".into()], false)
        .await
        .expect("spawn A");

    let fp_b1 = cluster
        .spawn_node(
            "Relay-B1-Primary",
            vec!["SubnetA".into(), "SubnetB".into()],
            true,
        )
        .await
        .expect("spawn B1");

    let fp_b2 = cluster
        .spawn_node(
            "Relay-B2-Backup",
            vec!["SubnetA".into(), "SubnetB".into()],
            true,
        )
        .await
        .expect("spawn B2");

    let fp_c = cluster
        .spawn_node("Failover-C", vec!["SubnetB".into()], false)
        .await
        .expect("spawn C");

    // 2. Program link costs:
    //    Relay B1: Low latency (RTT = 5ms)
    //    Relay B2: High latency (RTT = 60ms)
    matrix.set_bidirectional_link_config(
        &fp_a,
        &fp_b1,
        SimulatedLinkConfig::new().with_latency(5),
    );
    matrix.set_bidirectional_link_config(
        &fp_b1,
        &fp_c,
        SimulatedLinkConfig::new().with_latency(5),
    );
    matrix.set_bidirectional_link_config(
        &fp_a,
        &fp_b2,
        SimulatedLinkConfig::new().with_latency(60),
    );
    matrix.set_bidirectional_link_config(
        &fp_b2,
        &fp_c,
        SimulatedLinkConfig::new().with_latency(5),
    );

    // 3. Connect 1-hop direct links & propagate 2 rounds of gossip
    cluster.establish_direct_links().await;
    cluster
        .exchange_gossip_rounds(2)
        .await
        .expect("gossip rounds");

    let node_a = cluster.get_node(&fp_a).unwrap();
    let node_c = cluster.get_node(&fp_c).unwrap();

    // 4. Verify initial route choice on Node A prefers Relay B1 (lowest composite cost)
    let initial_route = {
        let graph = node_a.mesh_router().graph().read().await;
        graph
            .resolve_route(&fp_c)
            .expect("must resolve route to Node C")
    };

    assert_eq!(
        initial_route.next_hop_fp, fp_b1,
        "Node A must prefer Relay B1 over high-latency Relay B2"
    );
    assert_eq!(
        initial_route.full_route_path,
        vec![fp_a.clone(), fp_b1.clone(), fp_c.clone()]
    );

    // 5. Test initial delivery through Relay B1
    let (tx_c, mut rx_c) = tokio::sync::mpsc::channel(4);
    node_c
        .mesh_router()
        .set_packet_sink(move |env| {
            let _ = tx_c.try_send(env);
        })
        .await;

    let payload_1 = b"Message via primary Relay B1";
    let sealed_1 = seal_envelope(
        node_a.tls_identity(),
        &fp_c,
        &node_c.tls_identity().cert_pem,
        payload_1,
    )
    .unwrap();

    let routed_1 = RoutedEnvelope {
        packet_id: format!("env-b1-{}", uuid::Uuid::new_v4()),
        src_fp: fp_a.clone(),
        dst_fp: fp_c.clone(),
        ttl: 4,
        route_path: vec![fp_a.clone()],
        payload: sealed_1,
    };

    let fwd_1 = node_a
        .mesh_router()
        .forward_envelope(routed_1)
        .await
        .unwrap();
    assert_eq!(fwd_1.next_hop, Some(fp_b1.clone()));

    let rec_1 = tokio::time::timeout(Duration::from_secs(3), rx_c.recv())
        .await
        .unwrap()
        .unwrap();
    let dec_1 = open_envelope(
        node_c.tls_identity(),
        Some(&node_a.tls_identity().cert_pem),
        &rec_1.payload,
    )
    .unwrap();
    assert_eq!(dec_1, payload_1);

    // 6. Dynamic link severing: cut the link between Node A and primary Relay B1
    matrix.sever_link(&fp_a, &fp_b1);
    assert!(matrix.is_link_severed(&fp_a, &fp_b1));
    assert!(!matrix.can_reach(&fp_a, &fp_b1));

    // Remove severed link from Node A's topology graph (simulating link failure / heartbeat timeout)
    {
        let mut graph = node_a.mesh_router().graph().write().await;
        graph.remove_bidirectional_link(&fp_a, &fp_b1);
    }

    // 7. Verify automatic route failover to backup Relay B2
    let failover_route = {
        let graph = node_a.mesh_router().graph().read().await;
        graph
            .resolve_route(&fp_c)
            .expect("Node A must automatically failover to Relay B2")
    };

    assert_eq!(
        failover_route.next_hop_fp, fp_b2,
        "Next hop must have failed over to Relay B2"
    );
    assert_eq!(
        failover_route.full_route_path,
        vec![fp_a.clone(), fp_b2.clone(), fp_c.clone()]
    );

    // 8. Test successful delivery across failover route (Relay B2)
    let payload_2 = b"Message via failover Relay B2 after link cut!";
    let sealed_2 = seal_envelope(
        node_a.tls_identity(),
        &fp_c,
        &node_c.tls_identity().cert_pem,
        payload_2,
    )
    .unwrap();

    let routed_2 = RoutedEnvelope {
        packet_id: format!("env-b2-{}", uuid::Uuid::new_v4()),
        src_fp: fp_a.clone(),
        dst_fp: fp_c.clone(),
        ttl: 4,
        route_path: vec![fp_a.clone()],
        payload: sealed_2,
    };

    let fwd_2 = node_a
        .mesh_router()
        .forward_envelope(routed_2)
        .await
        .expect("forwarding via backup relay B2 must succeed");
    assert_eq!(fwd_2.next_hop, Some(fp_b2.clone()));

    let rec_2 = tokio::time::timeout(Duration::from_secs(3), rx_c.recv())
        .await
        .expect("timed out waiting for failover message")
        .expect("message received");
    let dec_2 = open_envelope(
        node_c.tls_identity(),
        Some(&node_a.tls_identity().cert_pem),
        &rec_2.payload,
    )
    .unwrap();
    assert_eq!(dec_2, payload_2);

    // 9. Verify stream transfer also succeeds over the failover path
    let stream_chunks = vec![
        Bytes::from(vec![0x11; 64 * 1024]),
        Bytes::from(vec![0x22; 64 * 1024]),
    ];
    let stream_id = format!("stream-failover-{}", uuid::Uuid::new_v4());
    let stream_stats = node_a
        .mesh_router()
        .send_stream(&fp_c, "file-failover", &stream_id, stream_chunks)
        .await
        .expect("streaming over failover route must succeed");

    assert_eq!(stream_stats.status, "delivered");
    assert_eq!(stream_stats.bytes_received, 128 * 1024);

    cluster.shutdown();
}
