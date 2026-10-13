# 🖧🔥 LanFlare

<div align="center">

**Ultra-fast, zero-config, secure local network file sharing.**  
*Fully interoperable with the [LocalSend](https://localsend.org) ecosystem across Android, iOS, Windows, macOS, and Linux.*

[![Crates.io](https://img.shields.io/crates/v/lanflare.svg)](https://crates.io/crates/lanflare)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](LICENSE)
[![Protocol: LocalSend v2.2](https://img.shields.io/badge/Protocol-LocalSend_v2.2-orange.svg)](docs/protocol.md)
[![Rust: 2024 Edition](https://img.shields.io/badge/Rust-2024_Edition-black.svg)](https://www.rust-lang.org)

[Key Features](#-key-features) •
[Quick Start](#-quick-start) •
[CLI Usage](#-cli-usage) •
[Desktop App](#-desktop-app) •
[LocalSend Interop](#-cross-device-compatibility-localsend) •
[Developer Docs](docs/README.md)

</div>

---

## 💡 What is LanFlare?

**LanFlare** is an open-source local file sharing application and command-line utility built in Rust. It lets you send files, folders, and text instantly between devices on the same Wi-Fi or local area network—**no internet access, no cloud storage, and no account registration required**.

LanFlare natively speaks the **LocalSend v2.2 protocol**. If your friend, colleague, or family member has LocalSend on their phone (Android / iPhone) or computer (Mac / Windows / Linux), LanFlare seamlessly detects them and exchanges files at full local network speed.

---

## ✨ Key Features

- ⚡ **Blazing Fast**: Directly streams data socket-to-disk without accumulating files in RAM. Effortlessly maxes out Gigabit Wi-Fi and Ethernet (>2.2 Gbps).
- 🔍 **Zero-Configuration Discovery**: Devices automatically detect each other in real time via UDP multicast and mDNS. Includes an intelligent fast-probe fallback for restricted guest networks and Wi-Fi AP isolation.
- 🔒 **Private & Secure**: Your files never touch external servers or the cloud. Transfers are secured using ephemeral end-to-end TLS encryption with certificate fingerprint verification and optional transfer PINs.
- 📱 **Universal Compatibility**: Works seamlessly with official LocalSend clients on Android, iOS, Windows, macOS, and Linux.
- 🌐 **Web Download Mode**: Want to send a file to a device without any app installed? LanFlare can generate a local web link so anyone on your Wi-Fi can download files directly using their web browser.
- 💻 **Both CLI and Desktop GUI**:
  - **CLI (`lanflare`)**: Interactive terminal picker, live streaming progress bars, and script-friendly flags.
  - **Desktop GUI**: Sleek, modern desktop interface powered by Tauri v2 and Svelte 5.

---

## 🚀 Quick Start

### Option 1: Install the CLI via Cargo

If you have Rust installed, install `lanflare` directly from [crates.io](https://crates.io/crates/lanflare):

```bash
cargo install lanflare
```

This installs both the `lanflare` and `lanflare-cli` commands onto your system path.

### Option 2: Pre-built Desktop App

Download the latest installer or executable for your operating system from the **[Releases](https://github.com/fokx/lanflare/releases)** page:
- **Linux**: `.AppImage` or `.deb`
- **Windows**: `.msi` or `.exe` installer
- **macOS**: `.dmg` (Universal binary for Apple Silicon & Intel)

---

## 🛠️ CLI Usage

The `lanflare` command is intuitive and easy to use.

### 1. Discover Nearby Devices

Scan your local network to see active peers:

```bash
lanflare scan
```

Output:
```text
🔍 Scanning local network for peers (UDP multicast + mDNS)...
✔ Discovered 2 peer(s):
  • Pixel 8 Pro (192.168.1.105:53317) [mobile / Android] (HTTPS)
  • MacBook Air (192.168.1.118:53317) [desktop / macOS] (HTTPS)
```

### 2. Send Files

#### Interactive Mode (With Peer Picker):
Pass any file(s) you wish to transfer. LanFlare scans for nearby peers and presents an interactive menu to select the destination:

```bash
lanflare send vacation-photo.jpg archive.zip
```

#### Direct Send (By IP):
Send directly to a known device IP address without waiting for discovery:

```bash
lanflare send presentation.pdf --to 192.168.1.105
```

#### Protected Transfer with PIN:
If the receiving device requires a PIN code:

```bash
lanflare send secrets.docx --to 192.168.1.105 --pin 1234
```

### 3. Receive Files

#### Interactive Receiver:
Run the receiver daemon. When another device attempts to send files, LanFlare prompts you to accept or decline:

```bash
lanflare receive
```

#### Headless / Automated Receiver:
Automatically accept incoming transfers and save them into a specific directory (great for servers or home labs):

```bash
lanflare receive --auto-accept --dir ~/Downloads/LanFlare
```

### 4. Check Local Device Status

Display your device alias, listening port, network interfaces, and TLS certificate fingerprint:

```bash
lanflare status
```

---

## 🖥️ Desktop App

The desktop version of LanFlare offers an intuitive, responsive graphical user interface:

- **Radar**: Visual, real-time radar sweep displaying nearby devices as they join and leave your Wi-Fi.
- **Send**: Drag-and-drop file staging area with peer selection and speed graphs.
- **Receive**: One-click transfer approvals with detailed sender identity and file breakdowns.
- **History**: Log of past completed transfers and quick open buttons.
- **Settings**: Customize your device nickname, download folder, network ports, and security PIN.

---

## 🤝 Cross-Device Compatibility (LocalSend)

LanFlare is 100% interoperable with the official [LocalSend](https://localsend.org) app:

| From Device | To Device | Works? | Notes |
| :--- | :--- | :---: | :--- |
| **LanFlare** (PC / Linux) | **LocalSend** (Android) | ✅ Yes | Full auto-discovery & high-speed transfer |
| **LanFlare** (PC / Linux) | **LocalSend** (iPhone / iPad) | ✅ Yes | Seamless mTLS transfer & AirDrop alternative |
| **LocalSend** (Any device) | **LanFlare** (Receiver) | ✅ Yes | Prompts approval or auto-accepts |
| **LanFlare** | **Any Web Browser** | ✅ Yes | Browser download mode via direct link |

---

## 🔬 For Developers & Advanced Users

Are you interested in system architecture, network protocols, memory bounds, or benchmarks?

Comprehensive implementation documentation is available in the **[`docs/`](docs/README.md)** directory:

- 🏛️ **[System Architecture](docs/architecture.md)**: Crate hierarchy, async threading model, and security boundaries.
- 📡 **[Protocol & Wire Specification](docs/protocol.md)**: LocalSend v2.2 wire models, REST endpoints, and PIN negotiation state machine.
- 🌐 **[Hybrid Discovery Engine](docs/discovery.md)**: UDP multicast, mDNS (DNS-SD), and parallel subnet scanning fallback.
- 💾 **[Streaming Storage Engine](docs/storage.md)**: Socket-to-disk streaming, atomic rename (`.lanflare-part`), and path traversal defenses.
- ⚡ **[Benchmarks & Performance](docs/benchmarks.md)**: Throughput test suite (>2.2 Gbps loopback) and fuzz testing resilience.
- 🖥️ **[Desktop & Tauri v2 Integration](docs/desktop-gui.md)**: Svelte 5 Runes state management and IPC bridge design.

---

## 📄 License

LanFlare is licensed under the **Apache License, Version 2.0**.  
See the [LICENSE](LICENSE) file for the full license text.
