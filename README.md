# Binary Download Manager

Phase 0 project untuk membuktikan download binary dari MDVH/SSCM sebelum membangun desktop UI penuh.

## Phase 0 Goal

Satu file dari metadata RAON/MDVH berhasil tersimpan ke disk dan ukuran file cocok dengan `selectedFiles[].size`.

Fokus saat ini:

- Parse export `RAONK Workflow Spy`.
- Ambil `fileName`, `serverPath`, `size`, `binaryId`, `fileId`, dan `RAONKSolutionAgent.connectedPort`.
- Probe RAON local agent di `127.0.0.1:<port>`.
- Coba replay beberapa kandidat endpoint local agent dengan payload download.
- Tulis report JSON dengan status yang bisa dipakai untuk iterasi reverse engineering berikutnya.

## CLI

```bash
cargo run --manifest-path tools/mdvh-agent-probe/Cargo.toml -- \
  --workflow-json fixtures/raonk-workflow/raonk-workflow-sample.json \
  --output-dir downloads \
  --port 47317
```

Exit codes:

- `0`: file downloaded and size matches.
- `10`: agent reachable but no downloadable stream.
- `20`: agent unreachable.
- `30`: endpoint found but download failed.
- `40`: invalid workflow JSON.

## Current Known MDVH/RAON Facts

- RAON config uses `agent` runtime.
- RAON config enables `<resume_mode upload="1" download="1">`.
- RAON config enables `<use_download_cache>1</use_download_cache>`.
- Browser passes selected files through hidden `selectFileMeta`.
- RAON callback receives `strName` and `strPath`.
- The actual resumable download likely happens inside RAON local agent or an MDVH/SSCM cache handler, not the browser tab.

## Deferred

- Tauri desktop UI.
- Project manager `/tkdn` integration.
- Download queue/history database.
- Automatic task updates.
