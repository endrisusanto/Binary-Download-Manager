use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use reqwest::{Client, Method, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub const EXIT_DOWNLOADED: i32 = 0;
pub const EXIT_AGENT_REACHABLE_NO_STREAM: i32 = 10;
pub const EXIT_AGENT_UNREACHABLE: i32 = 20;
pub const EXIT_ENDPOINT_DOWNLOAD_FAILED: i32 = 30;
pub const EXIT_INVALID_WORKFLOW_JSON: i32 = 40;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SelectedFile {
    #[serde(default, rename = "checkboxClass")]
    pub checkbox_class: Option<String>,
    #[serde(default, rename = "checkboxId")]
    pub checkbox_id: Option<String>,
    #[serde(default)]
    pub stamp: Option<String>,
    #[serde(rename = "fileName")]
    pub file_name: String,
    #[serde(default, rename = "fileType")]
    pub file_type: Option<String>,
    #[serde(rename = "serverPath")]
    pub server_path: String,
    #[serde(deserialize_with = "deserialize_size")]
    pub size: u64,
    #[serde(default, rename = "binaryId")]
    pub binary_id: Option<String>,
    #[serde(default, rename = "fileId")]
    pub file_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub file_name: String,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
}

pub type ProgressCallback = std::sync::Arc<dyn Fn(DownloadProgress) + Send + Sync + 'static>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowMetadata {
    pub selected_files: Vec<SelectedFile>,
    pub connected_port: Option<u16>,
    #[serde(default, rename = "raonkFlag")]
    pub raonk_flag: Option<String>,
    #[serde(default)]
    pub cookies: Option<String>,
    #[serde(default, rename = "pageOrigin")]
    pub page_origin: Option<String>,
    #[serde(default)]
    pub release: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Downloaded,
    AgentReplayed,
    EndpointFound,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeReport {
    pub status: ProbeStatus,
    #[serde(rename = "fileName")]
    pub file_name: Option<String>,
    #[serde(rename = "serverPath")]
    pub server_path: Option<String>,
    #[serde(rename = "expectedSize")]
    pub expected_size: Option<u64>,
    #[serde(rename = "actualSize")]
    pub actual_size: Option<u64>,
    #[serde(rename = "agentPort")]
    pub agent_port: Option<u16>,
    #[serde(rename = "agentBaseUrl")]
    pub agent_base_url: Option<String>,
    pub endpoint: Option<String>,
    pub notes: Vec<String>,
}

impl ProbeReport {
    pub fn exit_code(&self) -> i32 {
        match self.status {
            ProbeStatus::Downloaded => EXIT_DOWNLOADED,
            ProbeStatus::AgentReplayed => EXIT_AGENT_REACHABLE_NO_STREAM,
            ProbeStatus::EndpointFound => EXIT_ENDPOINT_DOWNLOAD_FAILED,
            ProbeStatus::Cancelled => 50,
            ProbeStatus::Failed => {
                if self
                    .notes
                    .iter()
                    .any(|note| note.contains("agent unreachable"))
                {
                    EXIT_AGENT_UNREACHABLE
                } else {
                    EXIT_ENDPOINT_DOWNLOAD_FAILED
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ProbeOptions {
    pub workflow_json: PathBuf,
    pub output_dir: PathBuf,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub timeout: Duration,
    pub cancellation_token: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

#[derive(Debug, Clone)]
pub struct PayloadListenOptions {
    pub bind_host: String,
    pub bind_port: u16,
    pub output_dir: PathBuf,
}

#[derive(Debug, Clone)]
struct CandidateEndpoint {
    method: Method,
    url: String,
    body: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PayloadListenReport {
    pub status: String,
    pub bind: String,
    #[serde(rename = "outputDir")]
    pub output_dir: String,
    #[serde(rename = "latestPayload")]
    pub latest_payload: String,
}

#[derive(Debug, Clone)]
struct AgentBase {
    url: String,
    port: u16,
}

pub fn parse_workflow_file(path: &Path) -> Result<WorkflowMetadata> {
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read workflow JSON: {}", path.display()))?;
    parse_workflow_json(&contents)
}

pub fn parse_workflow_json(contents: &str) -> Result<WorkflowMetadata> {
    let root: Value = serde_json::from_str(contents).context("workflow JSON is not valid JSON")?;

    if let Some(events) = root.as_array() {
        let mut selected_files = Vec::new();
        let mut ports = BTreeSet::new();

        for event in events {
            let detail = event.get("detail").unwrap_or(&Value::Null);
            if event.get("kind").and_then(Value::as_str) == Some("download-state-snapshot") {
                if let Some(files) = detail.get("selectedFiles").and_then(Value::as_array) {
                    for file in files {
                        selected_files.push(serde_json::from_value::<SelectedFile>(file.clone())?);
                    }
                }
                if let Some(port) = detail
                    .pointer("/raonkGlobals/RAONKSolutionAgent/connectedPort")
                    .and_then(value_to_port)
                {
                    ports.insert(port);
                }
            }
        }

        if selected_files.is_empty() {
            return Err(anyhow!("no selectedFiles found in workflow JSON"));
        }

        Ok(WorkflowMetadata {
            selected_files,
            connected_port: ports.into_iter().next(),
            raonk_flag: None,
            cookies: None,
            page_origin: None,
            release: None,
        })
    } else if let Some(obj) = root.as_object() {
        let files_value = obj
            .get("selectedFiles")
            .ok_or_else(|| anyhow!("payload JSON is an object but lacks 'selectedFiles'"))?;
        let files_array = files_value
            .as_array()
            .ok_or_else(|| anyhow!("'selectedFiles' in payload JSON must be an array"))?;

        let mut selected_files = Vec::new();
        for file in files_array {
            selected_files.push(serde_json::from_value::<SelectedFile>(file.clone())?);
        }

        if selected_files.is_empty() {
            return Err(anyhow!("selectedFiles array is empty"));
        }

        let connected_port = obj
            .get("connectedPort")
            .or_else(|| root.pointer("/raonkGlobals/RAONKSolutionAgent/connectedPort"))
            .and_then(value_to_port);

        let raonk_flag = obj
            .get("release")
            .and_then(|r| r.get("raonkFlag"))
            .and_then(Value::as_str)
            .map(String::from);

        let cookies = obj
            .get("cookies")
            .and_then(Value::as_str)
            .map(String::from);

        let page_origin = obj
            .get("pageOrigin")
            .and_then(Value::as_str)
            .map(String::from);

        let release = obj.get("release").cloned();

        Ok(WorkflowMetadata {
            selected_files,
            connected_port,
            raonk_flag,
            cookies,
            page_origin,
            release,
        })
    } else {
        Err(anyhow!(
            "workflow JSON must be either an array of events or a payload object"
        ))
    }
}

pub async fn run_probe(
    options: ProbeOptions,
    progress_callback: Option<ProgressCallback>,
) -> Result<ProbeReport> {
    let metadata = parse_workflow_file(&options.workflow_json)?;
    let selected = metadata
        .selected_files
        .first()
        .ok_or_else(|| anyhow!("no selected file available after parsing"))?
        .clone();
    let ports = candidate_ports(options.port, metadata.connected_port);

    let client = Client::builder()
        .timeout(options.timeout)
        .danger_accept_invalid_certs(true)
        .build()
        .context("failed to build HTTP client")?;

    let mut notes = Vec::new();
    notes.push(format!(
        "selected file {} from {}",
        selected.file_name, selected.server_path
    ));

    let hosts = candidate_hosts(options.host.as_deref());
    let Some(agent_base) = find_reachable_agent(&client, &hosts, &ports).await else {
        let port_numbers: Vec<u16> = ports.iter().map(|port| port.number).collect();
        notes.push(format!(
            "agent unreachable on candidate hosts {:?} and ports {:?}",
            hosts, port_numbers
        ));
        return Ok(report_for_selected(
            ProbeStatus::Failed,
            &selected,
            None,
            None,
            None,
            None,
            notes,
        ));
    };

    notes.push(format!("agent reachable on {}", agent_base.url));
    tokio::fs::create_dir_all(&options.output_dir)
        .await
        .with_context(|| format!("failed to create {}", options.output_dir.display()))?;

    let candidates = build_candidate_endpoints(&agent_base.url, &selected);
    let mut reachable_endpoint = None;
    for candidate in candidates {
        match try_candidate(
            &client,
            &candidate,
            &options.output_dir,
            &selected,
            &progress_callback,
            &options.cancellation_token,
        )
        .await
        {
            Ok(CandidateOutcome::Downloaded { path, bytes }) => {
                notes.push(format!("downloaded {} bytes to {}", bytes, path.display()));
                if bytes == selected.size {
                    notes.push("actual size matches expected size".to_string());
                    return Ok(report_for_selected(
                        ProbeStatus::Downloaded,
                        &selected,
                        Some(bytes),
                        Some(agent_base.port),
                        Some(agent_base.url),
                        Some(candidate.url),
                        notes,
                    ));
                }
                notes.push(format!(
                    "downloaded size mismatch: expected {}, actual {}",
                    selected.size, bytes
                ));
                return Ok(report_for_selected(
                    ProbeStatus::EndpointFound,
                    &selected,
                    Some(bytes),
                    Some(agent_base.port),
                    Some(agent_base.url),
                    Some(candidate.url),
                    notes,
                ));
            }
            Ok(CandidateOutcome::Replayed { status }) => {
                reachable_endpoint.get_or_insert(candidate.url.clone());
                notes.push(format!(
                    "{} returned HTTP {}",
                    candidate.url,
                    status.as_u16()
                ));
            }
            Ok(CandidateOutcome::Ignored { reason }) => notes.push(reason),
            Ok(CandidateOutcome::Cancelled) => {
                notes.push("download cancelled by user".to_string());
                return Ok(report_for_selected(
                    ProbeStatus::Cancelled,
                    &selected,
                    None,
                    Some(agent_base.port),
                    Some(agent_base.url),
                    Some(candidate.url),
                    notes,
                ));
            }
            Err(error) => notes.push(format!("{} failed: {error:#}", candidate.url)),
        }
    }

    Ok(report_for_selected(
        if reachable_endpoint.is_some() {
            ProbeStatus::AgentReplayed
        } else {
            ProbeStatus::Failed
        },
        &selected,
        None,
        Some(agent_base.port),
        Some(agent_base.url),
        reachable_endpoint,
        notes,
    ))
}

/// Direct download from SSCM server for raonkFlag=N files.
/// Uses captured browser cookies for authentication.
pub async fn run_direct_download(
    options: ProbeOptions,
    progress_callback: Option<ProgressCallback>,
) -> Result<ProbeReport> {
    let metadata = parse_workflow_file(&options.workflow_json)?;
    let selected = metadata
        .selected_files
        .first()
        .ok_or_else(|| anyhow!("no selected file available after parsing"))?
        .clone();

    let mut notes = Vec::new();
    notes.push(format!(
        "direct download for {} from {}",
        selected.file_name, selected.server_path
    ));

    let cookies = metadata.cookies.unwrap_or_default();
    let page_origin = metadata
        .page_origin
        .unwrap_or_else(|| "http://mdvh.sec.samsung.net".to_string());

    if cookies.is_empty() {
        notes.push("no browser cookies captured, authentication may fail".to_string());
    }

    let client = Client::builder()
        .timeout(options.timeout)
        .danger_accept_invalid_certs(true)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .context("failed to build HTTP client")?;

    tokio::fs::create_dir_all(&options.output_dir)
        .await
        .with_context(|| format!("failed to create {}", options.output_dir.display()))?;

    // Build candidate SSCM download URLs
    let release = metadata.release.unwrap_or(json!({}));
    let sscm_urls = build_sscm_download_urls(&page_origin, &selected, &release);

    notes.push(format!("trying {} SSCM download URLs", sscm_urls.len()));

    for sscm_url in &sscm_urls {
        notes.push(format!("trying URL: {}", sscm_url.url));

        let output_path = options.output_dir.join(&selected.file_name);
        let mut start_bytes = 0u64;

        if let Ok(meta) = tokio::fs::metadata(&output_path).await {
            if meta.is_file() {
                start_bytes = meta.len();
                if start_bytes == selected.size {
                    notes.push("file already fully downloaded".to_string());
                    return Ok(report_for_selected(
                        ProbeStatus::Downloaded,
                        &selected,
                        Some(start_bytes),
                        None,
                        None,
                        Some(sscm_url.url.clone()),
                        notes,
                    ));
                }
            }
        }

        let mut request = if sscm_url.is_post {
            let form_body = sscm_url.form_data.as_deref().unwrap_or("");
            client
                .post(&sscm_url.url)
                .header("Content-Type", "application/x-www-form-urlencoded")
                .body(form_body.to_string())
        } else {
            client.get(&sscm_url.url)
        };

        request = request
            .header("Cookie", &cookies)
            .header("Referer", &page_origin);

        if start_bytes > 0 {
            request = request.header("Range", format!("bytes={}-", start_bytes));
        }

        let response = match request.send().await {
            Ok(r) => r,
            Err(e) => {
                notes.push(format!("{} failed: {}", sscm_url.url, e));
                continue;
            }
        };

        let status = response.status();
        if !status.is_success() && status != StatusCode::PARTIAL_CONTENT {
            notes.push(format!("{} returned HTTP {}", sscm_url.url, status.as_u16()));
            continue;
        }

        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let content_length = response.content_length();

        // Check if this looks like a file response or just an HTML redirect/error
        let is_file = looks_like_file_response(&content_type, content_length, selected.size)
            || status == StatusCode::PARTIAL_CONTENT
            || content_type.contains("application/force-download")
            || content_type.contains("application/download");

        if !is_file {
            notes.push(format!(
                "{} returned content-type: {}, length: {:?} — not a file",
                sscm_url.url, content_type, content_length
            ));
            continue;
        }

        let is_partial = status == StatusCode::PARTIAL_CONTENT;
        if !is_partial {
            start_bytes = 0;
        }

        let bytes = save_response_body(
            response,
            &output_path,
            selected.file_name.clone(),
            selected.size,
            start_bytes,
            &progress_callback,
            &options.cancellation_token,
        )
        .await?;

        if let Some(token) = &options.cancellation_token {
            if token.load(std::sync::atomic::Ordering::Relaxed) {
                notes.push("download cancelled by user".to_string());
                return Ok(report_for_selected(
                    ProbeStatus::Cancelled,
                    &selected,
                    Some(start_bytes + bytes),
                    None,
                    None,
                    Some(sscm_url.url.clone()),
                    notes,
                ));
            }
        }

        let total_bytes = start_bytes + bytes;
        notes.push(format!("downloaded {} bytes to {}", total_bytes, output_path.display()));

        if total_bytes == selected.size {
            notes.push("actual size matches expected size".to_string());
        } else {
            notes.push(format!(
                "size mismatch: expected {}, actual {}",
                selected.size, total_bytes
            ));
        }

        return Ok(report_for_selected(
            ProbeStatus::Downloaded,
            &selected,
            Some(total_bytes),
            None,
            None,
            Some(sscm_url.url.clone()),
            notes,
        ));
    }

    notes.push("all SSCM download URLs failed".to_string());
    Ok(report_for_selected(
        ProbeStatus::Failed,
        &selected,
        None,
        None,
        None,
        None,
        notes,
    ))
}

#[derive(Debug, Clone)]
struct SscmDownloadUrl {
    url: String,
    is_post: bool,
    form_data: Option<String>,
}

fn build_sscm_download_urls(
    page_origin: &str,
    selected: &SelectedFile,
    release: &Value,
) -> Vec<SscmDownloadUrl> {
    let mut urls = Vec::new();

    // Build form data from release fields and selected file
    let mut form_parts: Vec<String> = Vec::new();
    if let Some(obj) = release.as_object() {
        for (key, value) in obj {
            if key == "raonkFlag" {
                continue;
            }
            if let Some(v) = value.as_str() {
                form_parts.push(format!(
                    "{}={}",
                    urlencoding::encode(key),
                    urlencoding::encode(v)
                ));
            }
        }
    }

    // Add selected file params
    form_parts.push(format!(
        "selectFile={}",
        urlencoding::encode("on")
    ));
    if let Some(stamp) = &selected.stamp {
        let meta = format!(
            "{}*{}*{}*{}*{}",
            stamp,
            selected.file_name,
            selected.file_type.as_deref().unwrap_or(""),
            selected.server_path,
            selected.size
        );
        form_parts.push(format!(
            "selectFileMeta={}",
            urlencoding::encode(&meta)
        ));
    }
    if let Some(binary_id) = &selected.binary_id {
        form_parts.push(format!(
            "selectFileBinaryId={}",
            urlencoding::encode(binary_id)
        ));
    }
    if let Some(file_id) = &selected.file_id {
        form_parts.push(format!(
            "selectFileId={}",
            urlencoding::encode(file_id)
        ));
    }

    let form_data = form_parts.join("&");

    // Primary: SSCM download endpoint via form POST
    urls.push(SscmDownloadUrl {
        url: format!("{}/sscm/appm/srbin/pjt/srBinaryFileDownload.do", page_origin),
        is_post: true,
        form_data: Some(form_data.clone()),
    });

    // Fallback: common alternative paths
    urls.push(SscmDownloadUrl {
        url: format!(
            "{}/sscm/appm/srbin/pjt/srBinaryReleaseFileDownload.do",
            page_origin
        ),
        is_post: true,
        form_data: Some(form_data.clone()),
    });

    // Fallback: direct GET with query params
    urls.push(SscmDownloadUrl {
        url: format!(
            "{}/sscm/appm/srbin/pjt/srBinaryFileDownload.do?{}",
            page_origin, form_data
        ),
        is_post: false,
        form_data: None,
    });

    urls
}

pub async fn listen_for_payloads(options: PayloadListenOptions) -> Result<i32> {
    tokio::fs::create_dir_all(&options.output_dir)
        .await
        .with_context(|| format!("failed to create {}", options.output_dir.display()))?;
    let bind = format!("{}:{}", options.bind_host, options.bind_port);
    let listener = TcpListener::bind(&bind)
        .await
        .with_context(|| format!("failed to bind payload listener on {bind}"))?;
    let latest_payload = options.output_dir.join("latest-mdvh-payload.json");
    let report = PayloadListenReport {
        status: "listening".to_string(),
        bind: bind.clone(),
        output_dir: options.output_dir.display().to_string(),
        latest_payload: latest_payload.display().to_string(),
    };
    println!("{}", serde_json::to_string_pretty(&report)?);

    loop {
        let (mut stream, peer) = listener.accept().await?;
        let output_dir = options.output_dir.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_payload_connection(&mut stream, &output_dir).await {
                let _ = write_http_response(
                    &mut stream,
                    "500 Internal Server Error",
                    "application/json",
                    &json!({ "ok": false, "error": format!("{error:#}") }).to_string(),
                )
                .await;
                eprintln!("payload connection from {peer} failed: {error:#}");
            }
        });
    }
}

async fn handle_payload_connection(
    stream: &mut tokio::net::TcpStream,
    output_dir: &Path,
) -> Result<()> {
    let mut buffer = vec![0u8; 1024 * 1024];
    let mut read_total = 0usize;
    loop {
        let read = stream.read(&mut buffer[read_total..]).await?;
        if read == 0 {
            break;
        }
        read_total += read;
        if read_total >= 4
            && buffer[..read_total]
                .windows(4)
                .any(|window| window == b"\r\n\r\n")
        {
            let header_end = buffer[..read_total]
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|position| position + 4)
                .ok_or_else(|| anyhow!("request header terminator missing"))?;
            let headers = String::from_utf8_lossy(&buffer[..header_end]).into_owned();
            let content_length = parse_content_length(&headers)?;
            while read_total < header_end + content_length {
                let read = stream.read(&mut buffer[read_total..]).await?;
                if read == 0 {
                    break;
                }
                read_total += read;
            }
            let body =
                &buffer[header_end..header_end + content_length.min(read_total - header_end)];
            return handle_http_payload(stream, output_dir, &headers, body).await;
        }
        if read_total == buffer.len() {
            return Err(anyhow!("request too large"));
        }
    }
    Err(anyhow!("empty request"))
}

async fn handle_http_payload(
    stream: &mut tokio::net::TcpStream,
    output_dir: &Path,
    headers: &str,
    body: &[u8],
) -> Result<()> {
    let request_line = headers.lines().next().unwrap_or_default();
    if request_line.starts_with("OPTIONS ") {
        return write_http_response(stream, "204 No Content", "text/plain", "").await;
    }
    if !request_line.starts_with("POST /import-mdvh ") {
        return write_http_response(
            stream,
            "404 Not Found",
            "application/json",
            &json!({ "ok": false, "error": "expected POST /import-mdvh" }).to_string(),
        )
        .await;
    }

    let payload: Value = serde_json::from_slice(body).context("payload body is not valid JSON")?;
    let selected_count = payload
        .get("selectedFiles")
        .and_then(Value::as_array)
        .map(|files| files.len())
        .unwrap_or(0);
    let timestamp = timestamp_for_filename();
    let payload_path = output_dir.join(format!("mdvh-payload-{timestamp}.json"));
    let latest_path = output_dir.join("latest-mdvh-payload.json");
    let pretty = serde_json::to_string_pretty(&payload)?;
    tokio::fs::write(&payload_path, &pretty).await?;
    tokio::fs::write(&latest_path, &pretty).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "status": "payload_received",
            "selectedFiles": selected_count,
            "path": payload_path,
            "latest": latest_path,
        }))?
    );

    write_http_response(
        stream,
        "200 OK",
        "application/json",
        &json!({
            "ok": true,
            "selectedFiles": selected_count,
            "path": payload_path,
        })
        .to_string(),
    )
    .await
}

fn parse_content_length(headers: &str) -> Result<usize> {
    for line in headers.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            return value
                .trim()
                .parse::<usize>()
                .context("invalid Content-Length");
        }
    }
    Ok(0)
}

