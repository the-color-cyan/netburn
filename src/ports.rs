use anyhow::{Context, Result};
use rand::Rng;
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs, UdpSocket};
use std::time::{Duration, Instant};

/// Reference STUN servers (host, ports) used to verify outbound UDP/STUN egress.
/// Teams media relays only answer authenticated ICE during real calls,
/// so they cannot be probed passively.
const REFERENCE_STUN: [(&str, &[u16]); 2] = [
    ("stun.l.google.com", &[3478, 19302]),
    ("stun.cloudflare.com", &[3478]),
];

pub const UDP_NOTE: &str = "Teams media relays (52.112.0.0/14, 52.122.0.0/15, UDP 3478-3481) only answer authenticated ICE during real calls and cannot be probed passively. These checks verify outbound UDP/STUN egress via public reference servers; a pass means the firewall permits the UDP behavior Teams media needs. A firewall that whitelists only Teams IP ranges may produce a false failure here — confirm with a real Teams call.";
const MAGIC_COOKIE: u32 = 0x2112_A442;

#[derive(Debug, Clone, Serialize)]
pub struct TcpCheck {
    pub host: String,
    pub port: u16,
    pub ok: bool,
    pub elapsed_ms: Option<f64>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct UdpCheck {
    pub host: String,
    pub port: u16,
    pub ok: bool,
    pub elapsed_ms: Option<f64>,
    pub reflexive_address: Option<String>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize)]
pub struct PortResults {
    pub tcp: Vec<TcpCheck>,
    pub udp: Vec<UdpCheck>,
    pub endpoint_source: String,
    pub note: String,
}

#[derive(Debug, Deserialize)]
struct Endpoint {
    #[serde(rename = "serviceAreaDisplayName")]
    display_name: Option<String>,
    category: Option<String>,
    urls: Option<Vec<String>>,
}

/// Extract concrete Teams signaling hosts from the endpoint service payload.
/// Only Optimize/Allow entries (core connectivity); "Default" entries include
/// supporting CDNs with stale DNS. Wildcards stripped to apex ("*.lync.com" -> "lync.com").
fn parse_signaling_hosts(endpoints: &[Endpoint]) -> Vec<String> {
    let mut hosts = Vec::new();
    for endpoint in endpoints.iter().filter(|e| {
        e.display_name.as_deref() == Some("Microsoft Teams")
            && matches!(e.category.as_deref(), Some("Optimize") | Some("Allow"))
    }) {
        for value in endpoint.urls.as_deref().unwrap_or(&[]) {
            let host = value
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .split('/')
                .next()
                .unwrap_or("")
                .trim_start_matches("*.")
                .to_string();
            if !host.is_empty() && !hosts.contains(&host) {
                hosts.push(host);
            }
        }
    }
    hosts
}

pub fn stun_request(transaction: [u8; 12]) -> Vec<u8> {
    let mut packet = Vec::with_capacity(20);
    packet.extend_from_slice(&1u16.to_be_bytes());
    packet.extend_from_slice(&0u16.to_be_bytes());
    packet.extend_from_slice(&MAGIC_COOKIE.to_be_bytes());
    packet.extend_from_slice(&transaction);
    packet
}

pub fn parse_stun_response(packet: &[u8], transaction: &[u8; 12]) -> Option<SocketAddr> {
    if packet.len() < 20
        || u16::from_be_bytes([packet[0], packet[1]]) & 0x0110 != 0x0100
        || packet[8..20] != transaction[..]
    {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([packet[2], packet[3]]))
        .min(packet.len().saturating_sub(20));
    let mut pos = 20;
    while pos + 4 <= 20 + length {
        let kind = u16::from_be_bytes([packet[pos], packet[pos + 1]]);
        let size = usize::from(u16::from_be_bytes([packet[pos + 2], packet[pos + 3]]));
        if pos + 4 + size > packet.len() {
            return None;
        }
        if (kind == 0x0020 || kind == 0x0001) && size >= 8 {
            let family = packet[pos + 5];
            if family == 1 && size >= 8 {
                let port = u16::from_be_bytes([packet[pos + 6], packet[pos + 7]])
                    ^ ((MAGIC_COOKIE >> 16) as u16);
                let mut ip = [0u8; 4];
                for (i, byte) in ip.iter_mut().enumerate() {
                    *byte = packet[pos + 8 + i] ^ MAGIC_COOKIE.to_be_bytes()[i];
                }
                return Some(SocketAddr::from((ip, port)));
            }
        }
        pos += 4 + ((size + 3) & !3);
    }
    None
}

pub fn tcp_check(host: &str, port: u16, timeout: Duration) -> TcpCheck {
    let started = Instant::now();
    let address = format!("{host}:{port}");
    match address
        .to_socket_addrs()
        .ok()
        .and_then(|mut a| a.find_map(|addr| TcpStream::connect_timeout(&addr, timeout).ok()))
    {
        Some(_) => TcpCheck {
            host: host.into(),
            port,
            ok: true,
            elapsed_ms: Some(started.elapsed().as_secs_f64() * 1000.0),
            error: None,
        },
        None => TcpCheck {
            host: host.into(),
            port,
            ok: false,
            elapsed_ms: None,
            error: Some("connection failed or timed out".into()),
        },
    }
}

