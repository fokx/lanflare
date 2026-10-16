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
[Android App](#-android-app) •
[Building for Android](#-building-for-android) •
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
- 📂 **Native Android Support**: Built-in Android system Share Sheet target (`ACTION_SEND` / `ACTION_SEND_MULTIPLE`) and direct saving to the public `Download` directory with native MediaScanner indexing.
- 🌐 **Web Download Mode**: Want to send a file to a device without any app installed? LanFlare can generate a local web link so anyone on your Wi-Fi can download files directly using their web browser.
- 💻 **CLI, Desktop, & Mobile**:
  - **CLI (`lanflare`)**: Interactive terminal picker, live streaming progress bars, and script-friendly flags.
  - **Desktop GUI**: Sleek, modern desktop interface powered by Tauri v2 and Svelte 5.
  - **Android APK**: Native mobile app with gesture bar insets, background multicast lock, and clipboard image integration.

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

### Option 3: Android APK

Download the pre-built `.apk` file for Android from the **[Releases](https://github.com/fokx/lanflare/releases)** page and install it on your device (supports Android 9+ / API 28+).


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

## 📱 Android App

LanFlare provides first-class support for Android devices with an optimized mobile interface and deep system integration:

### Mobile Features

- 📤 **Android System Share Sheet**: Share single or multiple photos, videos, documents, or text snippets from any app (Google Photos, Files, Chrome, etc.) directly into LanFlare using Android's native share target (`ACTION_SEND` and `ACTION_SEND_MULTIPLE`).
- 📂 **Direct Public Downloads Directory**: Automatically saves received files to `/storage/emulated/0/Download`—accessible from any Android file explorer (Google Files, Samsung My Files, Solid Explorer)—rather than an inaccessible app-isolated directory.
- 🖼️ **Native MediaScanner Indexing**: As soon as a file transfer finishes, `MediaScannerConnection` scans the saved file so images, audio, and videos appear immediately in your photo gallery and system downloads list.
- 📶 **Wi-Fi Multicast Lock**: Acquires an Android multicast lock to ensure discovery beacons are consistently broadcast and received even when the screen is dimmed or the device enters low-power states.
- 📐 **Edge-to-Edge Responsive UI**: Automatically measures Android system window insets, adapting padding for gesture navigation bars, cutouts, and status bars.

### How to Use on Android

1. **Send Files**:
   - Open LanFlare and tap **Add Files** or paste from the clipboard, then tap any discovered peer on the radar.
   - Alternatively, open your favorite gallery or file manager, select files, tap **Share**, and choose **LanFlare**.
2. **Receive Files**:
   - Keep LanFlare open. When another peer initiates a transfer, an approval prompt appears with the sender's alias and file list.
   - Tap **Accept** to download files directly to your device's **Download** folder.
3. **Storage Access**:
   - On Android 11+ (API 30+), if prompted in **Settings**, tap **Grant Permission** to allow "All files access" so LanFlare can save incoming transfers directly into your public `Download/` directory.

---

## 🔨 Building for Android

LanFlare is built with Tauri v2 and cross-compiles to a native Android APK via the Rust NDK toolchain.

### Prerequisites

Ensure you have the following installed on your development machine:

1. **Rust & Android Target**:
   ```bash
   rustup target add aarch64-linux-android
   ```
2. **Android SDK & NDK**:
   - Android SDK Command-line Tools / SDK Platform 36.
   - Android NDK (version 26 or higher, e.g., NDK 26+ / 30+).
   - Set environment variables:
     ```bash
     export ANDROID_HOME=$HOME/Android/Sdk
     export NDK_HOME=$ANDROID_HOME/ndk/<ndk-version>
     ```
3. **Java Development Kit**:
   - OpenJDK 17 or OpenJDK 21.
4. **Node.js & pnpm**:
   - Node.js 18+ and `pnpm`.
5. **Tauri CLI v2**:
   ```bash
   cargo install tauri-cli --version "^2.0.0"
   ```

### Build Commands

```bash
# 1. Install frontend dependencies and build web assets
cd web
pnpm install
pnpm build
cd ..

# 2. Build the Android debug APK (for ARM64 / modern phones)
cargo tauri android build --apk --debug --target aarch64

# 3. (Optional) Build a production release APK
cargo tauri android build --apk --target aarch64
```

### APK Output Location

After a successful build, the generated APK will be located at:
```text
crates/lanflare-desktop/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```

To install directly to a connected Android phone or emulator via ADB:
```bash
adb install crates/lanflare-desktop/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
```


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