async fn write_http_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    content_type: &str,
    body: &str,
) -> Result<()> {
    let response = format!(
        "HTTP/1.1 {status}\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

fn timestamp_for_filename() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    millis.to_string()
}

fn report_for_selected(
    status: ProbeStatus,
    selected: &SelectedFile,
    actual_size: Option<u64>,
    agent_port: Option<u16>,
    agent_base_url: Option<String>,
    endpoint: Option<String>,
    notes: Vec<String>,
) -> ProbeReport {
    ProbeReport {
        status,
        file_name: Some(selected.file_name.clone()),
        server_path: Some(selected.server_path.clone()),
        expected_size: Some(selected.size),
        actual_size,
        agent_port,
        agent_base_url,
        endpoint,
        notes,
    }
}

fn candidate_hosts(manual: Option<&str>) -> Vec<String> {
    let mut hosts = Vec::new();
    if let Some(host) = manual {
        let trimmed = host.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            hosts.push(trimmed.to_string());
        }
    }
    for host in [
        "http://127.0.0.1",
        "http://localhost",
        "https://127.0.0.1",
        "https://localhost",
    ] {
        if !hosts.iter().any(|value| value == host) {
            hosts.push(host.to_string());
        }
    }
    hosts
}

#[derive(Debug, Clone, Copy)]
struct CandidatePort {
    number: u16,
    trusted: bool,
}

