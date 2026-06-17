use anyhow::Result;
use clap::Parser;
use mdvh_agent_probe::{parse_workflow_file, run_probe, ProbeOptions, EXIT_INVALID_WORKFLOW_JSON};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(name = "mdvh-agent-probe")]
#[command(about = "Probe RAON local agent and attempt one MDVH/SSCM binary download")]
struct Args {
    #[arg(long)]
    workflow_json: PathBuf,

    #[arg(long)]
    output_dir: PathBuf,

    #[arg(long)]
    port: Option<u16>,

    #[arg(long, default_value_t = 10)]
    timeout_seconds: u64,

    #[arg(long)]
    parse_only: bool,
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
    if args.parse_only {
        match parse_workflow_file(&args.workflow_json) {
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
        workflow_json: args.workflow_json,
        output_dir: args.output_dir,
        port: args.port,
        timeout: Duration::from_secs(args.timeout_seconds),
    };

    let report = match run_probe(options).await {
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
