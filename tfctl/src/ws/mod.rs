//! Minimal RFC 6455 client over TcpStream: upgrade with a bearer token, masked client frames,
//! fragment reassembly, ping/pong, close. The read timeout makes `recv` return `Ok(None)` so the
//! agent loop can tick (heartbeat, keepalive) without a second thread.
use crate::http::{self, Url};
use crate::sha1;
use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, SystemTime};

mod frame;
pub use frame::*;

pub struct Client {
    stream: TcpStream,
    buf: Vec<u8>,
    asm: Assembler,
    rng: u64,
}

impl Client {
    pub fn connect(url: &Url, token: &str, timeout: Duration) -> io::Result<Client> {
        let mut stream = http::connect(url, timeout)?;
        let mut rng = seed();
        let mut nonce = [0u8; 16];
        for c in nonce.chunks_mut(8) {
            rng = xorshift(rng);
            c.copy_from_slice(&rng.to_le_bytes()[..c.len()]);
        }
        let key = sha1::base64(&nonce);
        let mut req = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\nUser-Agent: tfctl\r\n",
            url.path,
            url.host_header()
        );
        if !token.is_empty() {
            req.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        req.push_str("\r\n");
        stream.write_all(req.as_bytes())?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 2048];
        let (status, headers, end) = loop {
            let n = stream.read(&mut chunk)?;
            if n == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "handshake eof",
                ));
            }
            buf.extend_from_slice(&chunk[..n]);
            if let Some(h) = http::parse_head(&buf) {
                break h;
            }
            if buf.len() > 16384 {
                return Err(io::Error::other("handshake headers too long"));
            }
        };
        if status != 101 {
            return Err(io::Error::other(format!("upgrade refused: HTTP {status}")));
        }
        let accept = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("sec-websocket-accept"))
            .map(|(_, v)| v.as_str());
        if accept != Some(sha1::ws_accept(&key).as_str()) {
            return Err(io::Error::other("bad Sec-WebSocket-Accept"));
        }
        buf.drain(..end);
        Ok(Client {
            stream,
            buf,
            asm: Assembler::default(),
            rng,
        })
    }

    /// How long `recv` blocks before returning `Ok(None)`.
    pub fn set_read_timeout(&self, d: Duration) -> io::Result<()> {
        self.stream.set_read_timeout(Some(d))
    }

    fn send(&mut self, opcode: u8, payload: &[u8]) -> io::Result<()> {
        self.rng = xorshift(self.rng);
        let k = (self.rng as u32).to_le_bytes();
        self.stream
            .write_all(&encode(opcode, true, payload, Some(k)))
    }
    pub fn send_text(&mut self, s: &str) -> io::Result<()> {
        self.send(OP_TEXT, s.as_bytes())
    }
    pub fn ping(&mut self) -> io::Result<()> {
        self.send(OP_PING, b"tf")
    }
    pub fn close(&mut self) {
        let _ = self.send(OP_CLOSE, &1000u16.to_be_bytes());
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    /// Next text/binary/pong event (pings are answered here). `Ok(None)` on read timeout;
    /// `Err` on close, EOF or protocol error.
    pub fn recv(&mut self) -> io::Result<Option<Event>> {
        let mut chunk = [0u8; 4096];
        loop {
            while let Some((frame, used)) = decode(&self.buf).map_err(io::Error::other)? {
                self.buf.drain(..used);
                match self.asm.push(frame).map_err(io::Error::other)? {
                    None => {}
                    Some(Event::Ping(p)) => self.send(OP_PONG, &p)?,
                    Some(Event::Close(p)) => {
                        let _ = self.send(OP_CLOSE, &p);
                        return Err(io::Error::new(io::ErrorKind::ConnectionAborted, "closed"));
                    }
                    Some(ev) => return Ok(Some(ev)),
                }
            }
            match self.stream.read(&mut chunk) {
                Ok(0) => return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "eof")),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    return Ok(None)
                }
                Err(e) => return Err(e),
            }
        }
    }
}

fn seed() -> u64 {
    let t = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(1);
    xorshift(t ^ (std::process::id() as u64) << 32 ^ 0x9E37_79B9_7F4A_7C15)
}

fn xorshift(mut x: u64) -> u64 {
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    if x == 0 {
        0x2545_F491_4F6C_DD1D
    } else {
        x
    }
}
