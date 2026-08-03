use anyhow::{bail, Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct BandwidthResult {
    pub download_mbps: Option<f64>,
    pub upload_mbps: Option<f64>,
    pub download_bytes: u64,
    pub upload_bytes: u64,
    pub error: Option<String>,
}

fn base_url() -> String {
    std::env::var("NETBURN_SPEED_BASE")
        .unwrap_or_else(|_| "https://speed.cloudflare.com".into())
        .trim_end_matches('/')
        .into()
}

pub fn run() -> Result<BandwidthResult> {
    let binary =
        std::env::var("NETBURN_NETWORK_QUALITY_BIN").unwrap_or_else(|_| "networkQuality".into());
    let output = Command::new(binary)
        .args(["-c", "-s"])
        .output()
        .context("failed to run networkQuality")?;
    if !output.status.success() {
        bail!(
            "networkQuality failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let result: Value =
        serde_json::from_slice(&output.stdout).context("networkQuality returned invalid JSON")?;
    let download_bps = result["dl_throughput"]
        .as_f64()
        .context("networkQuality omitted download throughput")?;
    let upload_bps = result["ul_throughput"]
        .as_f64()
        .context("networkQuality omitted upload throughput")?;

    Ok(BandwidthResult {
        download_mbps: Some(download_bps / 1_000_000.0),
        upload_mbps: Some(upload_bps / 1_000_000.0),
        download_bytes: result["dl_bytes_transferred"].as_u64().unwrap_or(0),
        upload_bytes: result["ul_bytes_transferred"].as_u64().unwrap_or(0),
        error: None,
    })
}

pub fn download_once(size: u64) -> Result<u64> {
    let response = reqwest::blocking::get(format!("{}/__down?bytes={size}", base_url()))
        .context("download request failed")?;
    Ok(response.bytes()?.len() as u64)
}
