use std::sync::Arc;

use mdvh_agent_probe::{
    parse_workflow_file, run_probe, DownloadProgress, ProbeOptions, ProgressCallback,
};
use tauri::{AppHandle, Emitter};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[tauri::command]
fn open_download_folder(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let path = std::env::current_dir()
        .map(|p| p.join("downloads"))
        .unwrap_or_else(|_| std::path::PathBuf::from("downloads"));

    // Create downloads folder if it doesn't exist
    let _ = std::fs::create_dir_all(&path);

    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

async fn start_payload_listener(app_handle: AppHandle, port: u16) {
    let bind_addr = format!("127.0.0.1:{}", port);
    let listener = match TcpListener::bind(&bind_addr).await {
        Ok(l) => {
            let _ = app_handle.emit(
                "listener-status",
                serde_json::json!({
                    "active": true,
                    "bind": bind_addr.clone()
                }),
            );
            l
        }
        Err(e) => {
            let _ = app_handle.emit(
                "listener-status",
                serde_json::json!({
                    "active": false,
                    "error": format!("Failed to bind to {}: {}", bind_addr, e)
                }),
            );
            return;
        }
    };

    loop {
        if let Ok((mut stream, _)) = listener.accept().await {
            let app = app_handle.clone();
            tokio::spawn(async move {
                if let Err(e) = handle_stream(&app, &mut stream).await {
                    let _ = app.emit(
                        "download-error",
                        format!("Error reading payload connection: {}", e),
                    );
                }
            });
        }
    }
}

async fn handle_stream(app: &AppHandle, stream: &mut tokio::net::TcpStream) -> anyhow::Result<()> {
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut read_total = 0usize;
    loop {
        let read = stream.read(&mut buffer[read_total..]).await?;
        if read == 0 {
            break;
        }
        read_total += read;

        if read_total >= 4 && buffer[..read_total].windows(4).any(|w| w == b"\r\n\r\n") {
            let header_end = buffer[..read_total]
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
                .map(|pos| pos + 4)
                .ok_or_else(|| anyhow::anyhow!("Request header terminator missing"))?;

            let headers = String::from_utf8_lossy(&buffer[..header_end]).into_owned();

            // Extract Content-Length
            let mut content_length = 0;
            for line in headers.lines() {
                if let Some((name, value)) = line.split_once(':') {
                    if name.eq_ignore_ascii_case("content-length") {
                        content_length = value.trim().parse::<usize>().unwrap_or(0);
                        break;
                    }
                }
            }

            while read_total < header_end + content_length {
                let read = stream.read(&mut buffer[read_total..]).await?;
                if read == 0 {
                    break;
                }
                read_total += read;
            }

            let body =
                &buffer[header_end..header_end + content_length.min(read_total - header_end)];

            let request_line = headers.lines().next().unwrap_or_default();
            if request_line.starts_with("OPTIONS ") {
                let response = "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nConnection: close\r\n\r\n";
                stream.write_all(response.as_bytes()).await?;
                stream.flush().await?;
                return Ok(());
            }

            if !request_line.starts_with("POST /import-mdvh ") {
                let response = "HTTP/1.1 404 Not Found\r\nAccess-Control-Allow-Origin: *\r\nConnection: close\r\n\r\n";
                stream.write_all(response.as_bytes()).await?;
                stream.flush().await?;
                return Ok(());
            }

            // Parse payload
            let payload: serde_json::Value = serde_json::from_slice(body)?;

            // Save payload to file (relative to current directory)
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default();

            let output_dir = std::path::Path::new("payload-results");
            let _ = tokio::fs::create_dir_all(&output_dir).await;

            let payload_path = output_dir.join(format!("mdvh-payload-{}.json", timestamp));
            let latest_path = output_dir.join("latest-mdvh-payload.json");
            let pretty = serde_json::to_string_pretty(&payload)?;
            let _ = tokio::fs::write(&payload_path, &pretty).await;
            let _ = tokio::fs::write(&latest_path, &pretty).await;

            // Emit success response to extension
            let response_body =
                serde_json::json!({ "ok": true, "path": payload_path.to_string_lossy() })
                    .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response_body.len(),
                response_body
            );
            stream.write_all(response.as_bytes()).await?;
            stream.flush().await?;

            // Trigger download
            trigger_download(app, payload_path).await?;
            return Ok(());
        }

        if read_total == buffer.len() {
            return Err(anyhow::anyhow!("Request too large"));
        }
    }

    Ok(())
}

async fn trigger_download(app: &AppHandle, payload_path: std::path::PathBuf) -> anyhow::Result<()> {
    let app_clone = app.clone();
    tokio::spawn(async move {
        // 1. Parse payload
        let metadata = match parse_workflow_file(&payload_path) {
            Ok(meta) => meta,
            Err(e) => {
                let _ = app_clone.emit("download-error", format!("Failed to parse payload: {}", e));
                return;
            }
        };

        let selected = match metadata.selected_files.first() {
            Some(f) => f.clone(),
            None => {
                let _ = app_clone.emit("download-error", "No selected files found in payload");
                return;
            }
        };

        // Emit download-started to let frontend know a download starts
        let _ = app_clone.emit(
            "download-started",
            serde_json::json!({
                "fileName": selected.file_name,
                "expectedSize": selected.size,
                "serverPath": selected.server_path,
            }),
        );

        // 2. Run the probe and download
        let options = ProbeOptions {
            workflow_json: payload_path,
            output_dir: std::path::PathBuf::from("downloads"),
            host: None,
            port: None,
            timeout: std::time::Duration::from_secs(30),
        };

        // Progress callback to emit progress to frontend
        let app_progress = app_clone.clone();
        let callback: ProgressCallback = Arc::new(move |progress: DownloadProgress| {
            let _ = app_progress.emit("download-progress", progress);
        });
        let progress_callback = Some(callback);

        match run_probe(options, progress_callback).await {
            Ok(report) => {
                let _ = app_clone.emit("download-finished", report);
            }
            Err(e) => {
                let _ = app_clone.emit(
                    "download-failed",
                    serde_json::json!({
                        "fileName": selected.file_name.clone(),
                        "error": format!("Download failed: {}", e)
                    }),
                );
            }
        }
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let handle = app.handle().clone();
            // Start the HTTP payload listener on port 48991
            tokio::spawn(async move {
                start_payload_listener(handle, 48991).await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![open_download_folder])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
