//! Multi-hop overlay mesh networking, gossip topology, cut-through stream relay, and virtual simulator.
//!
//! Sub-phase 7.2: Link-state / path-vector topology graph and Dijkstra shortest-path route resolver.

pub mod diagnostics;
pub mod graph;
pub mod mailbox;
pub mod router;
pub mod sim;

pub use graph::{
    MeshGraphError, MeshLink, MeshNode, MeshTopologyGraph, ResolvedRoute, DEFAULT_LINK_EXPIRY_MS,
    DEFAULT_NODE_EXPIRY_MS,
};
pub use router::{
    ForwardResponse, MeshRouter, MeshRouterConfig, MeshTopologyView, PacketDedupBuffer,
    RouterError, StreamDeliveryStats, DEFAULT_DEDUP_CAPACITY, DEFAULT_STREAM_BUFFER_CHUNKS,
};
