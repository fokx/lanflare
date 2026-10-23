//! Multi-functional LAN collaboration toolbox: universal clipboard ring, federated search, and CRDT scratchpad.
//!
//! Scaffold module for Phase 10.

pub mod clipboard;
pub mod collab;
pub mod search;

pub use clipboard::{ClipboardError, DeviceRing, HandleClipboardResult, RingSecret};
pub use collab::{CollabError, CollabManager, ScratchpadDoc};
pub use search::{CatalogItem, FederatedSearchEngine, MeshFileCatalog, SearchError};
