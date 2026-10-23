//! Group Anti-Entropy Frontier Synchronization & Multi-Hop Relay E2E Test Suite.
//!
//! Sub-phase 9.2 verification:
//! - `test_four_peer_group_partition_and_anti_entropy_catchup`:
//!   In a 4-peer group where Peer D is offline while Peers A, B, and C exchange 20 messages,
//!   bringing Peer D online synchronizes all 20 missing messages in exact causal order within 1 sync round.
//! - `test_group_sync_over_multi_hop_mesh_relay`:
//!   End-to-end anti-entropy frontier synchronization across isolated subnets mediated by an intermediate relay.

use std::sync::Arc;

use lanflare_core::chat::group::GroupDagState;
use lanflare_core::chat::{GroupMember, GroupRole};
use lanflare_core::mesh::sim::{SimulatedMeshCluster, VirtualNetworkMatrix};

#[tokio::test]
async fn test_four_peer_group_partition_and_anti_entropy_catchup() {
    let matrix = Arc::new(VirtualNetworkMatrix::new());
    let mut cluster = SimulatedMeshCluster::new(matrix.clone());

    // 1. Spawn 4 nodes on SubnetA: Alice, Bob, Charlie, and Dan
    let fp_a = cluster
        .spawn_node_with_chat("Alice", vec!["SubnetA".into()], false)
        .await
        .expect("spawn Alice");

    let fp_b = cluster
        .spawn_node_with_chat("Bob", vec!["SubnetA".into()], false)
        .await
        .expect("spawn Bob");

    let fp_c = cluster
        .spawn_node_with_chat("Charlie", vec!["SubnetA".into()], false)
        .await
        .expect("spawn Charlie");

    let fp_d = cluster
        .spawn_node_with_chat("Dan", vec!["SubnetA".into()], false)
        .await
        .expect("spawn Dan");

    let node_a = cluster.get_node(&fp_a).unwrap();
    let node_b = cluster.get_node(&fp_b).unwrap();
    let node_c = cluster.get_node(&fp_c).unwrap();
    let node_d = cluster.get_node(&fp_d).unwrap();

    let engine_a = node_a.chat_engine().expect("engine A").clone();
    let engine_b = node_b.chat_engine().expect("engine B").clone();
    let engine_c = node_c.chat_engine().expect("engine C").clone();
    let engine_d = node_d.chat_engine().expect("engine D").clone();

    // Establish direct neighbor links
    cluster.establish_direct_links().await;

    // 2. Alice forms a 4-member group
    let member_b = GroupMember {
        fingerprint: fp_b.clone(),
        alias: "Bob".to_string(),
        cert_pem: engine_b.identity().cert_pem.clone(),
        role: GroupRole::Member,
        joined_at_ms: 1000,
    };
    let member_c = GroupMember {
        fingerprint: fp_c.clone(),
        alias: "Charlie".to_string(),
        cert_pem: engine_c.identity().cert_pem.clone(),
        role: GroupRole::Member,
        joined_at_ms: 1000,
    };
    let member_d = GroupMember {
        fingerprint: fp_d.clone(),
        alias: "Dan".to_string(),
        cert_pem: engine_d.identity().cert_pem.clone(),
        role: GroupRole::Member,
        joined_at_ms: 1000,
    };

    let (alice_group_arc, _epoch_key, dists) = engine_a
        .create_group("LanFlare Core Team", vec![member_b, member_c, member_d])
        .await
        .expect("Alice creates group");

    let roster = {
        let state = alice_group_arc.read().await;
        state.roster().clone()
    };
    let group_id = roster.group_id.clone();

    // Bob and Charlie receive roster & dists and register group
    let mut bob_dag = GroupDagState::new(roster.clone(), fp_b.clone());
    for dist in &dists {
        bob_dag.add_key_distribution(dist.clone());
        if dist.recipient_fp.eq_ignore_ascii_case(&fp_b) {
            bob_dag
                .apply_epoch_key_distribution(engine_b.identity(), dist)
                .expect("Bob applies key");
        }
    }
    let bob_group_arc = engine_b.register_group(bob_dag).await;

    let mut charlie_dag = GroupDagState::new(roster.clone(), fp_c.clone());
    for dist in &dists {
        charlie_dag.add_key_distribution(dist.clone());
        if dist.recipient_fp.eq_ignore_ascii_case(&fp_c) {
            charlie_dag
                .apply_epoch_key_distribution(engine_c.identity(), dist)
                .expect("Charlie applies key");
        }
    }
    let charlie_group_arc = engine_c.register_group(charlie_dag).await;

    // 3. Dan is OFFLINE and partitioned!
    // Peers Alice, Bob, and Charlie author 20 messages among themselves.
    let mut authored_texts = Vec::new();
    for i in 1..=20 {
        let text = format!("Message #{i} authored during Dan's offline partition");
        authored_texts.push(text.clone());

        match i % 3 {
            1 => {
                engine_a
                    .author_group_message(&group_id, text.as_bytes())
                    .await
                    .expect("Alice authors");
            }
            2 => {
                engine_b
                    .author_group_message(&group_id, text.as_bytes())
                    .await
                    .expect("Bob authors");
            }
            0 => {
                engine_c
                    .author_group_message(&group_id, text.as_bytes())
                    .await
                    .expect("Charlie authors");
            }
            _ => unreachable!(),
        }

        // Alice, Bob, Charlie periodically sync among each other to propagate concurrent branches
        if i % 5 == 0 {
            engine_b
                .group_sync()
                .sync_with_peer_http(&node_a.base_url(), &group_id)
                .await
                .expect("Bob syncs with Alice");
            engine_c
                .group_sync()
                .sync_with_peer_http(&node_b.base_url(), &group_id)
                .await
                .expect("Charlie syncs with Bob");
            engine_a
                .group_sync()
                .sync_with_peer_http(&node_c.base_url(), &group_id)
                .await
                .expect("Alice syncs with Charlie");
        }
    }

    // Final convergence sync between Alice, Bob, Charlie
    engine_b
        .group_sync()
        .sync_with_peer_http(&node_a.base_url(), &group_id)
        .await
        .expect("Bob syncs with Alice");
    engine_c
        .group_sync()
        .sync_with_peer_http(&node_a.base_url(), &group_id)
        .await
        .expect("Charlie syncs with Alice");
    engine_a
        .group_sync()
        .sync_with_peer_http(&node_b.base_url(), &group_id)
        .await
        .expect("Alice syncs with Bob");
    engine_a
        .group_sync()
        .sync_with_peer_http(&node_c.base_url(), &group_id)
        .await
        .expect("Alice syncs with Charlie");

    // Verify Alice, Bob, and Charlie each hold exactly 20 messages and identical deterministic sort order
    let alice_order = {
        let state = alice_group_arc.read().await;
        assert_eq!(state.len(), 20);
        state.topological_sort()
    };
    let bob_order = {
        let state = bob_group_arc.read().await;
        assert_eq!(state.len(), 20);
        state.topological_sort()
    };
    let charlie_order = {
        let state = charlie_group_arc.read().await;
        assert_eq!(state.len(), 20);
        state.topological_sort()
    };

    assert_eq!(alice_order, bob_order, "Alice and Bob order must match");
    assert_eq!(alice_order, charlie_order, "Alice and Charlie order must match");

    // 4. DAN COMES ONLINE!
    // Dan initializes group from roster (0 messages, no epoch key yet)
    let dan_dag = GroupDagState::new(roster.clone(), fp_d.clone());
    let dan_group_arc = engine_d.register_group(dan_dag).await;

    {
        let state = dan_group_arc.read().await;
        assert_eq!(state.len(), 0, "Dan starts with 0 messages");
        assert!(state.get_epoch_key(1).is_none(), "Dan has no epoch key yet");
    }

    // Dan synchronizes with Alice in EXACTLY ONE sync round
    let sync_stats = engine_d
        .group_sync()
        .sync_with_peer_http(&node_a.base_url(), &group_id)
        .await
        .expect("Dan syncs with Alice");

    assert_eq!(sync_stats.nodes_pulled, 20, "Dan must pull all 20 missing messages in 1 round");
    assert_eq!(sync_stats.epoch_keys_added, 1, "Dan must receive epoch 1 key");

    // 5. Verify Dan now has all 20 messages in identical topological order
    let dan_order = {
        let state = dan_group_arc.read().await;
        assert_eq!(state.len(), 20, "Dan must now have 20 messages");
        assert!(state.get_epoch_key(1).is_some(), "Dan must now hold epoch 1 key");
        state.topological_sort()
    };

    assert_eq!(dan_order, alice_order, "Dan's topological sort must be 100% identical to Alice's");

    // 6. Verify Dan can decrypt all 20 message payloads with 100% accuracy
    {
        let state = dan_group_arc.read().await;
        let mut decrypted_texts = Vec::new();
        for id in &dan_order {
            let node = state.get_node(id).expect("node exists");
            let plaintext_bytes = state.decrypt_node_payload(node).expect("decrypt node payload");
            let plaintext = String::from_utf8(plaintext_bytes).expect("valid utf-8");
            decrypted_texts.push(plaintext);
        }

        assert_eq!(decrypted_texts.len(), 20);
        for expected in &authored_texts {
            assert!(
                decrypted_texts.contains(expected),
                "Dan must successfully decrypt '{expected}'"
            );
        }
    }

    // 7. Verify subsequent sync round from Dan yields 0 missing nodes (anti-entropy convergence)
    let second_sync = engine_d
        .group_sync()
        .sync_with_peer_http(&node_a.base_url(), &group_id)
        .await
        .expect("Dan second sync with Alice");

    assert_eq!(second_sync.nodes_pulled, 0, "Second sync must pull 0 nodes");
    assert_eq!(second_sync.epoch_keys_added, 0, "Second sync must add 0 keys");
}

