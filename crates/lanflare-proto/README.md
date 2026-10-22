# lanflare-proto

[![Crates.io](https://img.shields.io/crates/v/lanflare-proto.svg)](https://crates.io/crates/lanflare-proto)
[![Documentation](https://docs.rs/lanflare-proto/badge.svg)](https://docs.rs/lanflare-proto)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

Protocol definitions, serialization models, and Data Transfer Objects (DTOs) for [LanFlare](https://github.com/fokx/lanflare), fully compatible with the official **LocalSend v2.2 specification**.

## Features

- **Strict Specification Conformance**: Implements official LocalSend v2.2 wire models including Multicast announcement, HTTP registration (`/api/localsend/v2/register`), transfer negotiation (`/api/localsend/v2/prepare-upload`), and reverse browser download (`/api/localsend/v2/prepare-download`).
- **Resilient Deserialization**: Implements custom serde deserializers mapping unknown device types cleanly to desktop fallback per Section 7.1 of the LocalSend specification.
- **Lightweight & Fast**: Zero extra overhead, standalone dependency tree relying only on `serde`, `serde_json`, and `thiserror`.

## Usage

Add this to your `Cargo.toml`:

```toml
[dependencies]
lanflare-proto = "0.5.0"

```

### Example: Multicast Announcement

```rust
use lanflare_proto::discovery::{MulticastMessageV2, DeviceType, ProtocolType};

let msg = MulticastMessageV2 {
    alias: "Aurora-Laptop".to_string(),
    version: "2.2".to_string(),
    device_model: Some("Linux".to_string()),
    device_type: Some(DeviceType::Desktop),
    fingerprint: "a1b2c3d4e5...".to_string(),
    port: 53317,
    protocol: ProtocolType::Https,
    download: true,
    announce: true,
};

let json = serde_json::to_string(&msg).unwrap();
println!("{json}");
```

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE) or [http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0)).
