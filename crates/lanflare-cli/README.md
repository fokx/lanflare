# lanflare-cli

[![Crates.io](https://img.shields.io/crates/v/lanflare-cli.svg)](https://crates.io/crates/lanflare-cli)
[![Documentation](https://docs.rs/lanflare-cli/badge.svg)](https://docs.rs/lanflare-cli)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

Command-line interface and alias package for [**LanFlare**](https://crates.io/crates/lanflare).

## Installation

You can install the CLI using either:

```bash
cargo install lanflare
```

or via this alias package:

```bash
cargo install lanflare-cli
```

Both commands provide the `lanflare` and `lanflare-cli` utilities for cross-platform local network file sharing compatible with [LocalSend](https://localsend.org).

## Quick Start

```bash
# Scan local network for peers
lanflare scan

# Send files to a peer
lanflare send document.pdf photo.jpg

# Start receiving files
lanflare receive

# Check device status
lanflare status
```

For comprehensive documentation, visit the [LanFlare GitHub Repository](https://github.com/fokx/lanflare).

## License

Licensed under the Apache License, Version 2.0 ([LICENSE](../../LICENSE) or [http://www.apache.org/licenses/LICENSE-2.0](http://www.apache.org/licenses/LICENSE-2.0)).
