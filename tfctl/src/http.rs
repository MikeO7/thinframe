//! Minimal HTTP/1.1 over TcpStream: URL splitting, one GET, response parsing. LAN only, no TLS.
use crate::sha256::Sha256;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Url {
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl Url {
    /// Accepts http://, ws:// (and nothing else: no TLS on the frame).
    pub fn parse(s: &str) -> Option<Url> {
        let rest = s
            .strip_prefix("http://")
            .or_else(|| s.strip_prefix("ws://"))?;
        let (hostport, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        // "[v6]" or "[v6]:port" (RFC 3986 IP-literal), else "host" or "host:port".
        let (host, port) = match hostport.strip_prefix('[') {
            Some(v6) => match v6.split_once(']')? {
                (h, "") => (h, 80),
                (h, p) => (h, p.strip_prefix(':')?.parse().ok()?),
            },
            None => match hostport.rsplit_once(':') {
                Some((h, p)) => (h, p.parse().ok()?),
                None => (hostport, 80),
            },
        };
        if host.is_empty() || host.contains(['[', ']']) {
            return None;
        }
        Some(Url {
            host: host.to_string(),
            port,
            path: path.to_string(),
        })
    }

    pub fn with_path(&self, path: &str) -> Url {
        Url {
            host: self.host.clone(),
            port: self.port,
            path: path.to_string(),
        }
    }

    /// `Host:` value; an IPv6 literal goes back in brackets (RFC 7230 5.4).
    pub fn host_header(&self) -> String {
        let host = if self.host.contains(':') {
            format!("[{}]", self.host)
        } else {
            self.host.clone()
        };
        if self.port == 80 {
            host
        } else {
            format!("{host}:{}", self.port)
        }
    }
}

pub fn connect(url: &Url, timeout: Duration) -> io::Result<TcpStream> {
    let addr = (url.host.as_str(), url.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| io::Error::other("no address"))?;
    let s = TcpStream::connect_timeout(&addr, timeout)?;
    s.set_read_timeout(Some(timeout))?;
    s.set_write_timeout(Some(timeout))?;
    s.set_nodelay(true)?;
    Ok(s)
}

#[derive(Debug, PartialEq)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Parses "HTTP/1.1 200 OK\r\nK: V\r\n\r\n". Returns (status, headers, header_len) or None if incomplete.
/// (status, headers, byte length of the head).
pub type Head = (u16, Vec<(String, String)>, usize);

pub fn parse_head(buf: &[u8]) -> Option<Head> {
    let end = buf.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    let head = std::str::from_utf8(&buf[..end]).ok()?;
    let mut lines = head.split("\r\n");
    let status = lines.next()?.split_whitespace().nth(1)?.parse().ok()?;
    let headers = lines
        .filter_map(|l| l.split_once(':'))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect();
    Some((status, headers, end))
}

/// GET with a bearer token, Connection: close. Body read by Content-Length or to EOF.
pub fn get(url: &Url, token: &str, timeout: Duration) -> io::Result<Response> {
    request("GET", url, token, None, timeout)
}

/// POST a JSON body (pairing, TASK 16).
pub fn post_json(url: &Url, token: &str, body: &str, timeout: Duration) -> io::Result<Response> {
    request("POST", url, token, Some(body), timeout)
}

fn request(
    method: &str,
    url: &Url,
    token: &str,
    body: Option<&str>,
    timeout: Duration,
) -> io::Result<Response> {
    request_safe(&url.path)?;
    request_safe(token)?;
    let mut s = connect(url, timeout)?;
    let mut req = format!(
        "{method} {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: tfctl\r\n",
        url.path,
        url.host_header()
    );
    if !token.is_empty() {
        req.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    if let Some(b) = body {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            b.len()
        ));
    }
    req.push_str("\r\n");
    req.push_str(body.unwrap_or(""));
    s.write_all(req.as_bytes())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let n = s.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some((_, headers, end)) = parse_head(&buf) {
            let len = headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, v)| v.parse::<usize>().ok());
            if let Some(len) = len {
                if buf.len() >= end + len {
                    break;
                }
            }
        }
        if buf.len() > 4 << 20 {
            return Err(io::Error::other("response too large"));
        }
    }
    let (status, headers, end) =
        parse_head(&buf).ok_or_else(|| io::Error::other("bad http response"))?;
    Ok(Response {
        status,
        headers,
        body: buf[end..].to_vec(),
    })
}

/// Streaming GET into a file, hashing on the way: the body never sits in memory (a rootfs
/// A value that goes into the request line or a header: the frame id (in the path), an
/// artifact name, the token. A control character or a space would end the line or the field
/// early and let a server-supplied name inject a header.
fn request_safe(s: &str) -> io::Result<()> {
    if s.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err(io::Error::other("control character or space in request"));
    }
    Ok(())
}

