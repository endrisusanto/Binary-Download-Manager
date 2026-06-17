use anyhow::{anyhow, Context, Result};
use futures::StreamExt;
use reqwest::{Client, Method, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;

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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkflowMetadata {
    pub selected_files: Vec<SelectedFile>,
    pub connected_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeStatus {
    Downloaded,
    AgentReplayed,
    EndpointFound,
    Failed,
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
}

#[derive(Debug, Clone)]
struct CandidateEndpoint {
    method: Method,
    url: String,
    body: Option<Value>,
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
    let events = root
        .as_array()
        .ok_or_else(|| anyhow!("workflow JSON must be an array of events"))?;

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
    })
}

pub async fn run_probe(options: ProbeOptions) -> Result<ProbeReport> {
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
        match try_candidate(&client, &candidate, &options.output_dir, &selected).await {
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
                let Ok(response) = client.get(&url).send().await else {
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
}

async fn try_candidate(
    client: &Client,
    candidate: &CandidateEndpoint,
    output_dir: &Path,
    selected: &SelectedFile,
) -> Result<CandidateOutcome> {
    let mut request = client.request(candidate.method.clone(), &candidate.url);
    if let Some(body) = &candidate.body {
        request = request.json(body);
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
    if looks_like_file_response(&content_type, content_length, selected.size) {
        let output_path = output_dir.join(&selected.file_name);
        let bytes = save_response_body(response, &output_path).await?;
        return Ok(CandidateOutcome::Downloaded {
            path: output_path,
            bytes,
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

async fn save_response_body(response: reqwest::Response, output_path: &Path) -> Result<u64> {
    let mut file = tokio::fs::File::create(output_path)
        .await
        .with_context(|| format!("failed to create {}", output_path.display()))?;
    let mut stream = response.bytes_stream();
    let mut total = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        file.write_all(&chunk).await?;
        total += chunk.len() as u64;
    }
    file.flush().await?;
    Ok(total)
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
}