fn candidate_ports(manual: Option<u16>, captured: Option<u16>) -> Vec<CandidatePort> {
    let mut trusted = BTreeSet::new();
    if let Some(port) = manual {
        trusted.insert(port);
    }
    if let Some(port) = captured {
        trusted.insert(port);
    }
    let mut all = trusted.clone();
    for port in [47317, 47318, 47319, 47320, 47321, 47322, 47323, 47324] {
        all.insert(port);
    }
    all.into_iter()
        .map(|number| CandidatePort {
            number,
            trusted: trusted.contains(&number),
        })
        .collect()
}

async fn find_reachable_agent(
    client: &Client,
    hosts: &[String],
    ports: &[CandidatePort],
) -> Option<AgentBase> {
    // Build a fast client for local agent discovery to avoid long timeouts on closed ports
    let discovery_client = Client::builder()
        .timeout(Duration::from_millis(500))
        .connect_timeout(Duration::from_millis(200))
        .danger_accept_invalid_certs(true)
        .build()
        .unwrap_or_else(|_| client.clone());

    for port in ports {
        for host in hosts {
            let base = format!("{host}:{}", port.number);
            let endpoints = [
                "/",
                "/version",
                "/kversion",
                "/raonk/version",
                "/kupload/version",
            ];
            for endpoint in endpoints {
                let url = format!("{base}{endpoint}");
                let Ok(response) = discovery_client.get(&url).send().await else {
                    continue;
                };
                if port.trusted || response_has_raon_fingerprint(response).await {
                    return Some(AgentBase {
                        url: base,
                        port: port.number,
                    });
                }
            }
        }
    }
    None
}

