# Protocol & Wire Specification

LanFlare implements the official **LocalSend Protocol v2.2** specification, ensuring drop-in compatibility with official mobile and desktop applications.

---

## Control Endpoints

All HTTP/HTTPS endpoints are mounted under `/api/localsend/v2/`.

### 1. Peer Discovery & Identification

| Endpoint | Method | Description | Request Body | Response Body |
| :--- | :--- | :--- | :--- | :--- |
| `/api/localsend/v2/info` | `GET` | Retrieve device info | Empty | `InfoRegisterDto` |
| `/api/localsend/v2/register`| `POST`| Unicast announcement response | `RegisterDtoV2` | `InfoRegisterDto` |

#### `InfoRegisterDto` Schema
```json
{
  "alias": "LanFlare-Workstation",
  "version": "2.2",
  "deviceModel": "Linux",
  "deviceType": "desktop",
  "fingerprint": "8d3e21...",
  "port": 53317,
  "protocol": "https",
  "download": true
}
```

---

### 2. File Transfer Orchestration

#### Step 1: Transfer Preparation (`POST /api/localsend/v2/prepare-upload`)
The sender requests permission to upload files, specifying file metadata and optionally a security PIN.

**Request Body**:
```json
{
  "info": {
    "alias": "Pixel-Phone",
    "version": "2.2",
    "deviceType": "mobile",
    "fingerprint": "c4d29a...",
    "port": 53317,
    "protocol": "https",
    "download": true
  },
  "files": {
    "file-id-1": {
      "id": "file-id-1",
      "fileName": "landscape.jpg",
      "size": 4194304,
      "fileType": "image/jpeg",
      "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    }
  }
}
```

**Response Body (Accepted)**:
```json
{
  "sessionId": "4b7b2528-766f-4318-bd73-a81d4da34389",
  "files": {
    "file-id-1": "file-token-secret-alpha"
  }
}
```

#### Step 2: Binary File Streaming (`POST /api/localsend/v2/upload`)
Query parameters identify the target file and session:
- `sessionId`: The session UUID granted in Step 1.
- `fileId`: The file identifier.
- `token`: The security token associated with this file.

The HTTP body contains the raw binary bytes of the file. LanFlare computes the SHA-256 hash concurrently as bytes arrive over the TCP stream.

---

### 3. Reverse File Transfer (Browser Download Mode)

When LanFlare acts as a file host for devices without an app (e.g., smart TVs, web browsers):

1. **`POST /api/localsend/v2/prepare-download`**:
   Sender registers files to be offered for download and receives a download session ID.
2. **`GET /api/localsend/v2/download`**:
   The remote peer or browser navigates to `http://<ip>:<port>/api/localsend/v2/download?sessionId=...&fileId=...` to stream the file directly.
