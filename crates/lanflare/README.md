# lanflare

[![Crates.io](https://img.shields.io/crates/v/lanflare.svg)](https://crates.io/crates/lanflare)
[![Documentation](https://docs.rs/lanflare/badge.svg)](https://docs.rs/lanflare)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

Lightning-fast, cross-platform CLI and interactive TUI for discovering local peers and sending/receiving files across local networks, fully compatible with [LocalSend](https://localsend.org).

## Installation

Install directly via Cargo:

```bash
cargo install lanflare
```

This installs both `lanflare` and `lanflare-cli` command aliases into your Cargo bin directory.

## Quick Start

### 1. Discover Peers

```bash
lanflare scan
```

Scans the local network using UDP multicast, mDNS, and active subnet sweeps, displaying peer aliases, IP addresses, ports, device models, and protocols.

### 2. Send Files

Send files interactively with a peer picker and live progress bar:

```bash
lanflare send report.pdf photo.jpg
```

Or send directly to a known IP address:

```bash
lanflare send archive.tar.gz --to 192.168.1.120
```

### 3. Receive Files

Start an interactive receiver daemon:

```bash
lanflare receive
```

Or run headless auto-accept mode:

```bash
lanflare receive --auto-accept --dir ~/Downloads/LanFlare
```

### 4. Check Local Status

```bash
lanflare status
```

Displays device alias, port, TLS certificate fingerprint, and active network interfaces.

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE) or [http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0)).