pub fn udp_check(host: &str, port: u16, timeout: Duration, retries: u8) -> UdpCheck {
    let started = Instant::now();
    let result = (|| -> Result<SocketAddr> {
        let server = format!("{host}:{port}")
            .to_socket_addrs()?
            .next()
            .context("DNS lookup failed")?;
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.connect(server)?;
        socket.set_read_timeout(Some(timeout))?;
        let mut rng = rand::rng();
        for _ in 0..=retries {
            let transaction: [u8; 12] = rng.random();
            socket.send(&stun_request(transaction))?;
            let mut response = [0u8; 2048];
            if let Ok(size) = socket.recv(&mut response)
                && let Some(address) = parse_stun_response(&response[..size], &transaction) {
                    return Ok(address);
                }
        }
        anyhow::bail!("no STUN response")
    })();
    match result {
        Ok(address) => UdpCheck {
            host: host.into(),
            port,
            ok: true,
            elapsed_ms: Some(started.elapsed().as_secs_f64() * 1000.0),
            reflexive_address: Some(address.to_string()),
            error: None,
        },
        Err(error) => UdpCheck {
            host: host.into(),
            port,
            ok: false,
            elapsed_ms: None,
            reflexive_address: None,
            error: Some(error.to_string()),
        },
    }
}

pub fn discover_endpoints() -> (Vec<String>, String) {
    let fallback_tcp = vec![
        "teams.microsoft.com".into(),
        "lync.com".into(),
        "teams.cloud.microsoft".into(),
    ];
    let id = format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        rand::random::<u32>(),
        rand::random::<u16>(),
        rand::random::<u16>(),
        rand::random::<u16>(),
        rand::random::<u64>() & 0xffff_ffff_ffff
    );
    let url = format!("https://endpoints.office.com/endpoints/worldwide?clientrequestid={id}");
    let fetched = Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .ok()
        .and_then(|client| {
            client
                .get(url)
                .send()
                .and_then(|r| r.error_for_status())
                .and_then(|r| r.json::<Vec<Endpoint>>())
                .ok()
        });
    match fetched {
        Some(endpoints) => {
            let mut hosts = parse_signaling_hosts(&endpoints);
            if !hosts.iter().any(|h| h == "teams.microsoft.com") {
                hosts.insert(0, "teams.microsoft.com".into());
            }
            if hosts.len() <= 1 {
                (fallback_tcp, "static fallback".into())
            } else {
                (hosts, "Microsoft 365 endpoint service".into())
            }
        }
        None => (fallback_tcp, "static fallback".into()),
    }
}

pub fn run() -> PortResults {
    let (tcp_hosts, udp_hosts, source) = match (
        std::env::var("NETBURN_TCP_TARGETS"),
        std::env::var("NETBURN_UDP_TARGETS"),
    ) {
        (Ok(tcp), Ok(udp)) => (
            tcp.split(',').map(String::from).collect(),
            udp.split(',').map(String::from).collect(),
            "env override".into(),
        ),
        _ => {
            let (tcp, source) = discover_endpoints();
            (
                tcp,
                REFERENCE_STUN.map(|(host, _)| String::from(host)).to_vec(),
                source,
            )
        }
    };
    let tcp = tcp_hosts
        .iter()
        .map(|host| tcp_check(host, 443, Duration::from_secs(3)))
        .collect();
    let udp = udp_hosts
        .iter()
        .flat_map(|host| {
            let ports: &[u16] = REFERENCE_STUN
                .iter()
                .find(|(h, _)| h == host)
                .map(|(_, p)| *p)
                .unwrap_or(&[3478, 19302]);
            ports
                .iter()
                .map(move |port| udp_check(host, *port, Duration::from_millis(800), 1))
        })
        .collect();
    PortResults {
        tcp,
        udp,
        endpoint_source: source,
        note: UDP_NOTE.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stun_round_trip_codec() {
        let tx = [7u8; 12];
        let request = stun_request(tx);
        let mut response = request.clone();
        response[0] = 1;
        response[1] = 1;
        response[2..4].copy_from_slice(&12u16.to_be_bytes());
        response.extend_from_slice(&0x0020u16.to_be_bytes());
        response.extend_from_slice(&8u16.to_be_bytes());
        response.extend_from_slice(&[0, 1]);
        response.extend_from_slice(&(4000u16 ^ 0x2112u16).to_be_bytes());
        let cookie = MAGIC_COOKIE.to_be_bytes();
        response.extend((0..4).map(|i| [127, 0, 0, 1][i] ^ cookie[i]));
        assert_eq!(
            parse_stun_response(&response, &tx),
            Some("127.0.0.1:4000".parse().unwrap())
        );
    }

    #[test]
    fn parses_signaling_hosts_from_endpoint_payload() {
        let json = r#"[
            {"id": 11, "serviceArea": "Skype", "serviceAreaDisplayName": "Microsoft Teams", "category": "Optimize", "ips": ["52.112.0.0/14"], "udpPorts": "3478,3479,3480,3481"},
            {"id": 12, "serviceArea": "Skype", "serviceAreaDisplayName": "Microsoft Teams", "category": "Allow", "urls": ["*.lync.com", "*.teams.microsoft.com", "teams.microsoft.com", "teams.cloud.microsoft"], "tcpPorts": "80,443"},
            {"id": 16, "serviceArea": "Skype", "serviceAreaDisplayName": "Microsoft Teams", "category": "Default", "urls": ["mlccdnprod.azureedge.net"]},
            {"id": 1, "serviceArea": "Exchange", "serviceAreaDisplayName": "Exchange Online", "category": "Optimize", "urls": ["outlook.office.com"]}
        ]"#;
        let endpoints: Vec<Endpoint> = serde_json::from_str(json).unwrap();
        let hosts = parse_signaling_hosts(&endpoints);
        assert_eq!(
            hosts,
            vec!["lync.com", "teams.microsoft.com", "teams.cloud.microsoft"]
        );
    }
}
