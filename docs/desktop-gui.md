# Desktop & Tauri v2 Integration

LanFlare includes a modern, high-performance desktop application (`lanflare-desktop`) powered by **Tauri v2** and **SvelteKit 2** with **Svelte 5 Runes**.

---

## Architectural Bridge

```
┌────────────────────────────────────────────────────────────────┐
│                         Desktop Webview                        │
│             SvelteKit 2 + Svelte 5 (Runes Reactivity)          │
│   ├── Radar Tab: Live animated peer discovery visualization   │
│   ├── Send Tab: Multi-file picker & target peer dispatch       │
│   ├── Receive Tab: Incoming session approval prompt            │
│   └── Settings Tab: Device alias, download path, mTLS & PIN    │
└───────────────────────────────┬────────────────────────────────┘
                                │ Tauri IPC (invoke / emit)
                                ▼
┌────────────────────────────────────────────────────────────────┐
│                     lanflare-desktop Core                      │
│   ├── Tauri Setup Hook: Spawns Tokio background reactors       │
│   ├── State: DesktopState (Arc<RwLock<AppConfig>>, Registry)   │
│   └── IPC Handlers: `get_peers`, `send_files`, `respond_transfer│
└───────────────────────────────┬────────────────────────────────┘
                                │
                                ▼
┌────────────────────────────────────────────────────────────────┐
│                         lanflare-core                          │
│     Discovery Engines, Hyper HTTP/HTTPS Server & Client        │
└────────────────────────────────────────────────────────────────┘
```

---

## Tokio Reactor Safeguards

In desktop graphical applications, the OS main thread drives the window event loop. Spawning Tokio async tasks directly on the main thread during initialization without an active reactor causes panics.

LanFlare implements explicit runtime guards:
- `tauri::async_runtime::spawn`: Used inside the `.setup()` hook to ensure discovery engines and registry eviction tasks run on the background async reactor.
- Safe initialization checks verify reactor presence before spawning background loops.

---

## Frontend Architecture

- **Svelte 5 Runes**:
  - `AppState` ([`web/src/lib/state.svelte.ts`](file:///f/lanflare/web/src/lib/state.svelte.ts)) uses `$state` and `$derived.by` for reactive synchronization of peers, active transfers, and configuration.
- **Dual-Mode IPC Adapter**:
  - [`web/src/lib/api.ts`](file:///f/lanflare/web/src/lib/api.ts) inspects `window.__TAURI_INTERNALS__`.
  - When running inside the Tauri native window, it routes commands to Tauri's IPC bridge.
  - When running in a standalone web browser (e.g. `pnpm dev`), it seamlessly falls back to mock responses, allowing rapid UI development without compiling the Rust backend.
