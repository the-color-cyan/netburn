use anyhow::{Context, Result, bail};
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Copy)]
pub struct Profile {
    pub name: &'static str,
    pub delay_ms: u32,
    pub loss: f64,
    pub bandwidth_mbit: u32,
}

pub const PROFILES: [Profile; 3] = [
    Profile {
        name: "congested-wifi",
        delay_ms: 50,
        loss: 0.02,
        bandwidth_mbit: 10,
    },
    Profile {
        name: "bad-wifi",
        delay_ms: 100,
        loss: 0.05,
        bandwidth_mbit: 5,
    },
    Profile {
        name: "lossy-vpn",
        delay_ms: 80,
        loss: 0.03,
        bandwidth_mbit: 20,
    },
];

fn command_text(program: &str, args: &[String]) -> String {
    format!("sudo {program} {}", args.join(" "))
}
fn run_command(program: &str, args: &[String], dry_run: bool) -> Result<()> {
    println!("{}", command_text(program, args));
    if dry_run {
        return Ok(());
    }
    let status = Command::new(program)
        .args(args)
        .status()
        .with_context(|| format!("could not execute {program}"))?;
    if !status.success() {
        bail!("{program} failed with {status}");
    }
    Ok(())
}

pub fn apply(
    name: &str,
    delay: Option<u32>,
    loss: Option<f64>,
    bandwidth: Option<u32>,
    dry_run: bool,
) -> Result<()> {
    if !dry_run && !is_root() {
        bail!("impair requires root; rerun with: sudo netburn impair {name}");
    }
    let profile = PROFILES.iter().find(|p| p.name == name);
    if profile.is_none() && delay.is_none() {
        bail!("unknown profile '{name}'; choose congested-wifi, bad-wifi, or lossy-vpn");
    }
    let p = profile.copied().unwrap_or(Profile {
        name: "custom",
        delay_ms: 0,
        loss: 0.0,
        bandwidth_mbit: 0,
    });
    let delay = delay.unwrap_or(p.delay_ms);
    let loss = loss.unwrap_or(p.loss);
    let bandwidth = bandwidth.unwrap_or(p.bandwidth_mbit);
    if !(0.0..=1.0).contains(&loss) {
        bail!("loss must be between 0 and 1");
    }
    let config = if bandwidth == 0 {
        format!("delay {delay}ms plr {loss}")
    } else {
        format!("bw {bandwidth}Mbit/s delay {delay}ms plr {loss}")
    };
    run_command(
        "dnctl",
        &["pipe".into(), "1".into(), "config".into(), config],
        dry_run,
    )?;
    let rules = "dummynet in from any to any pipe 1\ndummynet out from any to any pipe 1\n";
    println!("sudo pfctl -f - <<'NETBURN_RULES'\n{rules}NETBURN_RULES");
    if !dry_run {
        let mut child = Command::new("pfctl")
            .args(["-f", "-"])
            .stdin(Stdio::piped())
            .spawn()
            .context("could not execute pfctl")?;
        std::io::Write::write_all(
            child.stdin.as_mut().context("pfctl stdin unavailable")?,
            rules.as_bytes(),
        )?;
        let status = child.wait()?;
        if !status.success() {
            bail!("pfctl failed with {status}");
        }
        let status = Command::new("pfctl")
            .args(["-E"])
            .status()
            .context("could not enable pf")?;
        println!("sudo pfctl -E");
        if !status.success() {
            bail!("pfctl -E failed with {status}");
        }
        std::fs::write(marker_path(), b"enabled")?;
    }
    Ok(())
}

pub fn off(dry_run: bool) -> Result<()> {
    if !dry_run && !is_root() {
        bail!("impair off requires root; rerun with: sudo netburn impair off");
    }
    run_command("dnctl", &["-q".into(), "flush".into()], dry_run)?;
    run_command("pfctl", &["-f".into(), "/etc/pf.conf".into()], dry_run)?;
    if dry_run {
        println!("sudo pfctl -d (only if netburn enabled PF)");
    } else if std::path::Path::new(&marker_path()).exists() {
        run_command("pfctl", &["-d".into()], false)?;
        let _ = std::fs::remove_file(marker_path());
    }
    Ok(())
}

fn marker_path() -> String {
    format!("{}/netburn-pf-enabled", std::env::temp_dir().display())
}
fn is_root() -> bool {
    libc_getuid() == 0
}
#[cfg(unix)]
fn libc_getuid() -> u32 {
    std::process::Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(1)
}
#[cfg(not(unix))]
fn libc_getuid() -> u32 {
    1
}
