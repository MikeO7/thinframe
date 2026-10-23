//! Find the server on the LAN (server/internal/discover has the protocol). The agent asks when
//! its server stops answering, so a server whose address changed (DHCP, a new computer) is found
//! again with no card in hand. A probe goes to the broadcast addresses the frame can reach; the
//! server answers with its HTTP port, and its address is the reply's source.
//!
//! A frame with a token sends its id and a fresh nonce and moves only to a server that proves it
//! knows the token (HMAC keyed by the token's SHA-256), so a stranger on the LAN cannot collect
//! the token by answering. A frame with no token takes any answer: it holds nothing to steal,
//! and pairing still needs the PAIR button on its screen.
use crate::sha256::{hmac_hex, sha256_hex};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

/// The server's UDP port (discover.Port on the Go side).
pub const PORT: u16 = 41007;
const PROBE: &str = "thinframe-discover 1";
const REPLY: &str = "thinframe-server 1 ";
/// How long one probe waits for an answer.
pub const WAIT: Duration = Duration::from_secs(2);

/// Who is asking: the frame id and its token (None: not paired yet, take any server).
pub struct Asker<'a> {
    pub id: &'a str,
    pub token: Option<&'a str>,
}

/// Where a probe goes: every network (the kernel sends the limited broadcast on the default
/// route, Wi-Fi once it is up) and the USB-C link to a computer (statically 192.168.7.0/24).
pub fn targets() -> Vec<SocketAddr> {
    vec![
        SocketAddr::from((Ipv4Addr::BROADCAST, PORT)),
        SocketAddr::from((Ipv4Addr::new(192, 168, 7, 255), PORT)),
    ]
}

/// Where a probe for a server on the network goes: the limited broadcast (sent on the default
/// route, which may be the USB-C link) and each network interface's own broadcast address, so
/// Wi-Fi is asked even when the default route is the cable.
pub fn lan_targets() -> Vec<SocketAddr> {
    let mut t = vec![SocketAddr::from((Ipv4Addr::BROADCAST, PORT))];
    if let Ok(o) = std::process::Command::new("ip")
        .args(["-4", "-o", "addr"])
        .output()
    {
        t.extend(
            parse_brd(&String::from_utf8_lossy(&o.stdout))
                .into_iter()
                .map(|b| SocketAddr::from((b, PORT))),
        );
    }
    t
}

/// The `brd` addresses in `ip -4 -o addr` output, the USB-C link's left out.
pub fn parse_brd(text: &str) -> Vec<Ipv4Addr> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut w = line.split_whitespace();
        while let Some(x) = w.next() {
            if x == "brd" {
                if let Some(b) = w.next().and_then(|b| b.parse::<Ipv4Addr>().ok()) {
                    if !is_usb(IpAddr::V4(b)) && !out.contains(&b) {
                        out.push(b);
                    }
                }
            }
        }
    }
    out
}

/// One round: send the probe to `targets`, take the first valid answer within `wait`.
/// Ok(None) when nobody answered; Err only when no socket could be made at all.
pub fn find(
    targets: &[SocketAddr],
    wait: Duration,
    who: &Asker,
    nonce: &str,
) -> io::Result<Option<String>> {
    let sock = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0))?;
    sock.set_broadcast(true)?;
    let probe = format!("{PROBE} {} {nonce}\n", who.id);
    let mut sent = 0;
    for t in targets {
        // A network that is not up yet ("network unreachable") is not an error, just no answer.
        if sock.send_to(probe.as_bytes(), t).is_ok() {
            sent += 1;
        }
    }
    if sent == 0 {
        return Ok(None);
    }
    let deadline = Instant::now() + wait;
    let mut buf = [0u8; 512];
    // The first answer over the USB-C link waits up to PREFER_LAN for one over the network:
    // a frame that moved to the cable address would lose its server when the cable comes out.
    let mut usb: Option<(String, Instant)> = None;
    loop {
        let mut end = deadline;
        if let Some((_, t)) = &usb {
            end = end.min(*t + PREFER_LAN);
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(usb.map(|(u, _)| u));
        }
        sock.set_read_timeout(Some(left))?;
        match sock.recv_from(&mut buf) {
            Ok((n, from)) => {
                if let Some(url) = parse_reply(&buf[..n], from.ip(), who, nonce) {
                    if !is_usb(from.ip()) {
                        return Ok(Some(url));
                    }
                    usb.get_or_insert((url, Instant::now()));
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                return Ok(usb.map(|(u, _)| u))
            }
            Err(e) => return Err(e),
        }
    }
}

/// How long a USB-link answer waits for a network one.
const PREFER_LAN: Duration = Duration::from_millis(500);

/// The USB-C gadget network (statuspage::USB_IP is the frame's end).
pub fn is_usb(ip: IpAddr) -> bool {
    matches!(ip, IpAddr::V4(v) if v.octets()[..3] == [192, 168, 7])
}

/// The proof a server that knows `token` sends (discover.Proof on the Go side).
pub fn proof(token: &str, id: &str, nonce: &str, port: &str) -> String {
    let key = sha256_hex(token.as_bytes());
    let msg = format!(
        "thinframe-discover|{}|{nonce}|{port}",
        id.to_ascii_lowercase()
    );
    hmac_hex(key.as_bytes(), msg.as_bytes())
}

