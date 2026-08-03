use anyhow::{Context, Result};
use serde::Serialize;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
pub struct PingResult {
    pub target: String,
    pub transmitted: u32,
    pub received: u32,
    pub loss_percent: f64,
    pub min_ms: Option<f64>,
    pub avg_ms: Option<f64>,
    pub max_ms: Option<f64>,
    pub stddev_ms: Option<f64>,
    pub error: Option<String>,
}

pub fn parse_ping(target: &str, output: &str) -> PingResult {
    let mut result = PingResult {
        target: target.into(),
        transmitted: 0,
        received: 0,
        loss_percent: 100.0,
        min_ms: None,
        avg_ms: None,
        max_ms: None,
        stddev_ms: None,
        error: None,
    };
    for line in output.lines() {
        if line.contains("packets transmitted") {
            let fields: Vec<&str> = line
                .split_whitespace()
                .map(|s| s.trim_end_matches(','))
                .collect();
            let count_for = |keyword: &str| -> u32 {
                fields
                    .windows(3)
                    .find(|w| w[1] == "packets" && w[2] == keyword)
                    .and_then(|w| w[0].parse().ok())
                    .unwrap_or(0)
            };
            result.transmitted = count_for("transmitted");
            result.received = count_for("received");
            if let Some(loss) = line.split(',').find(|part| part.contains("packet loss")) {
                result.loss_percent = loss
                    .split('%')
                    .next()
                    .and_then(|s| s.split_whitespace().last())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(100.0);
            }
        }
        if line.contains("min/avg/max")
            && let Some(values) = line.split('=').nth(1) {
                let nums: Vec<f64> = values
                    .split('/')
                    .filter_map(|s| s.split_whitespace().next()?.parse().ok())
                    .collect();
                if nums.len() == 4 {
                    result.min_ms = Some(nums[0]);
                    result.avg_ms = Some(nums[1]);
                    result.max_ms = Some(nums[2]);
                    result.stddev_ms = Some(nums[3]);
                }
            }
    }
    result
}

pub fn ping(target: &str, count: u32) -> Result<PingResult> {
    let output = Command::new("ping")
        .args(["-c", &count.to_string(), "-i", "0.2", target])
        .output()
        .context("could not execute ping")?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut result = parse_ping(target, &text);
    if !output.status.success() && result.received == 0 {
        result.error = Some(text.lines().last().unwrap_or("ping failed").to_string());
    }
    Ok(result)
}

pub fn default_gateway() -> Option<String> {
    let output = Command::new("route")
        .args(["-n", "get", "default"])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| line.trim_start().starts_with("gateway:"))
        .and_then(|line| line.split(':').nth(1))
        .map(|gw| gw.trim().to_string())
}

pub fn default_targets() -> Vec<String> {
    let mut targets: Vec<String> = default_gateway().into_iter().collect();
    for host in ["1.1.1.1", "teams.microsoft.com"] {
        targets.push(host.into());
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_macos_ping() {
        let output = "--- 1.1.1.1 ping statistics ---\n20 packets transmitted, 20 packets received, 0.0% packet loss\nround-trip min/avg/max/stddev = 8.123/10.500/20.000/2.100 ms";
        let p = parse_ping("1.1.1.1", output);
        assert_eq!(p.transmitted, 20);
        assert_eq!(p.received, 20);
        assert_eq!(p.loss_percent, 0.0);
        assert_eq!(p.avg_ms, Some(10.5));
    }
    #[test]
    fn parses_gateway_from_route() {
        let output = "   route to: default\ndestination: default\n    gateway: 192.168.1.1\n  interface: en0\n";
        let gw = output
            .lines()
            .find(|line| line.trim_start().starts_with("gateway:"))
            .and_then(|line| line.split(':').nth(1))
            .map(|gw| gw.trim().to_string());
        assert_eq!(gw.as_deref(), Some("192.168.1.1"));
    }
}
