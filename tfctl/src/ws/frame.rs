//! RFC 6455 frame codec and fragment reassembly: pure functions over byte slices, tested with
//! the vectors from section 5.7. The TcpStream client is in ws/mod.rs.

pub const OP_CONT: u8 = 0x0;
pub const OP_TEXT: u8 = 0x1;
pub const OP_BIN: u8 = 0x2;
pub const OP_CLOSE: u8 = 0x8;
pub const OP_PING: u8 = 0x9;
pub const OP_PONG: u8 = 0xA;

/// Largest frame `decode` accepts and largest message `Assembler` joins.
pub const MAX_MESSAGE: usize = 16 << 20;

#[derive(Debug, Clone, PartialEq)]
pub struct Frame {
    pub fin: bool,
    pub opcode: u8,
    pub payload: Vec<u8>,
}

pub fn encode(opcode: u8, fin: bool, payload: &[u8], mask: Option<[u8; 4]>) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(if fin { 0x80 } else { 0 } | (opcode & 0x0F));
    let m = if mask.is_some() { 0x80 } else { 0 };
    match payload.len() {
        n if n < 126 => out.push(m | n as u8),
        n if n < 65536 => {
            out.push(m | 126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(m | 127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    match mask {
        Some(k) => {
            out.extend_from_slice(&k);
            out.extend(payload.iter().enumerate().map(|(i, b)| b ^ k[i % 4]));
        }
        None => out.extend_from_slice(payload),
    }
    out
}

/// Decodes one frame from the front of `buf`. `Ok(None)` when more bytes are needed.
pub fn decode(buf: &[u8]) -> Result<Option<(Frame, usize)>, String> {
    if buf.len() < 2 {
        return Ok(None);
    }
    if buf[0] & 0x70 != 0 {
        return Err("reserved bits set".into());
    }
    let fin = buf[0] & 0x80 != 0;
    let opcode = buf[0] & 0x0F;
    let masked = buf[1] & 0x80 != 0;
    let (len, mut off) = match buf[1] & 0x7F {
        126 => {
            if buf.len() < 4 {
                return Ok(None);
            }
            (u16::from_be_bytes([buf[2], buf[3]]) as usize, 4)
        }
        127 => {
            if buf.len() < 10 {
                return Ok(None);
            }
            let n = u64::from_be_bytes(buf[2..10].try_into().unwrap());
            if n > MAX_MESSAGE as u64 {
                return Err("frame too large".into());
            }
            (n as usize, 10)
        }
        n => (n as usize, 2),
    };
    let key = if masked {
        if buf.len() < off + 4 {
            return Ok(None);
        }
        let k = [buf[off], buf[off + 1], buf[off + 2], buf[off + 3]];
        off += 4;
        Some(k)
    } else {
        None
    };
    if buf.len() < off + len {
        return Ok(None);
    }
    let mut payload = buf[off..off + len].to_vec();
    if let Some(k) = key {
        for (i, b) in payload.iter_mut().enumerate() {
            *b ^= k[i % 4];
        }
    }
    Ok(Some((
        Frame {
            fin,
            opcode,
            payload,
        },
        off + len,
    )))
}

#[derive(Debug, PartialEq)]
pub enum Event {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Pong,
    Close(Vec<u8>),
}

/// Joins continuation frames; control frames pass straight through (they may interleave).
#[derive(Default)]
pub struct Assembler {
    frag: Option<(u8, Vec<u8>)>,
}

impl Assembler {
    pub fn push(&mut self, f: Frame) -> Result<Option<Event>, String> {
        match f.opcode {
            OP_PING => return Ok(Some(Event::Ping(f.payload))),
            OP_PONG => return Ok(Some(Event::Pong)),
            OP_CLOSE => return Ok(Some(Event::Close(f.payload))),
            OP_TEXT | OP_BIN if self.frag.is_some() => {
                return Err("new message mid-fragment".into())
            }
            OP_CONT if self.frag.is_none() => return Err("continuation without start".into()),
            OP_TEXT | OP_BIN | OP_CONT => {}
            op => return Err(format!("unknown opcode {op}")),
        }
        let (op, mut data) = match self.frag.take() {
            Some((op, mut d)) => {
                // Each frame is capped by `decode`; the message they add up to is capped here.
                if d.len() + f.payload.len() > MAX_MESSAGE {
                    return Err("message too large".into());
                }
                d.extend_from_slice(&f.payload);
                (op, d)
            }
            None => (f.opcode, f.payload),
        };
        if !f.fin {
            self.frag = Some((op, data));
            return Ok(None);
        }
        if op == OP_TEXT {
            let s = String::from_utf8(std::mem::take(&mut data)).map_err(|_| "non-utf8 text")?;
            return Ok(Some(Event::Text(s)));
        }
        Ok(Some(Event::Binary(data)))
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn dec(b: &[u8]) -> (Frame, usize) {
        decode(b).unwrap().unwrap()
    }

    #[test]
    fn rfc6455_5_7_vectors() {
        // unmasked "Hello"
        let raw = [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
        assert_eq!(encode(OP_TEXT, true, b"Hello", None), raw);
        let (f, n) = dec(&raw);
        assert_eq!(
            (f.fin, f.opcode, f.payload.as_slice(), n),
            (true, OP_TEXT, &b"Hello"[..], 7)
        );
        // masked "Hello"
        let masked = [
            0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ];
        assert_eq!(
            encode(OP_TEXT, true, b"Hello", Some([0x37, 0xfa, 0x21, 0x3d])),
            masked
        );
        assert_eq!(dec(&masked).0.payload, b"Hello");
        // ping / masked pong
        let ping = [0x89, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
        assert_eq!(dec(&ping).0.opcode, OP_PING);
        let pong = [
            0x8a, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
        ];
        assert_eq!(
            encode(OP_PONG, true, b"Hello", Some([0x37, 0xfa, 0x21, 0x3d])),
            pong
        );
        // 256 bytes binary, 16-bit length
        let data = vec![0xAB; 256];
        let enc = encode(OP_BIN, true, &data, None);
        assert_eq!(&enc[..4], &[0x82, 0x7E, 0x01, 0x00]);
        assert_eq!(dec(&enc).0.payload, data);
        // 64 KiB binary, 64-bit length
        let data = vec![0xCD; 65536];
        let enc = encode(OP_BIN, true, &data, None);
        assert_eq!(&enc[..10], &[0x82, 0x7F, 0, 0, 0, 0, 0, 1, 0, 0]);
        let (f, n) = dec(&enc);
        assert_eq!((f.payload.len(), n), (65536, enc.len()));
    }

    #[test]
    fn incomplete_frames_wait_and_bad_rsv_fails() {
        assert_eq!(decode(&[0x81]).unwrap(), None);
        assert_eq!(decode(&[0x81, 0x05, 0x48]).unwrap(), None);
        assert_eq!(decode(&[0x81, 0x85, 0x37, 0xfa]).unwrap(), None);
        assert!(decode(&[0xF1, 0x00]).is_err());
    }

    #[test]
    fn fragments_reassemble_with_interleaved_ping() {
        let mut a = Assembler::default();
        let (f1, _) = dec(&[0x01, 0x03, 0x48, 0x65, 0x6c]);
        let (p, _) = dec(&[0x89, 0x00]);
        let (f2, _) = dec(&[0x80, 0x02, 0x6c, 0x6f]);
        assert_eq!(a.push(f1).unwrap(), None);
        assert_eq!(a.push(p).unwrap(), Some(Event::Ping(vec![])));
        assert_eq!(a.push(f2).unwrap(), Some(Event::Text("Hello".into())));
        let (close, _) = dec(&encode(OP_CLOSE, true, &1000u16.to_be_bytes(), None));
        assert_eq!(a.push(close).unwrap(), Some(Event::Close(vec![0x03, 0xE8])));
        let (lone_cont, _) = dec(&[0x80, 0x00]);
        assert!(a.push(lone_cont).is_err());
    }

    /// Each frame was capped at 16 MiB but a message of endless continuation frames grew
    /// without bound (the fuzz review in fuzz.rs).
    #[test]
    fn fragmented_message_is_capped() {
        let mut a = Assembler::default();
        let mib = vec![0u8; 1 << 20];
        let frame = |opcode, fin| Frame {
            fin,
            opcode,
            payload: mib.clone(),
        };
        assert_eq!(a.push(frame(OP_BIN, false)).unwrap(), None);
        for _ in 1..16 {
            assert_eq!(a.push(frame(OP_CONT, false)).unwrap(), None);
        }
        let e = a.push(frame(OP_CONT, true)).unwrap_err();
        assert!(e.contains("too large"), "{e}");
        // Exactly 16 MiB still goes through.
        let mut a = Assembler::default();
        assert_eq!(a.push(frame(OP_BIN, false)).unwrap(), None);
        for _ in 1..15 {
            assert_eq!(a.push(frame(OP_CONT, false)).unwrap(), None);
        }
        match a.push(frame(OP_CONT, true)).unwrap() {
            Some(Event::Binary(d)) => assert_eq!(d.len(), MAX_MESSAGE),
            other => panic!("{other:?}"),
        }
    }
}