async fn response_has_raon_fingerprint(response: Response) -> bool {
    let headers = response.headers().clone();
    if headers.iter().any(|(name, value)| {
        let text = format!("{}:{}", name.as_str(), value.to_str().unwrap_or_default());
        text.to_ascii_lowercase().contains("raon") || text.to_ascii_lowercase().contains("kupload")
    }) {
        return true;
    }
    match response.text().await {
        Ok(text) => {
            let sample = text
                .chars()
                .take(4096)
                .collect::<String>()
                .to_ascii_lowercase();
            sample.contains("raon") || sample.contains("kupload") || sample.contains("k upload")
        }
        Err(_) => false,
    }
}

fn build_candidate_endpoints(base: &str, selected: &SelectedFile) -> Vec<CandidateEndpoint> {
    let payload = json!({
        "id": "kupload",
        "cmd": "downloadAll",
        "strCmd": "downloadAll",
        "strIsWebFile": "1",
        "strKey": 0,
        "strIsLargeFile": "0",
        "strIsLast": "1",
        "strName": selected.file_name,
        "strPath": selected.server_path,
        "fileName": selected.file_name,
        "serverPath": selected.server_path,
        "size": selected.size,
        "binaryId": selected.binary_id,
        "fileId": selected.file_id,
    });
    let query = format!(
        "strName={}&strPath={}&size={}",
        urlencoding::encode(&selected.file_name),
        urlencoding::encode(&selected.server_path),
        selected.size
    );

    [
        ("POST", "/download", Some(payload.clone())),
        ("POST", "/raonk/download", Some(payload.clone())),
        ("POST", "/raonkupload/download", Some(payload.clone())),
        ("POST", "/kupload/download", Some(payload.clone())),
        ("POST", "/manager/download", Some(payload.clone())),
        ("POST", "/agent/download", Some(payload.clone())),
        ("POST", "/raonkhandler", Some(payload.clone())),
        ("POST", "/handler", Some(payload.clone())),
        ("GET", &format!("/download?{query}"), None),
        ("GET", &format!("/raonk/download?{query}"), None),
        ("GET", &format!("/kupload/download?{query}"), None),
    ]
    .into_iter()
    .map(|(method, path, body)| CandidateEndpoint {
        method: Method::from_bytes(method.as_bytes()).expect("static method is valid"),
        url: format!("{base}{path}"),
        body,
    })
    .collect()
}

