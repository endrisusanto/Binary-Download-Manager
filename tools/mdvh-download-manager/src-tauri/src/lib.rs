use std::sync::Arc;

use mdvh_agent_probe::{
    parse_workflow_file, run_direct_download, run_probe, DownloadProgress, ProbeOptions,
    ProgressCallback,
};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

struct ListenerStatusState {
    active: std::sync::atomic::AtomicBool,
    bind: std::sync::Mutex<String>,
    error: std::sync::Mutex<Option<String>>,
}

#[tauri::command]
fn get_listener_status(state: tauri::State<'_, ListenerStatusState>) -> serde_json::Value {
    let active = state.active.load(std::sync::atomic::Ordering::SeqCst);
    let bind = state.bind.lock().unwrap().clone();
    let error = state.error.lock().unwrap().clone();
    serde_json::json!({
        "active": active,
        "bind": bind,
        "error": error
    })
}

struct DownloadConfigState {
    output_dir: std::sync::Mutex<std::path::PathBuf>,
}

struct ActiveDownload {
    temp_payload_path: std::path::PathBuf,
    cancellation_token: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

struct DownloadManagerState {
    active_downloads: std::sync::Mutex<std::collections::HashMap<String, ActiveDownload>>,
}

#[tauri::command]
async fn pick_download_dir(
    app: tauri::AppHandle,
    state: tauri::State<'_, DownloadConfigState>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let folder = app.dialog().file().blocking_pick_folder();
    if let Some(path_buf) = folder {
        let path_str = path_buf.to_string();
        *state.output_dir.lock().unwrap() = std::path::PathBuf::from(&path_str);
        Ok(Some(path_str))
    } else {
        Ok(None)
    }
}

#[tauri::command]
fn get_download_dir(state: tauri::State<'_, DownloadConfigState>) -> String {
    state
        .output_dir
        .lock()
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[tauri::command]
fn pause_download(
    file_name: String,
    state: tauri::State<'_, DownloadManagerState>,
) -> Result<(), String> {
    if let Some(download) = state.active_downloads.lock().unwrap().get(&file_name) {
        download
            .cancellation_token
            .store(true, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    } else {
        Err(format!("Download {} not found", file_name))
    }
}

#[tauri::command]
async fn resume_download(
    app: AppHandle,
    file_name: String,
    state: tauri::State<'_, DownloadManagerState>,
) -> Result<(), String> {
    let temp_payload_path = {
        let downloads = state.active_downloads.lock().unwrap();
        if let Some(download) = downloads.get(&file_name) {
            download.temp_payload_path.clone()
        } else {
            return Err(format!("Download {} not found", file_name));
        }
    };

    // Trigger download using the stored temporary payload JSON
    trigger_download(&app, temp_payload_path)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn get_app_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[tauri::command]
fn open_download_folder(
    app: AppHandle,
    state: tauri::State<'_, DownloadConfigState>,
) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    let path = state.output_dir.lock().unwrap().clone();

    // Create downloads folder if it doesn't exist
    let _ = std::fs::create_dir_all(&path);

    app.opener()
        .open_path(path.to_string_lossy(), None::<&str>)
        .map_err(|e| e.to_string())
}

async fn start_payload_listener(app_handle: AppHandle, port: u16) {
    let bind_addr = format!("127.0.0.1:{}", port);
    let state = app_handle.state::<ListenerStatusState>();
    *state.bind.lock().unwrap() = bind_addr.clone();

    let listener = match TcpListener::bind(&bind_addr).await {
        Ok(l) => {
            state
                .active
                .store(true, std::sync::atomic::Ordering::SeqCst);
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
            state
                .active
                .store(false, std::sync::atomic::Ordering::SeqCst);
            *state.error.lock().unwrap() = Some(e.to_string());
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

        if metadata.selected_files.is_empty() {
            let _ = app_clone.emit("download-error", "No selected files found in payload");
            return;
        }

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or_default();

        let output_dir_base = std::path::Path::new("payload-results");
        let _ = tokio::fs::create_dir_all(&output_dir_base).await;

        for (idx, selected) in metadata.selected_files.into_iter().enumerate() {
            let app_file_clone = app_clone.clone();
            let selected_clone = selected.clone();

            // Build single file payload JSON to pass to run_probe
            let single_payload = serde_json::json!({
                "selectedFiles": [selected_clone],
                "connectedPort": metadata.connected_port,
                "release": metadata.release,
                "cookies": metadata.cookies,
                "pageOrigin": metadata.page_origin,
            });

            let temp_payload_path =
                output_dir_base.join(format!("mdvh-payload-temp-{}-{}.json", timestamp, idx));
            let pretty = match serde_json::to_string_pretty(&single_payload) {
                Ok(p) => p,
                Err(e) => {
                    let _ = app_file_clone.emit(
                        "download-error",
                        format!("Failed to serialize temp payload: {}", e),
                    );
                    continue;
                }
            };
            if let Err(e) = tokio::fs::write(&temp_payload_path, &pretty).await {
                let _ = app_file_clone.emit(
                    "download-error",
                    format!("Failed to write temp payload: {}", e),
                );
                continue;
            }

            // Emit download-started to let frontend know a download starts
            let _ = app_file_clone.emit(
                "download-started",
                serde_json::json!({
                    "fileName": selected.file_name,
                    "expectedSize": selected.size,
                    "serverPath": selected.server_path,
                }),
            );

            // Fetch configured output directory from state
            let output_dir = app_file_clone
                .state::<DownloadConfigState>()
                .output_dir
                .lock()
                .unwrap()
                .clone();

            let cancellation_token = Arc::new(std::sync::atomic::AtomicBool::new(false));

            // Store active download
            {
                let state = app_file_clone.state::<DownloadManagerState>();
                state.active_downloads.lock().unwrap().insert(
                    selected.file_name.clone(),
                    ActiveDownload {
                        temp_payload_path: temp_payload_path.clone(),
                        cancellation_token: cancellation_token.clone(),
                    },
                );
            }

            // Run the probe and download in a background task
            let options = ProbeOptions {
                workflow_json: temp_payload_path.clone(),
                output_dir,
                host: None,
                port: None,
                timeout: std::time::Duration::from_secs(600),
                cancellation_token: Some(cancellation_token),
            };

            // Progress callback to emit progress to frontend
            let app_progress = app_file_clone.clone();
            let callback: ProgressCallback = Arc::new(move |progress: DownloadProgress| {
                let _ = app_progress.emit("download-progress", progress);
            });
            let progress_callback = Some(callback);

            let is_direct = metadata.raonk_flag.as_deref() == Some("N")
                || metadata
                    .release
                    .as_ref()
                    .and_then(|r| r.get("raonkFlag"))
                    .and_then(|rf| rf.as_str())
                    == Some("N");

            println!(
                "[DEBUG BDM] Triggering download for file: {}",
                selected.file_name
            );
            println!("[DEBUG BDM] Metadata raonk_flag: {:?}", metadata.raonk_flag);
            println!(
                "[DEBUG BDM] Metadata release raonkFlag: {:?}",
                metadata.release.as_ref().and_then(|r| r.get("raonkFlag"))
            );
            println!("[DEBUG BDM] Page origin: {:?}", metadata.page_origin);
            println!(
                "[DEBUG BDM] Cookies present: {}",
                metadata.cookies.is_some() && !metadata.cookies.as_ref().unwrap().is_empty()
            );
            println!("[DEBUG BDM] Decision is_direct = {}", is_direct);

            tokio::spawn(async move {
                let result = if is_direct {
                    println!("[DEBUG BDM] Starting direct SSCM download...");
                    run_direct_download(options, progress_callback).await
                } else {
                    println!("[DEBUG BDM] Starting local agent probe...");
                    run_probe(options, progress_callback).await
                };

                println!(
                    "[DEBUG BDM] Result for {}: {:?}",
                    selected_clone.file_name, result
                );

                match result {
                    Ok(report) => {
                        let is_cancelled =
                            matches!(report.status, mdvh_agent_probe::ProbeStatus::Cancelled);
                        let _ = app_file_clone.emit("download-finished", &report);

                        // Only remove payload if not cancelled, because we need it to resume
                        if !is_cancelled {
                            let _ = tokio::fs::remove_file(&temp_payload_path).await;
                            let state = app_file_clone.state::<DownloadManagerState>();
                            state
                                .active_downloads
                                .lock()
                                .unwrap()
                                .remove(&selected.file_name);
                        }
                    }
                    Err(e) => {
                        let _ = app_file_clone.emit(
                            "download-failed",
                            serde_json::json!({
                                "fileName": selected.file_name.clone(),
                                "error": format!("Download failed: {}", e)
                            }),
                        );
                        let _ = tokio::fs::remove_file(&temp_payload_path).await;
                        let state = app_file_clone.state::<DownloadManagerState>();
                        state
                            .active_downloads
                            .lock()
                            .unwrap()
                            .remove(&selected.file_name);
                    }
                }
            });
        }
    });

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "linux")]
    {
        if std::env::var("WEBKIT_DISABLE_DMABUF_RENDERER").is_err() {
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
    }

    let default_output_dir = std::env::current_dir()
        .map(|p| p.join("downloads"))
        .unwrap_or_else(|_| std::path::PathBuf::from("downloads"));

    tauri::Builder::default()
        .manage(ListenerStatusState {
            active: std::sync::atomic::AtomicBool::new(false),
            bind: std::sync::Mutex::new("".to_string()),
            error: std::sync::Mutex::new(None),
        })
        .manage(DownloadConfigState {
            output_dir: std::sync::Mutex::new(default_output_dir),
        })
        .manage(DownloadManagerState {
            active_downloads: std::sync::Mutex::new(std::collections::HashMap::new()),
        })
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();

            // Build Tray Menu items
            let show_i =
                MenuItem::with_id(app, "show", "Show Download Manager", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &quit_i])?;

            // Build Tray Icon safely (prevent unwrapping panics if default icon is missing)
            let mut tray_builder = TrayIconBuilder::new();
            if let Some(icon) = app.default_window_icon() {
                tray_builder = tray_builder.icon(icon.clone());
            }

            let _tray = tray_builder
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: tauri::tray::MouseButton::Left,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            // Start the HTTP payload listener on port 48991
            tauri::async_runtime::spawn(async move {
                start_payload_listener(handle, 48991).await;
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Prevent destroying the window, hide it instead so it runs in background
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            open_download_folder,
            get_listener_status,
            pick_download_dir,
            get_download_dir,
            pause_download,
            resume_download,
            get_app_version
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