/// "thinframe-server 1 8080 <proof>" from 192.168.1.10 -> "http://192.168.1.10:8080", when the
/// proof checks out (or the frame has no token).
pub fn parse_reply(bytes: &[u8], from: IpAddr, who: &Asker, nonce: &str) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?.trim();
    let mut f = text.strip_prefix(REPLY)?.split_whitespace();
    let port_s = f.next()?;
    let port: u16 = port_s.parse().ok()?;
    if port == 0 {
        return None;
    }
    if let Some(token) = who.token.filter(|t| !t.is_empty()) {
        let want = proof(token, who.id, nonce, port_s);
        if !f.any(|p| p == want) {
            return None;
        }
    }
    match from {
        IpAddr::V4(ip) => Some(format!("http://{ip}:{port}")),
        IpAddr::V6(ip) => Some(format!("http://[{ip}]:{port}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    /// The same vector is in server/internal/discover: both sides must agree byte for byte.
    const VECTOR: &str = "26dbd77d3662d486572c0fc1ac939b856f215d26840f62600ac4a4efdb781374";
    const NONCE: &str = "0123456789abcdef";

    fn office(token: Option<&str>) -> Asker<'_> {
        Asker {
            id: "office",
            token,
        }
    }

    /// A pretend server on 127.0.0.1 that answers every probe with `answer(probe)`.
    fn responder(answer: fn(&str) -> String) -> SocketAddr {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = s.local_addr().unwrap();
        thread::spawn(move || {
            let mut b = [0u8; 256];
            while let Ok((n, from)) = s.recv_from(&mut b) {
                let p = String::from_utf8_lossy(&b[..n]).to_string();
                if p.starts_with(PROBE) {
                    let _ = s.send_to(answer(&p).as_bytes(), from);
                }
            }
        });
        addr
    }

    #[test]
    fn proof_matches_the_server() {
        assert_eq!(proof("t0ken", "office", NONCE, "8080"), VECTOR);
        assert_eq!(proof("t0ken", "Office", NONCE, "8080"), VECTOR);
    }

    #[test]
    fn finds_a_server_that_proves_the_token() {
        let a = responder(|p| {
            let nonce = p.split_whitespace().nth(3).unwrap().to_string();
            format!(
                "thinframe-server 1 8080 {}\n",
                proof("t0ken", "office", &nonce, "8080")
            )
        });
        let url = find(&[a], Duration::from_secs(2), &office(Some("t0ken")), NONCE).unwrap();
        assert_eq!(url.as_deref(), Some("http://127.0.0.1:8080"));
    }

    #[test]
    fn a_frame_with_a_token_ignores_a_server_without_proof() {
        let a = responder(|_| "thinframe-server 1 8080\n".into());
        let t = Instant::now();
        let got = find(
            &[a],
            Duration::from_millis(300),
            &office(Some("t0ken")),
            NONCE,
        )
        .unwrap();
        assert_eq!(got, None);
        assert!(t.elapsed() >= Duration::from_millis(300));
        // Not paired yet: the same answer is good enough.
        let url = find(&[a], Duration::from_secs(2), &office(None), NONCE).unwrap();
        assert_eq!(url.as_deref(), Some("http://127.0.0.1:8080"));
    }

    #[test]
    fn first_good_answer_wins_over_noise() {
        let bad = responder(|_| "thinframe-server 2 8080\n".into());
        let good = responder(|_| "thinframe-server 1 9090".into());
        let url = find(&[bad, good], Duration::from_secs(2), &office(None), NONCE).unwrap();
        assert_eq!(url.as_deref(), Some("http://127.0.0.1:9090"));
    }

    #[test]
    fn reply_parsing() {
        let ip: IpAddr = "192.168.1.10".parse().unwrap();
        let no = office(None);
        assert_eq!(
            parse_reply(b"thinframe-server 1 8080\n", ip, &no, NONCE).as_deref(),
            Some("http://192.168.1.10:8080")
        );
        assert_eq!(parse_reply(b"thinframe-server 1 0", ip, &no, NONCE), None);
        assert_eq!(parse_reply(b"thinframe-server 1 x", ip, &no, NONCE), None);
        assert_eq!(parse_reply(b"thinframe-discover 1", ip, &no, NONCE), None);
        assert_eq!(parse_reply(b"\xff\xfe", ip, &no, NONCE), None);
        let yes = office(Some("t0ken"));
        let good = format!("thinframe-server 1 8080 deadbeef {VECTOR}");
        assert!(parse_reply(good.as_bytes(), ip, &yes, NONCE).is_some());
        assert_eq!(
            parse_reply(b"thinframe-server 1 8080 deadbeef", ip, &yes, NONCE),
            None
        );
        let other_nonce = "fedcba9876543210";
        assert_eq!(parse_reply(good.as_bytes(), ip, &yes, other_nonce), None);
        let v6: IpAddr = "fe80::1".parse().unwrap();
        assert_eq!(
            parse_reply(b"thinframe-server 1 80", v6, &no, NONCE).as_deref(),
            Some("http://[fe80::1]:80")
        );
    }

    #[test]
    fn usb_link_addresses() {
        assert!(is_usb("192.168.7.1".parse().unwrap()));
        assert!(!is_usb("192.168.1.172".parse().unwrap()));
        assert!(!is_usb("127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn interface_broadcasts() {
        let text = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever\n\
            2: usb0    inet 192.168.7.2/24 brd 192.168.7.255 scope global usb0\n\
            4: wlan0    inet 192.168.1.240/24 brd 192.168.1.255 scope global dynamic wlan0\n";
        assert_eq!(parse_brd(text), [Ipv4Addr::new(192, 168, 1, 255)]);
        assert!(parse_brd("").is_empty());
    }

    #[test]
    fn targets_are_the_broadcasts() {
        let t = targets();
        assert_eq!(t[0].to_string(), "255.255.255.255:41007");
        assert_eq!(t[1].to_string(), "192.168.7.255:41007");
    }
}
