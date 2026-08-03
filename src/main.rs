mod bandwidth;
mod impair;
mod ping;
mod ports;
mod report;
mod stress;

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use report::{ReportData, Thresholds};
use std::time::Duration;

#[derive(Debug, Parser)]
#[command(
    name = "netburn",
    about = "Teams-focused network performance and stress testing for macOS"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Run(RunArgs),
    Ping(OutputArgs),
    Ports(OutputArgs),
    Bandwidth(OutputArgs),
    Stress(OutputArgs),
    Impair(ImpairArgs),
}
#[derive(Debug, Args)]
struct OutputArgs {
    #[arg(long)]
    json: bool,
}
#[derive(Debug, Args)]
struct RunArgs {
    #[arg(long, default_value = "netburn-report.html")]
    report: String,
    #[arg(long)]
    json: bool,
    #[arg(long, default_value_t = 10.0)]
    min_bandwidth: f64,
}
#[derive(Debug, Args)]
struct ImpairArgs {
    profile: String,
    #[arg(long)]
    delay: Option<u32>,
    #[arg(long)]
    loss: Option<f64>,
    #[arg(long)]
    bw: Option<u32>,
    #[arg(long)]
    dry_run: bool,
}

fn print_json<T: serde::Serialize>(value: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Spinner for a long-running stage. Draws to stderr, so --json stdout stays clean.
fn stage(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.green} {msg} [{elapsed_precise}]")
            .unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ "),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(Duration::from_millis(80));
    pb
}

fn ping_all(count: u32) -> Result<Vec<ping::PingResult>> {
    let targets = ping::default_targets();
    let pb = stage("Ping");
    let mut results = Vec::new();
    for target in &targets {
        pb.set_message(format!("Ping {target}"));
        results.push(ping::ping(target, count)?);
    }
    pb.finish_with_message(format!("Ping: {} targets", targets.len()));
    Ok(results)
}

fn ports_stage() -> ports::PortResults {
    let pb = stage("Checking Teams ports");
    let results = ports::run();
    pb.finish_with_message(format!(
        "Ports: TCP {}/{}, UDP {}/{}",
        results.tcp.iter().filter(|c| c.ok).count(),
        results.tcp.len(),
        results.udp.iter().filter(|c| c.ok).count(),
        results.udp.len()
    ));
    results
}

fn bandwidth_stage() -> Result<bandwidth::BandwidthResult> {
    let pb = stage("Measuring bandwidth");
    let results = bandwidth::run()?;
    pb.finish_with_message(format!(
        "Bandwidth: ↓{:.2} ↑{:.2} Mbps",
        results.download_mbps.unwrap_or(0.0),
        results.upload_mbps.unwrap_or(0.0)
    ));
    Ok(results)
}

fn stress_stage() -> Result<stress::StressResult> {
    let pb = stage("Stress test (saturating link)");
    let results = stress::run()?;
    pb.finish_with_message(format!(
        "Stress: idle {:.1} ms → loaded {:.1} ms ({})",
        results.idle_ms.unwrap_or(0.0),
        results.loaded_ms.unwrap_or(0.0),
        results.grade
    ));
    Ok(results)
}

fn run_ping(json: bool) -> Result<()> {
    let results = ping_all(20)?;
    if json {
        print_json(&results)?;
    } else {
        for result in &results {
            println!(
                "{}: avg {:.2} ms, jitter {:.2} ms, loss {:.1}%",
                result.target,
                result.avg_ms.unwrap_or(0.0),
                result.stddev_ms.unwrap_or(0.0),
                result.loss_percent
            );
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Run(RunArgs {
        report: "netburn-report.html".into(),
        json: false,
        min_bandwidth: 10.0,
    })) {
        Command::Ping(args) => {
            run_ping(args.json)?;
        }
        Command::Ports(args) => {
            let results = ports_stage();
            if args.json {
                print_json(&results)?;
            } else {
                println!(
                    "Endpoint source: {}\nTCP checks: {}/{} passed\nUDP checks: {}/{} passed",
                    results.endpoint_source,
                    results.tcp.iter().filter(|c| c.ok).count(),
                    results.tcp.len(),
                    results.udp.iter().filter(|c| c.ok).count(),
                    results.udp.len()
                );
            }
        }
        Command::Bandwidth(args) => {
            let results = bandwidth_stage()?;
            if args.json {
                print_json(&results)?;
            } else {
                println!(
                    "Download: {:.2} Mbps\nUpload: {:.2} Mbps",
                    results.download_mbps.unwrap_or(0.0),
                    results.upload_mbps.unwrap_or(0.0)
                );
            }
        }
        Command::Stress(args) => {
            let results = stress_stage()?;
            if args.json {
                print_json(&results)?;
            } else {
                println!(
                    "Idle: {:.2} ms\nLoaded: {:.2} ms\nDelta: {:.2} ms\nGrade: {}",
                    results.idle_ms.unwrap_or(0.0),
                    results.loaded_ms.unwrap_or(0.0),
                    results.delta_ms.unwrap_or(0.0),
                    results.grade
                );
            }
        }
        Command::Impair(args) => {
            if args.profile == "off" {
                impair::off(args.dry_run)?;
            } else {
                impair::apply(&args.profile, args.delay, args.loss, args.bw, args.dry_run)?;
            }
        }
        Command::Run(args) => {
            let ping_results = ping_all(20)?;
            let ports = ports_stage();
            let bandwidth = bandwidth_stage()?;
            let stress = stress_stage()?;
            let data = ReportData {
                ping: ping_results,
                ports,
                bandwidth,
                stress,
            };
            let thresholds = Thresholds {
                bandwidth_mbps: args.min_bandwidth,
                ..Thresholds::default()
            };
            report::write(&args.report, &data, thresholds)?;
            if args.json {
                print_json(&data)?;
            } else {
                println!("Report written to {}", args.report);
            }
        }
    }
    Ok(())
}
