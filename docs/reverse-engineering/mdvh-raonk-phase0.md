# MDVH / RAON Phase 0 Notes

## Captured Workflow

Selected file metadata appears in `download-state-snapshot.detail.selectedFiles`:

- `fileName`: displayed artifact filename.
- `serverPath`: internal SSCM path, for example `F:/SSCM_FILE/202606/OUTPUT_QB/...qb`.
- `size`: expected output bytes.
- `binaryId` and `fileId`: task/file identifiers from the MDVH page.

The browser then calls:

```text
selectFileDownload()
RAONKUPLOAD_BeforeDownloadFile("kupload", {
  strCmd: "downloadAll",
  strIsWebFile: "1",
  strName: "...tar.md5",
  strPath: "F:/SSCM_FILE/...qb"
})
```

## RAON Config Hints

- `runtimes`: `agent`
- `resume_mode`: download enabled
- `use_download_cache`: enabled
- monitoring URLs:
  - `http://10.195.20.163:80/raonkupload/handler/raonkmonitor.jsp`
  - `http://10.195.20.165:80/kmonitor/raonkmonitor.jsp`

## Phase 0 Strategy

1. Parse the workflow JSON.
2. Resolve local agent port from `RAONKSolutionAgent.connectedPort`.
3. Probe local agent endpoints.
4. Replay candidate download payloads.
5. If no endpoint streams bytes, use traffic capture to map the local agent protocol.