enum CandidateOutcome {
    Downloaded { path: PathBuf, bytes: u64 },
    Replayed { status: StatusCode },
    Ignored { reason: String },
    Cancelled,
}

async fn try_candidate(
    client: &Client,
    candidate: &CandidateEndpoint,
    output_dir: &Path,
    selected: &SelectedFile,
    progress_callback: &Option<ProgressCallback>,
    cancellation_token: &Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<CandidateOutcome> {
    let mut request = client.request(candidate.method.clone(), &candidate.url);
    if let Some(body) = &candidate.body {
        request = request.json(body);
    }

    let output_path = output_dir.join(&selected.file_name);
    let mut start_bytes = 0u64;

    if let Ok(metadata) = tokio::fs::metadata(&output_path).await {
        if metadata.is_file() {
            start_bytes = metadata.len();
            if start_bytes > 0 && start_bytes < selected.size {
                request = request.header("Range", format!("bytes={}-", start_bytes));
            } else if start_bytes == selected.size {
                // Already fully downloaded
                return Ok(CandidateOutcome::Downloaded {
                    path: output_path,
                    bytes: start_bytes,
                });
            }
        }
    }

    let response = request.send().await?;
    let status = response.status();
    if !status.is_success() {
        return Ok(CandidateOutcome::Ignored {
            reason: format!("{} returned HTTP {}", candidate.url, status.as_u16()),
        });
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let content_length = response.content_length();
    if looks_like_file_response(&content_type, content_length, selected.size) || status == StatusCode::PARTIAL_CONTENT {
        let is_partial = status == StatusCode::PARTIAL_CONTENT;
        if !is_partial {
            start_bytes = 0; // The server ignored the Range header
        }

        let bytes = save_response_body(
            response,
            &output_path,
            selected.file_name.clone(),
            selected.size,
            start_bytes,
            progress_callback,
            cancellation_token,
        )
        .await?;

        if let Some(token) = cancellation_token {
            if token.load(std::sync::atomic::Ordering::Relaxed) {
                return Ok(CandidateOutcome::Cancelled);
            }
        }

        return Ok(CandidateOutcome::Downloaded {
            path: output_path,
            bytes: start_bytes + bytes,
        });
    }

    Ok(CandidateOutcome::Replayed { status })
}

fn looks_like_file_response(
    content_type: &str,
    content_length: Option<u64>,
    expected_size: u64,
) -> bool {
    if let Some(length) = content_length {
        if length == expected_size || length > 1024 * 1024 {
            return true;
        }
    }
    content_type.contains("application/octet-stream")
        || content_type.contains("application/x-tar")
        || content_type.contains("binary")
}

async fn save_response_body(
    response: reqwest::Response,
    output_path: &Path,
    file_name: String,
    total_bytes: u64,
    start_bytes: u64,
    progress_callback: &Option<ProgressCallback>,
    cancellation_token: &Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<u64> {
    let mut options = tokio::fs::OpenOptions::new();
    options.create(true).write(true);
    if start_bytes > 0 {
        options.append(true);
    } else {
        options.truncate(true);
    }

    let mut file = options.open(output_path)
        .await
        .with_context(|| format!("failed to create or open {}", output_path.display()))?;

    let mut stream = response.bytes_stream();
    let mut bytes_downloaded = 0u64;

    while let Some(chunk) = stream.next().await {
        if let Some(token) = cancellation_token {
            if token.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
        }

        let chunk = chunk?;
        file.write_all(&chunk).await?;
        bytes_downloaded += chunk.len() as u64;

        if let Some(callback) = progress_callback {
            callback(DownloadProgress {
                file_name: file_name.clone(),
                bytes_downloaded: start_bytes + bytes_downloaded,
                total_bytes,
            });
        }
    }
    file.flush().await?;
    Ok(bytes_downloaded)
}

fn deserialize_size<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Number(number) => number
            .as_u64()
            .ok_or_else(|| serde::de::Error::custom("size must be unsigned integer")),
        Value::String(text) => text
            .trim()
            .parse::<u64>()
            .map_err(|_| serde::de::Error::custom("size string must be unsigned integer")),
        _ => Err(serde::de::Error::custom("size must be number or string")),
    }
}

fn value_to_port(value: &Value) -> Option<u16> {
    match value {
        Value::Number(number) => number.as_u64().and_then(|port| u16::try_from(port).ok()),
        Value::String(text) => text.trim().parse::<u16>().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workflow_with_detail(detail: Value) -> String {
        json!([
            {
                "kind": "download-state-snapshot",
                "detail": detail
            }
        ])
        .to_string()
    }

    #[test]
    fn parses_selected_file_and_port() {
        let json = workflow_with_detail(json!({
            "selectedFiles": [{
                "fileName": "CP_A146.tar.md5",
                "serverPath": "F:/SSCM_FILE/file.qb",
                "size": "38441070",
                "binaryId": "BIN",
                "fileId": "FILE"
            }],
            "raonkGlobals": {
                "RAONKSolutionAgent": {
                    "connectedPort": "47317"
                }
            }
        }));
        let parsed = parse_workflow_json(&json).unwrap();
        assert_eq!(parsed.connected_port, Some(47317));
        assert_eq!(parsed.selected_files[0].file_name, "CP_A146.tar.md5");
        assert_eq!(parsed.selected_files[0].size, 38441070);
    }

    #[test]
    fn parses_missing_port() {
        let json = workflow_with_detail(json!({
            "selectedFiles": [{
                "fileName": "file.tar.md5",
                "serverPath": "F:/SSCM_FILE/file.qb",
                "size": 42
            }]
        }));
        let parsed = parse_workflow_json(&json).unwrap();
        assert_eq!(parsed.connected_port, None);
        assert_eq!(parsed.selected_files.len(), 1);
    }

    #[test]
    fn parses_multiple_selected_files() {
        let json = workflow_with_detail(json!({
            "selectedFiles": [
                { "fileName": "a.tar.md5", "serverPath": "F:/a.qb", "size": "1" },
                { "fileName": "b.tar.md5", "serverPath": "F:/b.qb", "size": "2" }
            ]
        }));
        let parsed = parse_workflow_json(&json).unwrap();
        assert_eq!(parsed.selected_files.len(), 2);
        assert_eq!(parsed.selected_files[1].file_name, "b.tar.md5");
    }

    #[test]
    fn rejects_malformed_size() {
        let json = workflow_with_detail(json!({
            "selectedFiles": [{
                "fileName": "file.tar.md5",
                "serverPath": "F:/SSCM_FILE/file.qb",
                "size": "nope"
            }]
        }));
        assert!(parse_workflow_json(&json).is_err());
    }

    #[test]
    fn rejects_missing_selected_files() {
        let json = workflow_with_detail(json!({ "selectedFiles": [] }));
        assert!(parse_workflow_json(&json).is_err());
    }

    #[test]
    fn parses_direct_payload_object() {
        let json = json!({
            "kind": "mdvh-selected-binaries",
            "selectedFiles": [{
                "fileName": "CP_A146.tar.md5",
                "serverPath": "F:/SSCM_FILE/file.qb",
                "size": "38441070",
                "binaryId": "BIN",
                "fileId": "FILE"
            }]
        })
        .to_string();
        let parsed = parse_workflow_json(&json).unwrap();
        assert_eq!(parsed.connected_port, None);
        assert_eq!(parsed.selected_files[0].file_name, "CP_A146.tar.md5");
        assert_eq!(parsed.selected_files[0].size, 38441070);
    }

    #[tokio::test]
    async fn test_progress_callback_during_download() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buffer = [0u8; 1024];
                let _ = tokio::io::AsyncReadExt::read(&mut stream, &mut buffer).await;
                let body = vec![b'A'; 100];
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, response.as_bytes()).await;
                let _ = tokio::io::AsyncWriteExt::write_all(&mut stream, &body).await;
            }
        });

        let client = reqwest::Client::new();
        let res = client.get(format!("http://{}", addr)).send().await.unwrap();

        let temp_dir = tempfile::tempdir().unwrap();
        let output_path = temp_dir.path().join("test_file.bin");

        let progress_updates = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let progress_updates_clone = progress_updates.clone();

        let progress_callback: Option<ProgressCallback> =
            Some(std::sync::Arc::new(move |progress| {
                progress_updates_clone.lock().unwrap().push(progress);
            }));

        let bytes = save_response_body(
            res,
            &output_path,
            "test_file.bin".to_string(),
            100,
            0,
            &progress_callback,
            &None,
        )
        .await
        .unwrap();
        assert_eq!(bytes, 100);

        let updates = progress_updates.lock().unwrap();
        assert!(!updates.is_empty());
        assert_eq!(updates.last().unwrap().bytes_downloaded, 100);
        assert_eq!(updates.last().unwrap().total_bytes, 100);
        assert_eq!(updates.last().unwrap().file_name, "test_file.bin");
    }
}
