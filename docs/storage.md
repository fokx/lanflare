# Streaming Storage & Security Engine

LanFlare's storage engine (`lanflare-core::storage`) is engineered for multi-gigabit throughput while offering zero-compromise protection against directory traversal attacks, corrupted transfers, and disk-exhaustion exploits.

---

## Direct Socket-to-Disk Pipeline

```
  TCP Stream (Hyper Body)
           │
           │  128 KB Async Chunks
           ▼
  ┌─────────────────────────────────────────────────────────────┐
  │                 StreamingFileWriter                         │
  │  ├── 1. Compute SHA-256 state incrementally                 │
  │  ├── 2. Track written byte count vs promised quota          │
  │  └── 3. Flush to temp file: `.{name}.{uuid}.lanflare-part`  │
  └──────────────────────────────┬──────────────────────────────┘
                                 │ Transfer Complete
                                 ▼
                    Match promised SHA-256 & size?
                     ├── YES ──> Atomic `rename()` to destination file
                     └── NO  ──> Abort & unlink `.lanflare-part`
```

### Key Properties
1. **Zero Complete In-Memory Buffering**: Files of any size (from a 1 KB photo to a 100 GB virtual disk image) stream directly to disk in 128 KB chunks. RAM usage remains flat and minimal (< 30 MB).
2. **Atomic Inode Rename**: The temporary `.lanflare-part` file is placed in the exact same parent directory as the target destination. This guarantees that `tokio::fs::rename` is an atomic same-filesystem operation. Other applications never observe partial, half-written files.
3. **RAII Automatic Drop Cleanup**: If an upload stream is disconnected, canceled, or encounters a socket error, `StreamingFileWriter`'s `Drop` implementation immediately unlinks the temporary `.lanflare-part` file, preventing disk clutter.

---

## Security Defenses

### 1. Directory Traversal Sanitization
Attackers might attempt to supply file names with relative path sequences (e.g. `../../../../etc/shadow` or `..\..\Windows\System32\cmd.exe`):
- `storage::path::sanitize_filename`:
  - Strips all leading/trailing path separators (`/`, `\`).
  - Removes directory traversal tokens (`.`, `..`).
  - Replaces null bytes (`\0`) and control characters.
  - Neutralizes Windows reserved device names (`CON`, `PRN`, `AUX`, `NUL`, `COM1-9`, `LPT1-9`).
  - Replaces empty or blank results with a safe fallback name (`unnamed_file`).

### 2. Disk Bombing & Allocation Enforcement
- The receiver validates the promised file size during `prepare-upload`.
- As bytes arrive, `StreamingFileWriter` enforces `bytes_written <= expected_size`. If an attacker attempts to send more data than promised to exhaust server disk space, the stream is aborted immediately, the temporary file is deleted, and HTTP 413 / 422 is returned.

### 3. Collision Resolution
If a file with the target name already exists in the destination folder, LanFlare resolves collisions by appending a monotonic counter (e.g., `document (1).pdf`, `document (2).pdf`) without overwriting existing data.
