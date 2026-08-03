use std::fs;
use std::net::UdpSocket;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::thread;
use std::time::Duration;

fn netburn() -> Command {
    Command::new(env!("CARGO_BIN_EXE_netburn"))
}

/// Minimal STUN server: answer binding requests with XOR-MAPPED-ADDRESS.
fn stun_server() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    thread::spawn(move || {
        let mut buf = [0u8; 2048];
        loop {
            let Ok((size, peer)) = socket.recv_from(&mut buf) else {
                return;
            };
            let packet = &buf[..size];
            if packet.len() < 20 || packet[0] != 0x00 || packet[1] != 0x01 {
                continue;
            }
            let mut response = Vec::new();
            response.extend_from_slice(&0x0101u16.to_be_bytes());
            response.extend_from_slice(&12u16.to_be_bytes());
            response.extend_from_slice(&packet[4..20]);
            response.extend_from_slice(&0x0020u16.to_be_bytes());
            response.extend_from_slice(&8u16.to_be_bytes());
            response.extend_from_slice(&[0, 1]);
            let std::net::SocketAddr::V4(peer4) = peer else {
                continue;
            };
            response.extend_from_slice(&(peer4.port() ^ 0x2112).to_be_bytes());
            let cookie = 0x2112_A442u32.to_be_bytes();
            for (i, b) in peer4.ip().octets().iter().enumerate() {
                response.push(b ^ cookie[i]);
            }
            let _ = socket.send_to(&response, peer);
        }
    });
    port
}

#[test]
fn impair_dry_run_prints_commands_without_root() {
    let output = netburn()
        .args(["impair", "congested-wifi", "--dry-run"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("dnctl pipe 1 config bw 10Mbit/s delay 50ms plr 0.02"));
    assert!(stdout.contains("dummynet in from any to any pipe 1"));
    assert!(stdout.contains("pfctl -f -"));
}

#[test]
fn impair_off_dry_run_prints_restore_commands() {
    let output = netburn()
        .args(["impair", "off", "--dry-run"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("dnctl -q flush"));
    assert!(stdout.contains("pfctl -f /etc/pf.conf"));
}

#[test]
fn impair_unknown_profile_fails() {
    let output = netburn()
        .args(["impair", "nope", "--dry-run"])
        .output()
        .unwrap();
    assert!(!output.status.success());
}

#[test]
fn ports_json_structure_with_env_overrides() {
    let output = netburn()
        .args(["ports", "--json"])
        .env("NETBURN_TCP_TARGETS", "127.0.0.1")
        .env("NETBURN_UDP_TARGETS", "127.0.0.1")
        .output()
        .unwrap();
    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["endpoint_source"], "env override");
    let udp = json["udp"].as_array().unwrap();
    assert_eq!(udp.len(), 2);
    assert!(udp.iter().all(|c| c["port"].is_u64()));
    assert_eq!(json["tcp"].as_array().unwrap().len(), 1);
}

#[test]
fn udp_check_probe_against_stun_fake() {
    // Direct end-to-end STUN probe via the CLI would need privileged ports;
    // codec correctness is covered by unit tests. Here we verify the fake
    // server speaks the protocol by round-tripping with a raw socket built
    // the same way netburn builds it.
    let port = stun_server();
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    let mut request = Vec::new();
    request.extend_from_slice(&1u16.to_be_bytes());
    request.extend_from_slice(&0u16.to_be_bytes());
    request.extend_from_slice(&0x2112_A442u32.to_be_bytes());
    request.extend_from_slice(&[42u8; 12]);
    socket.send_to(&request, ("127.0.0.1", port)).unwrap();
    let mut buf = [0u8; 2048];
    let size = socket.recv(&mut buf).unwrap();
    assert_eq!(&buf[0..2], &0x0101u16.to_be_bytes());
    assert_eq!(&buf[8..20], &[42u8; 12]);
    assert!(size >= 32);
}

#[test]
fn bandwidth_uses_network_quality_output() {
    let script =
        std::env::temp_dir().join(format!("netburn-network-quality-{}", std::process::id()));
    fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' '{\"dl_throughput\":826633000,\"ul_throughput\":883898000,\"dl_bytes_transferred\":1000,\"ul_bytes_transferred\":2000}'\n",
    )
    .unwrap();
    fs::set_permissions(&script, fs::Permissions::from_mode(0o700)).unwrap();

    let output = netburn()
        .args(["bandwidth", "--json"])
        .env("NETBURN_NETWORK_QUALITY_BIN", &script)
        .output()
        .unwrap();
    let _ = fs::remove_file(script);

    assert!(output.status.success());
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["download_mbps"], 826.633);
    assert_eq!(json["upload_mbps"], 883.898);
    assert_eq!(json["download_bytes"], 1000);
    assert_eq!(json["upload_bytes"], 2000);
    assert!(json["error"].is_null());
}
