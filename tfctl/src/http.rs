//! Minimal HTTP/1.1 over TcpStream: URL splitting, one GET, response parsing. LAN only, no TLS.
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
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
        let (host, port) = match hostport.rsplit_once(':') {
            Some((h, p)) if !h.contains(']') || h.ends_with(']') => (h, p.parse().ok()?),
            _ => (hostport, 80),
        };
        if host.is_empty() {
            return None;
        }
        Some(Url {
            host: host.trim_matches(['[', ']']).to_string(),
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

    pub fn host_header(&self) -> String {
        if self.port == 80 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
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

#[cfg(test)]
mod tests {
    use super::*;

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
