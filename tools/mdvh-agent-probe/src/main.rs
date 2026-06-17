use anyhow::{anyhow, Result};
use clap::Parser;
use mdvh_agent_probe::{
    listen_for_payloads, parse_workflow_file, run_probe, PayloadListenOptions, ProbeOptions,
    EXIT_INVALID_WORKFLOW_JSON,
};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(name = "mdvh-agent-probe")]
#[command(about = "Probe RAON local agent and attempt one MDVH/SSCM binary download")]
struct Args {
    #[arg(long)]
    workflow_json: Option<PathBuf>,

    #[arg(long)]
    output_dir: PathBuf,

    #[arg(long)]
    host: Option<String>,

    #[arg(long)]
    port: Option<u16>,

    #[arg(long, default_value_t = 10)]
    timeout_seconds: u64,

    #[arg(long)]
    parse_only: bool,

    #[arg(long)]
    listen_payload: bool,

    #[arg(long, default_value_t = 48991)]
    listen_port: u16,

    #[arg(long, default_value = "127.0.0.1")]
    listen_host: String,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => ExitCode::from(code as u8),
        Err(error) => {
            eprintln!("{error:#}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<i32> {
    let args = Args::parse();
    if args.listen_payload {
        return listen_for_payloads(PayloadListenOptions {
            bind_host: args.listen_host,
            bind_port: args.listen_port,
            output_dir: args.output_dir,
        })
        .await;
    }

    let workflow_json = args
        .workflow_json
        .clone()
        .ok_or_else(|| anyhow!("--workflow-json is required unless --listen-payload is used"))?;

    if args.parse_only {
        match parse_workflow_file(&workflow_json) {
            Ok(metadata) => {
                println!("{}", serde_json::to_string_pretty(&metadata)?);
                return Ok(0);
            }
            Err(error) => {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "status": "failed",
                        "notes": [format!("invalid workflow JSON: {error:#}")]
                    }))?
                );
                return Ok(EXIT_INVALID_WORKFLOW_JSON);
            }
        }
    }

    let options = ProbeOptions {
        workflow_json,
        output_dir: args.output_dir,
        host: args.host,
        port: args.port,
        timeout: Duration::from_secs(args.timeout_seconds),
        cancellation_token: None,
    };

    let report = match run_probe(options, None).await {
        Ok(report) => report,
        Err(error) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "status": "failed",
                    "notes": [format!("{error:#}")]
                }))?
            );
            return Ok(EXIT_INVALID_WORKFLOW_JSON);
        }
    };
    let exit_code = report.exit_code();
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(exit_code)
}
