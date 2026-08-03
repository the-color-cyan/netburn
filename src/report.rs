use crate::{
    bandwidth::BandwidthResult, ping::PingResult, ports::PortResults, stress::StressResult,
};
use anyhow::Result;
use serde::Serialize;
use std::fs;

#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    pub jitter_ms: f64,
    pub loss_percent: f64,
    pub media_rtt_ms: f64,
    pub bandwidth_mbps: f64,
}
impl Default for Thresholds {
    fn default() -> Self {
        Self {
            jitter_ms: 30.0,
            loss_percent: 1.0,
            media_rtt_ms: 100.0,
            bandwidth_mbps: 10.0,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ReportData {
    pub ping: Vec<PingResult>,
    pub ports: PortResults,
    pub bandwidth: BandwidthResult,
    pub stress: StressResult,
}

fn esc(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn badge(status: &str) -> String {
    format!("<span class=\"badge {status}\">{status}</span>")
}
fn status(value: Option<f64>, limit: f64) -> &'static str {
    match value {
        Some(value) if value <= limit => "pass",
        Some(value) if value <= limit * 2.0 => "warn",
        Some(_) => "fail",
        None => "fail",
    }
}
pub fn write(path: &str, data: &ReportData, thresholds: Thresholds) -> Result<()> {
    let json = serde_json::to_string_pretty(data)?.replace("</script", "<\\/script");
    let ping_rows = data
        .ping
        .iter()
        .map(|p| {
            format!(
                "<tr><td>{}</td><td>{:.2} ms</td><td>{:.2} ms</td><td>{:.2}% {}</td><td>{}</td></tr>",
                esc(&p.target),
                p.avg_ms.unwrap_or(0.0),
                p.stddev_ms.unwrap_or(0.0),
                p.loss_percent,
                badge(status(
                    if p.loss_percent.is_nan() { None } else { Some(p.loss_percent) },
                    thresholds.loss_percent
                )),
                badge(status(p.stddev_ms, thresholds.jitter_ms))
            )
        })
        .collect::<String>();
    let tcp_rows = data
        .ports
        .tcp
        .iter()
        .map(|p| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&p.host),
                p.port,
                badge(if p.ok { "pass" } else { "fail" })
            )
        })
        .collect::<String>();
    let udp_rows = data
        .ports
        .udp
        .iter()
        .map(|p| {
            format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
                esc(&p.host),
                p.port,
                p.reflexive_address.as_deref().unwrap_or("—"),
                p.elapsed_ms
                    .map(|ms| format!("{ms:.2} ms"))
                    .unwrap_or_else(|| "—".into()),
                badge(if p.ok {
                    status(p.elapsed_ms, thresholds.media_rtt_ms)
                } else {
                    "fail"
                })
            )
        })
        .collect::<String>();
    let bandwidth_status = if data.bandwidth.download_mbps.unwrap_or(0.0)
        >= thresholds.bandwidth_mbps
        && data.bandwidth.upload_mbps.unwrap_or(0.0) >= thresholds.bandwidth_mbps
    {
        "pass"
    } else {
        "warn"
    };
    let html = format!(
        r##"<!doctype html><html><head><meta charset="utf-8"><title>Network Report</title><style>body{{font:15px system-ui,sans-serif;max-width:1100px;margin:2rem auto;padding:0 1rem;background:#16191d;color:#e2e6ea}}table{{border-collapse:collapse;width:100%;margin:1rem 0 2rem}}td,th{{border:1px solid #363d45;padding:.5rem;text-align:left}}th{{background:#232830}}small{{color:#9aa4ae}}.badge{{border-radius:4px;padding:.15rem .45rem;text-transform:uppercase;font-size:.75rem;font-weight:700}}.pass{{background:#14532d;color:#86efac}}.warn{{background:#713f12;color:#fde047}}.fail{{background:#7f1d1d;color:#fca5a5}}code{{white-space:pre-wrap}}</style></head><body><h1>Network report</h1><p>Teams-focused network assessment.</p><p><em>This is a prototype — results and heuristics are subject to change.</em></p><h2>Ping</h2><table><tr><th>Target</th><th>Average</th><th>Jitter (stddev)</th><th>Loss</th><th>Status</th></tr>{ping_rows}</table><h2>Teams ports</h2><p>Endpoint source: {source}</p><h3>TCP signaling</h3><table><tr><th>Host</th><th>Port</th><th>Status</th></tr>{tcp_rows}</table><h3>UDP media egress (STUN)</h3><p><small>{note}</small></p><table><tr><th>Host</th><th>Port</th><th>Reflexive address</th><th>RTT</th><th>Status</th></tr>{udp_rows}</table><h2>Bandwidth</h2><p>Download: <strong>{download:.2} Mbps</strong> · Upload: <strong>{upload:.2} Mbps</strong> {badge_status}</p><p><small>Metric is macOS networkQuality download/upload throughput; may underreport versus fast.com/Ookla because methodologies and server selection differ.</small></p><h2>Stress / bufferbloat</h2><table><tr><th>Idle</th><th>Loaded</th><th>Delta</th><th>Grade</th></tr><tr><td>{idle:.2} ms</td><td>{loaded:.2} ms</td><td>{delta:.2} ms</td><td>{grade}</td></tr></table><h2>Raw results</h2><script type="application/json" id="netburn-results">{json}</script></body></html>"##,
        ping_rows = ping_rows,
        source = esc(&data.ports.endpoint_source),
        note = esc(&data.ports.note),
        tcp_rows = tcp_rows,
        udp_rows = udp_rows,
        download = data.bandwidth.download_mbps.unwrap_or(0.0),
        upload = data.bandwidth.upload_mbps.unwrap_or(0.0),
        badge_status = badge(bandwidth_status),
        idle = data.stress.idle_ms.unwrap_or(0.0),
        loaded = data.stress.loaded_ms.unwrap_or(0.0),
        delta = data.stress.delta_ms.unwrap_or(0.0),
        grade = esc(&data.stress.grade),
        json = json
    );
    fs::write(path, html)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bandwidth::BandwidthResult, ping::PingResult, ports::PortResults, stress::StressResult,
    };

    #[test]
    fn report_contains_sections_badges_and_json() {
        let data = ReportData {
            ping: vec![PingResult {
                target: "1.1.1.1".into(),
                transmitted: 20,
                received: 20,
                loss_percent: 0.0,
                min_ms: Some(8.0),
                avg_ms: Some(10.0),
                max_ms: Some(12.0),
                stddev_ms: Some(2.0),
                error: None,
            }],
            ports: PortResults {
                tcp: vec![],
                udp: vec![],
                endpoint_source: "test".into(),
                note: "test note".into(),
            },
            bandwidth: BandwidthResult {
                download_mbps: Some(50.0),
                upload_mbps: Some(25.0),
                download_bytes: 1,
                upload_bytes: 1,
                error: None,
            },
            stress: StressResult {
                idle_ms: Some(10.0),
                loaded_ms: Some(12.0),
                delta_ms: Some(2.0),
                grade: "A".into(),
                error: None,
            },
        };
        let path =
            std::env::temp_dir().join(format!("netburn-report-test-{}.html", std::process::id()));
        write(path.to_str().unwrap(), &data, Thresholds::default()).unwrap();
        let html = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        for needle in [
            "Ping",
            "Teams ports",
            "Bandwidth",
            "Metric is macOS networkQuality download/upload throughput",
            "bufferbloat",
            "badge pass",
            "background:#16191d",
            "application/json",
            "\"grade\": \"A\"",
        ] {
            assert!(html.contains(needle), "missing {needle}");
        }
    }
}
