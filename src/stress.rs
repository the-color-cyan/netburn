use crate::{bandwidth, ping};
use anyhow::Result;
use serde::Serialize;
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct StressResult {
    pub idle_ms: Option<f64>,
    pub loaded_ms: Option<f64>,
    pub delta_ms: Option<f64>,
    pub grade: String,
    pub error: Option<String>,
}

pub fn grade(delta: f64) -> &'static str {
    match delta {
        d if d < 5.0 => "A",
        d if d < 15.0 => "B",
        d if d < 40.0 => "C",
        d if d < 80.0 => "D",
        _ => "F",
    }
}

pub fn run() -> Result<StressResult> {
    let target = std::env::var("NETBURN_STRESS_TARGET").unwrap_or_else(|_| "1.1.1.1".into());
    let idle = ping::ping(&target, 3)?;
    let idle_ms = idle.avg_ms;
    let seconds = std::env::var("NETBURN_STRESS_SECONDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10u64);
    let workers: Vec<_> = (0..4)
        .map(|_| {
            thread::spawn(move || {
                let until = Instant::now() + Duration::from_secs(seconds);
                while Instant::now() < until {
                    let _ = bandwidth::download_once(2 * 1024 * 1024);
                }
            })
        })
        .collect();
    let until = Instant::now() + Duration::from_secs(seconds);
    let mut samples = Vec::new();
    while Instant::now() < until {
        if let Ok(result) = ping::ping(&target, 1)
            && let Some(ms) = result.avg_ms {
                samples.push(ms);
            }
    }
    for worker in workers {
        let _ = worker.join();
    }
    let loaded_ms = if samples.is_empty() {
        None
    } else {
        Some(samples.iter().sum::<f64>() / samples.len() as f64)
    };
    let delta_ms = idle_ms
        .zip(loaded_ms)
        .map(|(idle, loaded)| (loaded - idle).max(0.0));
    Ok(StressResult {
        idle_ms,
        loaded_ms,
        delta_ms,
        grade: delta_ms.map_or("F".into(), |d| grade(d).into()),
        error: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grades_latency_delta() {
        assert_eq!(grade(4.9), "A");
        assert_eq!(grade(15.0), "C");
        assert_eq!(grade(100.0), "F");
    }
}