#[tokio::test]
async fn test_group_sync_over_multi_hop_mesh_relay() {
    let matrix = Arc::new(VirtualNetworkMatrix::new());
    let mut cluster = SimulatedMeshCluster::new(matrix.clone());

    // 1. Setup 3 nodes across 2 subnets:
    // Node A on SubnetA only (Alice)
    let fp_a = cluster
        .spawn_node_with_chat("Node-A", vec!["SubnetA".into()], false)
        .await
        .expect("spawn Node-A");

    // Relay B bridging SubnetA and SubnetB (Relay)
    let fp_b = cluster
        .spawn_node_with_chat("Relay-B", vec!["SubnetA".into(), "SubnetB".into()], true)
        .await
        .expect("spawn Relay-B");

    // Node C on SubnetB only (Charlie)
    let fp_c = cluster
        .spawn_node_with_chat("Node-C", vec!["SubnetB".into()], false)
        .await
        .expect("spawn Node-C");

    // Verify physical subnet isolation: A and C cannot communicate directly
    assert!(matrix.can_reach(&fp_a, &fp_b));
    assert!(matrix.can_reach(&fp_b, &fp_c));
    assert!(!matrix.can_reach(&fp_a, &fp_c), "A and C must be strictly partitioned");

    // Connect 1-hop physical neighbors and execute gossip rounds so A and C discover routes
    cluster.establish_direct_links().await;
    cluster.exchange_gossip_rounds(2).await.expect("gossip exchange");

    let node_a = cluster.get_node(&fp_a).unwrap();
    let node_c = cluster.get_node(&fp_c).unwrap();
    let engine_a = node_a.chat_engine().expect("engine A").clone();
    let engine_c = node_c.chat_engine().expect("engine C").clone();

    // Verify route resolution on Node A and Node C across Relay B
    {
        let graph_a = node_a.mesh_router().graph().read().await;
        let route_c = graph_a.resolve_route(&fp_c).expect("route to C");
        assert_eq!(route_c.next_hop_fp, fp_b);
        assert_eq!(route_c.full_route_path, vec![fp_a.clone(), fp_b.clone(), fp_c.clone()]);
    }
    {
        let graph_c = node_c.mesh_router().graph().read().await;
        let route_a = graph_c.resolve_route(&fp_a).expect("route to A");
        assert_eq!(route_a.next_hop_fp, fp_b);
        assert_eq!(route_a.full_route_path, vec![fp_c.clone(), fp_b.clone(), fp_a.clone()]);
    }

    // 2. Node A creates a group with Node C as a member
    let member_c = GroupMember {
        fingerprint: fp_c.clone(),
        alias: "Node-C".to_string(),
        cert_pem: engine_c.identity().cert_pem.clone(),
        role: GroupRole::Member,
        joined_at_ms: 1000,
    };

    let (group_a_arc, _epoch_key, _dists) = engine_a
        .create_group("Mesh Secret Group", vec![member_c])
        .await
        .expect("Alice creates group");

    let roster = {
        let state = group_a_arc.read().await;
        state.roster().clone()
    };
    let group_id = roster.group_id.clone();

    // Node C registers group with roster (0 messages, no epoch key yet)
    let dag_c = GroupDagState::new(roster.clone(), fp_c.clone());
    let group_c_arc = engine_c.register_group(dag_c).await;

    // 3. Node A authors 5 group messages
    let mut expected_texts = Vec::new();
    for i in 1..=5 {
        let msg_text = format!("Multi-hop group message #{i} via Relay B");
        expected_texts.push(msg_text.clone());
        engine_a
            .author_group_message(&group_id, msg_text.as_bytes())
            .await
            .expect("Alice authors");
    }

    {
        let state_a = group_a_arc.read().await;
        assert_eq!(state_a.len(), 5);
    }
    {
        let state_c = group_c_arc.read().await;
        assert_eq!(state_c.len(), 0, "Node C has not synced yet");
    }

    // 4. Node C initiates group anti-entropy sync with Node A over multi-hop mesh!
    // Since direct HTTP between A and C is blocked by VirtualNetworkMatrix,
    // engine_c.sync_group automatically routes the sync request through Relay B to Node A.
    let stats = engine_c
        .sync_group(&group_id, &fp_a)
        .await
        .expect("Node C syncs with Node A over multi-hop mesh");

    assert_eq!(stats.nodes_pulled, 5, "Node C must pull 5 nodes across mesh relay");
    assert_eq!(stats.epoch_keys_added, 1, "Node C must receive epoch 1 key across mesh relay");

    // 5. Verify Node C has all 5 messages and decrypts them
    let c_order = {
        let state_c = group_c_arc.read().await;
        assert_eq!(state_c.len(), 5, "Node C now has all 5 messages");
        assert!(state_c.get_epoch_key(1).is_some(), "Node C has epoch key");
        state_c.topological_sort()
    };

    let a_order = {
        let state_a = group_a_arc.read().await;
        state_a.topological_sort()
    };

    assert_eq!(c_order, a_order, "Causal ordering across mesh must match 100%");

    // Verify decryption of all 5 messages on Node C
    {
        let state_c = group_c_arc.read().await;
        for (idx, id) in c_order.iter().enumerate() {
            let node = state_c.get_node(id).unwrap();
            let decrypted = state_c.decrypt_node_payload(node).expect("Node C decrypts");
            assert_eq!(decrypted, expected_texts[idx].as_bytes());
        }
    }
}
