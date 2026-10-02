pub mod interface;
pub mod mdns;
pub mod multicast;
pub mod registry;
pub mod scanner;

pub use registry::{
    DiscoveredPeer, DiscoverySource, PeerEndpoint, PeerRegistry, PeerRegistryConfig,
    PeerUpdateReason, RegistryEvent,
};