/// image is ~150 MB). Returns the body's SHA-256 hex. Non-200 is an error; a body shorter than
/// Content-Length is an error. No Range resume yet (the server supports it; TODO when Wi-Fi
/// drops prove to be a problem).
pub fn download(url: &Url, token: &str, timeout: Duration, dest: &Path) -> io::Result<String> {
    request_safe(&url.path)?;
    request_safe(token)?;
    let mut s = connect(url, timeout)?;
    let mut req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: tfctl\r\n",
        url.path,
        url.host_header()
    );
    if !token.is_empty() {
        req.push_str(&format!("Authorization: Bearer {token}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes())?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 16384];
    let (status, headers, end) = loop {
        let n = s.read(&mut chunk)?;
        if n == 0 {
            return Err(io::Error::other("connection closed before headers"));
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(h) = parse_head(&buf) {
            break h;
        }
        if buf.len() > 64 << 10 {
            return Err(io::Error::other("http head too large"));
        }
    };
    if status != 200 {
        return Err(io::Error::other(format!("GET {}: HTTP {status}", url.path)));
    }
    let want = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<u64>().ok());
    let mut out = fs::File::create(dest)?;
    let mut hash = Sha256::new();
    let mut got = 0u64;
    let mut data = buf[end..].to_vec();
    loop {
        out.write_all(&data)?;
        hash.update(&data);
        got += data.len() as u64;
        if want.is_some_and(|w| got >= w) {
            break;
        }
        let n = s.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        data = chunk[..n].to_vec();
    }
    out.sync_all()?;
    if let Some(w) = want {
        if got != w {
            return Err(io::Error::other(format!("short body: {got} of {w} bytes")));
        }
    }
    Ok(hash.finish_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_values_reject_control_characters_and_spaces() {
        assert!(request_safe("/api/frames/kitchen/config").is_ok());
        assert!(request_safe("t0k3n").is_ok());
        assert!(request_safe("").is_ok());
        assert!(request_safe("a b").is_err());
        assert!(request_safe("x\r\nX-Injected: y").is_err());
        assert!(request_safe("\x7f").is_err());
    }

    /// One-shot HTTP server on localhost for the download test.
    pub fn serve_once(response: Vec<u8>) -> Url {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (mut c, _) = l.accept().unwrap();
            let mut req = [0u8; 4096];
            let _ = c.read(&mut req);
            c.write_all(&response).unwrap();
        });
        Url::parse(&format!("http://127.0.0.1:{port}/api/artifacts/x")).unwrap()
    }

    #[test]
    fn downloads_and_hashes() {
        let body: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
        let mut resp =
            format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
        resp.extend_from_slice(&body);
        let dest = std::env::temp_dir().join(format!("tfctl-dl-{}", std::process::id()));
        let sum = download(&serve_once(resp), "t", Duration::from_secs(5), &dest).unwrap();
        assert_eq!(sum, crate::sha256::sha256_hex(&body));
        assert_eq!(fs::read(&dest).unwrap(), body);
        fs::remove_file(&dest).unwrap();
        let resp = b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec();
        let e = download(&serve_once(resp), "", Duration::from_secs(5), &dest).unwrap_err();
        assert!(e.to_string().contains("HTTP 404"));
        let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc".to_vec();
        let e = download(&serve_once(resp), "", Duration::from_secs(5), &dest).unwrap_err();
        assert!(e.to_string().contains("short body"), "{e}");
        let _ = fs::remove_file(&dest);
    }

    #[test]
    fn parses_urls() {
        let u = Url::parse("http://nas.local:8080/api/x").unwrap();
        assert_eq!(
            u,
            Url {
                host: "nas.local".into(),
                port: 8080,
                path: "/api/x".into()
            }
        );
        let u = Url::parse("ws://10.0.0.5").unwrap();
        assert_eq!((u.port, u.path.as_str()), (80, "/"));
        assert_eq!(u.host_header(), "10.0.0.5");
        assert!(Url::parse("https://x/").is_none());
        assert!(Url::parse("http://").is_none());
        assert_eq!(u.with_path("/p").path, "/p");
    }

    /// Found by fuzz.rs: `http://[::1]/` (IPv6 literal, no port) was refused, and a host with a
    /// stray `]` and `:` split at the wrong colon, so the URL did not read back the same.
    #[test]
    fn parses_ipv6_literals() {
        let u = Url::parse("http://[::1]/").unwrap();
        assert_eq!((u.host.as_str(), u.port, u.path.as_str()), ("::1", 80, "/"));
        assert_eq!(u.host_header(), "[::1]");
        let u = Url::parse("ws://[fe80::1]:8080").unwrap();
        assert_eq!(
            (u.host.as_str(), u.port, u.path.as_str()),
            ("fe80::1", 8080, "/")
        );
        assert_eq!(u.host_header(), "[fe80::1]:8080");
        for bad in [
            "http://[::1/",
            "http://[::1]x/",
            "http://[]/",
            "http://[::1]:/",
            "http://a]b:c]:9000/",
            "http://[a]b]:1/",
        ] {
            assert!(Url::parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn parses_head() {
        let raw =
            b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}";
        let (status, headers, end) = parse_head(raw).unwrap();
        assert_eq!(status, 200);
        assert_eq!(end, raw.len() - 2);
        assert_eq!(headers[1], ("Content-Length".to_string(), "2".to_string()));
        assert!(parse_head(b"HTTP/1.1 200 OK\r\nX: y\r\n").is_none());
    }
}
