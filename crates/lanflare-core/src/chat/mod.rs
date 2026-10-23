//! Delay-tolerant P2P & group chat, anti-harassment trust gate, persistent store, and outbox.
//!
//! Scaffold module for Phase 8 & Phase 9.

pub mod engine;
pub mod group;
pub mod outbox;
pub mod store;
pub mod sync;
pub mod trust;

pub use trust::{
    compute_time_bucket, count_leading_zeros, generate_hashcash_pow, verify_hashcash_pow,
    ContactTrustState, TokenBucket, TrustError, TrustGate, TrustGateConfig,
};
