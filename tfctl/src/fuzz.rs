//! Property and fuzz harness for every hand-rolled parser, run by `cargo test` (test builds
//! only; nothing here reaches the binary). No crates: a xorshift64 PRNG, a mutator over valid
//! seed inputs, and a few structured generators. Each target asserts only "no panic, and the
//! output invariants hold". A failure prints the target and the input that triggered it.
//!
//! Fixed seeds, so a run is reproducible. `TFCTL_FUZZ_ITERS=N` scales every target (the
//! default keeps the whole suite to a few seconds); `TFCTL_FUZZ_SEED=N` explores other inputs.
use crate::json::{self, Value};
use crate::{b64, config, http, png, provision, sha1, sha256, slot, touchfw, update, wifi, ws};
use std::fmt::Debug;
use std::panic::{catch_unwind, AssertUnwindSafe};

pub struct Rng(u64);

impl Rng {
    pub fn new(salt: u64) -> Rng {
        let base = std::env::var("TFCTL_FUZZ_SEED")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0x9E37_79B9_7F4A_7C15u64);
        Rng((base ^ salt.wrapping_mul(0x2545_F491_4F6C_DD1D)) | 1)
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    /// 0..n (n > 0).
    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// True one time in `k`.
    pub fn one_in(&mut self, k: usize) -> bool {
        self.below(k) == 0
    }
    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
    pub fn bytes(&mut self, max: usize) -> Vec<u8> {
        let n = self.below(max + 1);
        (0..n).map(|_| self.next() as u8).collect()
    }
    /// A u32 that is often an edge value: lengths, offsets and counts in binary headers.
    pub fn edge_u32(&mut self) -> u32 {
        match self.below(6) {
            0 => *self.pick(&[0, 1, 0x7f, 0x80, 0xff, 0xffff, 0x8000_0000, 0xffff_fff0]),
            1 => u32::MAX - self.below(64) as u32,
            2 => self.below(1 << 16) as u32,
            _ => self.next() as u32,
        }
    }

    /// 1..=4 mutations of `seed`: bit flips, byte sets, truncation, deletion, duplication,
    /// insertion of special bytes, edge-value overwrites. Output capped at `seed.len() * 2 + 64`.
    pub fn mutate(&mut self, seed: &[u8]) -> Vec<u8> {
        let mut v = seed.to_vec();
        let cap = seed.len() * 2 + 64;
        for _ in 0..1 + self.below(4) {
            let n = v.len();
            match self.below(9) {
                0 if n > 0 => {
                    let i = self.below(n);
                    v[i] ^= 1 << self.below(8);
                }
                1 if n > 0 => {
                    let i = self.below(n);
                    v[i] = self.next() as u8;
                }
                2 if n > 0 => v.truncate(self.below(n)),
                3 if n > 0 => {
                    let i = self.below(n);
                    let j = (i + 1 + self.below(16)).min(n);
                    v.drain(i..j);
                }
                4 if n > 0 => {
                    let i = self.below(n);
                    let j = (i + 1 + self.below(64)).min(n);
                    let piece = v[i..j].to_vec();
                    let at = self.below(n + 1);
                    v.splice(at..at, piece);
                }
                5 | 6 => {
                    let s = *self.pick(SPECIAL);
                    let at = self.below(n + 1);
                    v.splice(at..at, s.iter().copied());
                }
                7 if n >= 4 => {
                    let i = self.below(n - 3);
                    let e = self.edge_u32();
                    let b = if self.one_in(2) {
                        e.to_le_bytes()
                    } else {
                        e.to_be_bytes()
                    };
                    v[i..i + 4].copy_from_slice(&b);
                }
                _ => {
                    let at = self.below(n + 1);
                    v.insert(at, self.next() as u8);
                }
            }
            v.truncate(cap);
        }
        v
    }

    /// A mutated seed (or, now and then, pure noise) as a string.
    pub fn text(&mut self, seeds: &[&str]) -> String {
        let b = if self.one_in(16) {
            self.bytes(256)
        } else {
            let s = self.pick(seeds).as_bytes();
            self.mutate(s)
        };
        String::from_utf8_lossy(&b).into_owned()
    }

    pub fn bin(&mut self, seeds: &[&[u8]]) -> Vec<u8> {
        if self.one_in(16) {
            self.bytes(512)
        } else {
            let s = *self.pick(seeds);
            self.mutate(s)
        }
    }

    /// A short string drawn from tricky characters.
    pub fn word(&mut self, max: usize) -> String {
        const CH: &[char] = &[
            'a',
            'Z',
            '0',
            '9',
            ' ',
            '"',
            '\\',
            '/',
            '\n',
            '\r',
            '\t',
            '\0',
            '\u{1}',
            '\u{1f}',
            '\u{7f}',
            '=',
            ':',
            '#',
            '[',
            ']',
            '{',
            '}',
            '-',
            '.',
            'é',
            '€',
            '\u{1F600}',
            '\u{FFFD}',
            '\u{2028}',
            '\u{85}',
        ];
        let n = self.below(max + 1);
        (0..n).map(|_| *self.pick(CH)).collect()
    }
}

const SPECIAL: &[&[u8]] = &[
    b"\"",
    b"\\",
    b"\\\\",
    b"{",
    b"[",
    b"}",
    b"]",
    b"{[",
    b"\r\n",
    b"\n",
    b"\r\n\r\n",
    b"\0",
    b"\xff",
    b"\xfe\xff",
    b"%",
    b"%00",
    b"=",
    b"==",
    b"\\u",
    b"\\uD800",
    b"\\uDBFF\\u0000",
    b"\\uDC00",
    b":",
    b"/",
    b"#",
    b"'",
    b" ",
    b"\t",
    b"\x1b[",
    b"\x1b[1;90m",
    b"-",
    b"0",
    b"9",
    b"e9999",
    b"-2147483648",
    b"4294967295",
    b"\xe2\x82\xac",
    b"\xf0\x9f\x98\x80",
    b"[[[[[[[[",
];

/// Iterations for a target whose default is `n`.
fn iters(n: usize) -> usize {
    let scale = std::env::var("TFCTL_FUZZ_ITERS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    match scale {
        Some(k) => (n * k / 20_000).max(1),
        None => n,
    }
}

/// Run one case; on a panic, fail naming the target and the input.
fn check<T: Debug>(target: &str, input: &T, f: impl FnOnce(&T)) {
    if catch_unwind(AssertUnwindSafe(|| f(input))).is_err() {
        panic!("fuzz {target}: panicked or broke an invariant on input {input:?}");
    }
}

// ---- json -------------------------------------------------------------------------------

const JSON_SEEDS: &[&str] = &[
    r#"{"type":"config","url":"http://nas:8080/frame/k","rotation":3,"brightness":80,"screen_off":"23:00","screen_on":null,"page_version":"7","ok":true,"list":[1,2.5,-3e2],"nested":{"a":[]}}"#,
    r#""a\"b\\c\né😀""#,
    r#"[{"a":[{"b":[[1,{"c":"\u0000"}]]}]},-0.0,1e308,"\/"]"#,
    r#"{"type":"action","id":42,"name":"screen","value":"off"}"#,
    "[]",
    "-12.5e-3",
];

fn rand_value(r: &mut Rng, depth: usize) -> Value {
    match r.below(if depth == 0 { 4 } else { 6 }) {
        0 => Value::Null,
        1 => Value::Bool(r.one_in(2)),
        2 => {
            let n = match r.below(4) {
                0 => r.next() as i64 as f64,
                1 => (r.next() as i32) as f64 / 7.0,
                2 => f64::from_bits(r.next()),
                _ => r.below(1000) as f64,
            };
            Value::Num(if n.is_finite() { n } else { 0.5 })
        }
        3 => Value::Str(r.word(12)),
        4 => Value::Arr((0..r.below(4)).map(|_| rand_value(r, depth - 1)).collect()),
        _ => Value::Obj(
            (0..r.below(4))
                .map(|_| (r.word(6), rand_value(r, depth - 1)))
                .collect(),
        ),
    }
}

#[test]
fn fuzz_json() {
    let mut r = Rng::new(1);
    for _ in 0..iters(20_000) {
        let t = r.text(JSON_SEEDS);
        check("json::parse", &t, |t| {
            if let Ok(v) = json::parse(t) {
                assert_eq!(json::parse(&v.to_string()).as_ref(), Ok(&v));
            }
        });
    }
    for _ in 0..iters(5_000) {
        let v = rand_value(&mut r, 5);
        check("json round trip", &v, |v| {
            assert_eq!(json::parse(&v.to_string()).as_ref(), Ok(v));
        });
    }
}

// ---- http -------------------------------------------------------------------------------

const HEAD_SEEDS: &[&[u8]] = &[
    b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}",
    b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nSec-WebSocket-Accept: x\r\n\r\n",
    b"HTTP/1.0 404\r\n\r\n",
];

const URL_SEEDS: &[&str] = &[
    "http://nas.local:8080/api/x",
    "ws://10.0.0.5",
    "http://[::1]/",
    "http://[fe80::1%25en0]:9000/ws?x=1",
    "ws://host:65535/a/b",
    "http://a/",
];

#[test]
fn fuzz_http() {
    let mut r = Rng::new(2);
    for _ in 0..iters(20_000) {
        let b = r.bin(HEAD_SEEDS);
        check("http::parse_head", &b, |b| {
            if let Some((_, headers, end)) = http::parse_head(b) {
                assert!(end <= b.len() && b[..end].ends_with(b"\r\n\r\n"));
                assert!(headers.len() <= end / 2);
            }
        });
        let u = r.text(URL_SEEDS);
        check("http::Url::parse", &u, |u| {
            if let Some(url) = http::Url::parse(u) {
                assert!(!url.host.is_empty() && url.path.starts_with('/'));
                let again = format!("http://{}{}", url.host_header(), url.path);
                assert_eq!(http::Url::parse(&again), Some(url));
            }
        });
    }
}

// ---- websocket --------------------------------------------------------------------------

#[test]
fn fuzz_ws_codec() {
    let mut r = Rng::new(3);
    let seeds: Vec<Vec<u8>> = vec![
        ws::encode(ws::OP_TEXT, true, b"Hello", Some([0x37, 0xfa, 0x21, 0x3d])),
        ws::encode(ws::OP_BIN, false, &[0xAB; 300], None),
        ws::encode(ws::OP_PING, true, b"", None),
        vec![0x82, 0x7F, 0, 0, 0, 0, 0, 1, 0, 0],
    ];
    let seeds: Vec<&[u8]> = seeds.iter().map(Vec::as_slice).collect();
    for _ in 0..iters(20_000) {
        // Encode/decode round trip, with trailing bytes that must stay unread.
        let op = r.below(16) as u8;
        let fin = r.one_in(2);
        let mut payload = r.bytes(200);
        if r.one_in(50) {
            payload.resize(65_536 + r.below(10), 7);
        }
        let mask = r.one_in(2).then(|| (r.next() as u32).to_le_bytes());
        let mut enc = ws::encode(op, fin, &payload, mask);
        let n = enc.len();
        enc.extend(r.bytes(8));
        let (f, used) = ws::decode(&enc).unwrap().unwrap();
        assert_eq!((f.fin, f.opcode, used), (fin, op, n));
        assert_eq!(f.payload, payload);
        for cut in [0, n / 2, n - 1] {
            assert_eq!(ws::decode(&enc[..cut]), Ok(None), "short frame must wait");
        }
        // Garbage: no panic, never claims more than it was given.
        let b = r.bin(&seeds);
        check("ws::decode", &b, |b| {
            if let Ok(Some((f, used))) = ws::decode(b) {
                assert!(used <= b.len() && f.payload.len() <= used);
            }
        });
    }
}

#[test]
fn fuzz_ws_assembler() {
    let mut r = Rng::new(4);
    for _ in 0..iters(5_000) {
        // A stream of whole messages, each split into 1..4 fragments with control frames
        // between them, cut into random TCP-sized pieces: the events come out in order.
        let mut wire = Vec::new();
        let mut want = Vec::new();
        for _ in 0..1 + r.below(4) {
            let text = r.one_in(2);
            let body = if text {
                r.word(40).into_bytes()
            } else {
                r.bytes(100)
            };
            let parts = 1 + r.below(4);
            let step = body.len() / parts + 1;
            let chunks: Vec<&[u8]> = if body.is_empty() {
                vec![&body[..]]
            } else {
                body.chunks(step).collect()
            };
            for (i, c) in chunks.iter().enumerate() {
                let op = match (i, text) {
                    (0, true) => ws::OP_TEXT,
                    (0, false) => ws::OP_BIN,
                    _ => ws::OP_CONT,
                };
                wire.extend(ws::encode(op, i + 1 == chunks.len(), c, None));
                if r.one_in(3) {
                    wire.extend(ws::encode(ws::OP_PING, true, b"p", None));
                    want.push(ws::Event::Ping(b"p".to_vec()));
                }
            }
            want.push(if text {
                ws::Event::Text(String::from_utf8(body.clone()).unwrap())
            } else {
                ws::Event::Binary(body.clone())
            });
        }
        // A ping after a final fragment comes out after that message, so the messages and
        // the pings are compared as two sequences.
        let mut buf = Vec::new();
        let mut asm = ws::Assembler::default();
        let mut got = Vec::new();
        let mut rest = &wire[..];
        while !rest.is_empty() || !buf.is_empty() {
            let take = (1 + r.below(64)).min(rest.len());
            buf.extend_from_slice(&rest[..take]);
            rest = &rest[take..];
            while let Some((f, used)) = ws::decode(&buf).unwrap() {
                buf.drain(..used);
                if let Some(ev) = asm.push(f).unwrap() {
                    got.push(ev);
                }
            }
            if rest.is_empty() {
                assert!(buf.is_empty(), "whole frames leave nothing behind");
            }
        }
        let msgs = |v: &[ws::Event]| {
            v.iter()
                .filter(|e| !matches!(e, ws::Event::Ping(_)))
                .map(|e| format!("{e:?}"))
                .collect::<Vec<_>>()
        };
        assert_eq!(msgs(&got), msgs(&want));
        assert_eq!(got.len(), want.len());
    }
    // Random frames: errors are fine, panics are not.
    for _ in 0..iters(20_000) {
        let frames: Vec<ws::Frame> = (0..1 + r.below(8))
            .map(|_| ws::Frame {
                fin: r.one_in(2),
                opcode: *r.pick(&[0, 0, 1, 2, 3, 8, 9, 10, 15]),
                payload: r.bytes(32),
            })
            .collect();
        check("ws::Assembler::push", &frames, |frames| {
            let mut asm = ws::Assembler::default();
            for f in frames {
                let _ = asm.push(f.clone());
            }
        });
    }
}

// ---- base64, sha --------------------------------------------------------------------------

#[test]
fn fuzz_b64() {
    let mut r = Rng::new(5);
    for _ in 0..iters(20_000) {
        let data = r.bytes(100);
        let enc = sha1::base64(&data);
        assert_eq!(b64::decode(&enc).as_deref(), Some(&data[..]));
        let t = r.text(&[&enc, "Zm9vYg==", "iVBO\nRw0K\r\nGgo="]);
        check("b64::decode", &t, |t| {
            if let Some(out) = b64::decode(t) {
                assert!(out.len() <= t.len() / 4 * 3);
            }
        });
    }
}

#[test]
fn sha_known_vectors_around_block_boundaries() {
    // Python hashlib over bytes (i * 7 + 3) % 256 for i in 0..n: the lengths where the padding
    // spills into a second block (55/56, 119/120) and the exact block multiples.
    const V: &[(usize, &str, &str)] = &[
        (
            55,
            "ddf57317ef34bfee3b6df83d359098930eb278bc",
            "e7313d333c272e639f790978283f9eb392e843d0f29b7016828bb1daa4aac70b",
        ),
        (
            56,
            "a0d492bb0fc889d0eca3bc137066ab6f4f74f369",
            "4324d65f3c103567f5589c710bc08f8523f929a9272e3af36fc968e52abc6c27",
        ),
        (
            57,
            "11a02dcf95859677a62e75024067c22b165d890f",
            "35df609437dcfea3279283ab79fd554e2bf78f8f7ae2de532d8ee300b09e8f73",
        ),
        (
            63,
            "c55856749bef509bdfe6bfebfc7bf4e793e82132",
            "81c80242132f230c3bd41b3e63bbcff16107339549214a99614ff26664625055",
        ),
        (
            64,
            "bede92be29c3874e1b54ddc77988d606fc857a8e",
            "39e3d7b6b5d075d37d053ad89b24b41bef4f3c29760c84447cab3f3be1882241",
        ),
        (
            65,
            "b05a80522b053d6dc7e0a517d0e70212c7dad11f",
            "aacca6ff74fdbb296d165a45cecfa04e5127bc008770fbbdd48006f2d2fae95e",
        ),
        (
            119,
            "504e27376a6e0f0dba8295b85cb25dc4dfa17d23",
            "9ce7368e4daf32341631b492e80359dc9f594b48453cd0dd5bf0b19279cc177e",
        ),
        (
            120,
            "82134b02fb3f702491be9bed581eeab59334acb2",
            "7836b787757e95e58b3ca5aec90b1b004e8deba1e50e9675af9cabf1a13a04b5",
        ),
        (
            121,
            "f004a54c50aa3705ccc2f03f21016ca6a25755cb",
            "1189a98a00c71bc1848ea8bdc9700b442bee0be7c3f45172303f1ab0b6f1617e",
        ),
        (
            127,
            "34d5e582029e9b9b85b2febe31da3db7cdabaaea",
            "a8d23e75d936f303d248888d9b165ee543f4cbafcad3c9dd2a79bd84faa11d07",
        ),
        (
            128,
            "a09133e6730ffe899efb70204cb5646cd5dc24ee",
            "d2742f1f4ac6bb7ca2b239ee18402ba8b3f9f8e652d2a72973c2b9ba11c08cf6",
        ),
    ];
    let mut r = Rng::new(6);
    for &(n, s1, s256) in V {
        let d: Vec<u8> = (0..n).map(|i| ((i * 7 + 3) % 256) as u8).collect();
        assert_eq!(sha256::hex(&sha1::sha1(&d)), s1, "sha1 len {n}");
        assert_eq!(sha256::sha256_hex(&d), s256, "sha256 len {n}");
        // Streaming in random pieces gives the one-shot digest.
        for _ in 0..50 {
            let mut h = sha256::Sha256::new();
            let mut rest = &d[..];
            while !rest.is_empty() {
                let k = r.below(rest.len() + 1);
                h.update(&rest[..k]);
                rest = &rest[k..];
            }
            assert_eq!(h.finish_hex(), s256, "sha256 streamed len {n}");
        }
    }
}

// ---- png / inflate ------------------------------------------------------------------------

/// A 4x3 RGBA PNG made by zlib (fixed/dynamic Huffman, Sub/Up/Paeth filters).
const RGBA_HEX: &str = "89504e470d0a1a0a0000000d4948445200000004000000030806000000b4f4aec60000003c4944415478da63745168b2d9792cbf2dcf8bc327662dc7352697d04d6942aa324e1176f7195631ff99cca2c168e3d9cab0507f8e666a796563dc7300cef0126b1de370680000000049454e44ae426082";

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

#[test]
fn fuzz_png() {
    let mut r = Rng::new(7);
    let rgba = unhex(RGBA_HEX);
    let stored = png::encode_rgb(3, 2, &[0x112233, 0x445566, 0x778899, 0xaabbcc, 0, 0xffffff]);
    // The zlib payload of the RGBA file (skip the 2-byte zlib header), for inflate alone.
    let deflate = rgba[8 + 25 + 8 + 2..8 + 25 + 8 + 0x3c].to_vec();
    for _ in 0..iters(20_000) {
        let b = r.bin(&[&rgba, &stored]);
        check("png::decode", &b, |b| {
            if let Ok(img) = png::decode(b) {
                assert_eq!(img.px.len(), img.w * img.h);
                assert!(img.w * img.h <= png::MAX_PIXELS);
            }
        });
        let mut d = r.bin(&[&deflate]);
        if r.one_in(4) && !d.is_empty() {
            d[0] = (d[0] & !7) | 0b101; // final dynamic block: random code-length tables
        }
        let limit = 1 + r.below(4096);
        check("png::inflate", &d, |d| {
            if let Ok(out) = png::inflate(d, limit) {
                assert!(out.len() <= limit);
            }
        });
    }
    for _ in 0..iters(2_000) {
        let (w, h) = (1 + r.below(40), 1 + r.below(40));
        let px: Vec<u32> = (0..w * h).map(|_| r.next() as u32 & 0xff_ffff).collect();
        let img = png::decode(&png::encode_rgb(w, h, &px)).unwrap();
        assert_eq!((img.w, img.h), (w, h));
        assert!(img.px.iter().zip(&px).all(|(a, b)| *a == 0xff00_0000 | b));
    }
}

// ---- slot / config / provision -------------------------------------------------------------

const UENV_SEEDS: &[&str] = &[
    "# comment\nslot=a\ntries=0\n#pf_mode=auto\nslot=b\n",
    "pf_mode=ums\n",
    "slot = b\ntries = 2\nbootcmd=run x\0",
    "",
];

#[test]
fn fuzz_slot() {
    let mut r = Rng::new(8);
    for _ in 0..iters(20_000) {
        let t = r.text(UENV_SEEDS);
        let (s, tries) = (
            *r.pick(&[slot::Slot::A, slot::Slot::B]),
            r.next() as u32 % 10,
        );
        check("slot::UEnv", &t, |t| {
            let u = slot::UEnv::parse(t);
            let again = slot::UEnv::parse(&u.render(s, tries));
            assert_eq!((again.slot, again.tries), (Some(s), tries));
            // Other lines survive (a lone "\r" line may lose its "\r": str::lines).
            let others = |t: &str| {
                t.lines()
                    .filter(|l| !l.contains("slot") && !l.contains("tries"))
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .count()
            };
            assert_eq!(
                others(&u.render(s, tries)),
                others(t.trim_end_matches('\0'))
            );
        });
        let c = r.text(&[
            "root=PARTUUID=50463037-02 ro quiet",
            "root=/dev/mmcblk0p3 tf.slot=b",
        ]);
        check("slot::from_cmdline", &c, |c| {
            let _ = slot::from_cmdline(c);
        });
    }
}

const CONF_SEEDS: &[&str] = &[
    "# x\nFRAME_URL=\"http://a/\"\nCOG_ROTATION=1\nCOG_EXTRA_ARGS=\"--a --b\"\nBRIGHTNESS=80\n",
    "SERVER_URL=http://nas:8080/\nFRAME_ID=kitchen\nSPLASH_BG=\"#102030\"\nPOLL_INTERVAL_S=5\n",
    "COG_ROTATION=200\nBRIGHTNESS=999\nHEALTH_INTERVAL_S=-1\n",
];

#[test]
fn fuzz_config() {
    let mut r = Rng::new(9);
    let keys = ["FRAME_ID", "FRAME_TOKEN", "SPLASH_TEXT", "SCREEN_OFF", "TZ"];
    for _ in 0..iters(20_000) {
        let t = r.text(CONF_SEEDS);
        let (k, v) = (*r.pick(&keys), r.word(10));
        check("config::parse", &(&t, k, &v), |(t, k, v)| {
            let c = config::parse(t);
            assert!(c.rotation < 4 && c.brightness <= 100);
            assert!(c.splash_bg <= 0xff_ffff);
            // An override reads back as written (minus the characters it cannot hold).
            let c = config::parse(&config::apply_overrides(t, &[(*k, v.as_str())]));
            let got = match *k {
                "FRAME_ID" => c.frame_id,
                "FRAME_TOKEN" => c.token,
                "SPLASH_TEXT" => c.splash_text,
                "SCREEN_OFF" => c.screen_off,
                _ => c.tz,
            };
            assert_eq!(got, v.replace(['"', '\n', '\r'], ""));
        });
    }
}

const PROVISION_SEEDS: &[&str] = &[
    "WIFI_SSID=\"Home Net\"\nWIFI_PSK='s3cret pass'\nSERVER_URL=http://nas:8080//\nFRAME_ID=kitchen\nFRAME_TOKEN=abc\nCOG_ROTATION=1\nSSH_PUBKEY=ssh-ed25519 AAAA x@y\n",
    "# comment\nFRAME_ID=-bad-\nCOG_ROTATION=7\nSSH_PUBKEY=rsa nope\n",
];

#[test]
fn fuzz_provision() {
    let mut r = Rng::new(10);
    for _ in 0..iters(20_000) {
        let t = r.text(PROVISION_SEEDS);
        check("provision::parse", &t, |t| {
            let p = provision::parse(t);
            assert!(p.rotation.is_none_or(|r| r < 4));
            assert!(p.frame_id.as_deref().is_none_or(provision::valid_hostname));
            assert!(p.server_url.as_deref().is_none_or(|s| !s.ends_with('/')));
            assert!(p
                .ssh_pubkey
                .as_deref()
                .is_none_or(|k| k.starts_with("ssh-") || k.starts_with("ecdsa-")));
        });
    }
}

// ---- wifi ---------------------------------------------------------------------------------

const NETS: &str = "\x1b[1m                               Available networks\x1b[0m\n\
    --------------------------------------------------------------------------------\n\
    \x1b[1;90m      Network name                      Security            Signal\x1b[0m\n\
    --------------------------------------------------------------------------------\n\
    \x1b[1;90m  > \x1b[0m  Groovy Wifi                       psk                 -3000\n\
    \x1b[0m      Cafe                              open                -8100\n\
    \x1b[0m      Old                               wep (unsupported)   -7000\n\
    \x1b[0m      Office  2                         8021x               -6000\n";

const SHOW: &str = "\x1b[1m Station: wlan0 \x1b[0m\n  Settable  Property  Value\n  \
    State                 connected\n  Connected network     Groovy Wifi\n";

#[test]
fn fuzz_wifi() {
    let mut r = Rng::new(11);
    let header = "      Network name                      Security            Signal\n";
    for _ in 0..iters(20_000) {
        let t = if r.one_in(3) {
            // Structured rows with edge-value signals.
            let mut t = header.to_string();
            for _ in 0..r.below(4) {
                let any = r.next() as i32;
                let sig = *r.pick(&[i32::MIN, i32::MAX, -201, 201, -9000, 0, any]);
                let sec = *r.pick(&["psk", "open", "wep (unsupported)", ""]);
                t.push_str(&format!("  > {:<32}  {sec}  {sig}\n", r.word(8)));
            }
            t
        } else {
            r.text(&[NETS, SHOW])
        };
        check("wifi::parse_networks", &t, |t| {
            let nets = wifi::parse_networks(t);
            for (i, n) in nets.iter().enumerate() {
                assert!(!n.ssid.trim().is_empty());
                assert!((1..=4).contains(&n.bars()));
                assert!(!nets[..i]
                    .iter()
                    .any(|m| m.ssid == n.ssid && m.security == n.security));
            }
            let _ = wifi::parse_state(t);
            assert!(!wifi::strip_ansi(t).contains("\x1b["));
        });
        let ssid = r.word(12);
        check("wifi::profile_name", &ssid, |s| {
            if wifi::valid_ssid(s) {
                assert_eq!(
                    wifi::ssid_of(&wifi::profile_name(s)).as_deref(),
                    Some(s.as_str())
                );
            }
            let _ = wifi::ssid_of(s);
            let _ = wifi::escape_value(s);
        });
    }
}

// ---- agent config -------------------------------------------------------------------------

#[test]
fn fuzz_agent_config() {
    use crate::agent::Agent;
    use std::sync::mpsc;
    let mut r = Rng::new(12);
    let base = std::env::temp_dir().join(format!("tfctl-fuzz-agent-{}", std::process::id()));
    let conf = base.join("thinframe.conf");
    std::fs::create_dir_all(&base).unwrap();
    let (tx, _rx) = mpsc::channel();
    let cfg = config::parse("SERVER_URL=http://nas:8080\nFRAME_ID=k\n");
    let mut a = Agent::new(&cfg, tx, conf.to_str().unwrap()).unwrap();
    a.paths = update::Paths::under(&base);
    let keys = [
        "url",
        "rotation",
        "brightness",
        "screen_off",
        "screen_on",
        "schedule",
        "page_version",
        "splash_text",
        "splash_bg",
        "splash_png",
        "software_version",
        "artifacts",
    ];
    let png64 = sha1::base64(&unhex(RGBA_HEX));
    // Each changed key is persisted with an fsync: fewer rounds than the pure parsers.
    for _ in 0..iters(300) {
        let mut kv = Vec::new();
        for _ in 0..1 + r.below(5) {
            let k = *r.pick(&keys);
            let v = match r.below(6) {
                0 => Value::Str(r.word(8)),
                1 => Value::Num(r.next() as i8 as f64),
                2 => Value::Str(png64.clone()),
                3 => Value::obj().set(&r.word(3), r.word(70)),
                4 => Value::Str(format!("{}", r.below(300))),
                _ => rand_value(&mut r, 2),
            };
            kv.push((k.to_string(), v));
        }
        let v = Value::Obj(kv);
        check("Agent::apply_config", &v, |v| {
            a.apply_config(v);
            assert!(a.cfg.rotation < 4 && a.cfg.brightness <= 100);
            let saved = config::load(conf.to_str().unwrap()).unwrap();
            assert!(saved.rotation < 4 && saved.brightness <= 100);
            for (file, sum) in a.artifacts.0.values() {
                assert!(!file.is_empty() && !file.contains('/') && sum.len() == 64);
            }
        });
    }
    std::fs::remove_dir_all(&base).unwrap();
}

// ---- touch firmware -----------------------------------------------------------------------

#[test]
fn fuzz_touchfw() {
    use touchfw::tests::{boot_image, cpio, gzip, module, page_fw};
    let mut r = Rng::new(13);
    let ko = module(&page_fw());
    let arch = cpio(&[("init", b"#!"), ("lib/modules/gslX680new.ko", &ko)]);
    let gz = gzip(&arch);
    let boot = boot_image(&gz);
    for _ in 0..iters(20_000) {
        let m = r.bin(&[&ko]);
        check("touchfw::table_from_module", &m, |m| {
            if let Ok(t) = touchfw::table_from_module(m, "gsl_m86_hd") {
                assert!(t.len() <= m.len());
            }
            let _ = touchfw::layout_ok(m);
        });
        let a = r.bin(&[&arch]);
        check("touchfw::cpio_find", &a, |a| {
            if let Some(body) = touchfw::cpio_find(a, "gslX680new.ko") {
                assert!(body.len() <= a.len());
            }
        });
        let g = r.bin(&[&gz]);
        check("touchfw::gunzip", &g, |g| {
            if let Ok(out) = touchfw::gunzip(g, 1 << 16) {
                assert!(out.len() <= 1 << 16);
            }
        });
        let h = r.bin(&[&boot[..64]]);
        check("touchfw::ramdisk_span", &h, |h| {
            let _ = touchfw::ramdisk_span(h);
        });
    }
    for _ in 0..iters(1_000) {
        let disk = r.bin(&[&boot]);
        check("touchfw::extract", &disk, |disk| {
            let read = |o: u64, b: &mut [u8]| -> std::io::Result<()> {
                let s = usize::try_from(o)
                    .ok()
                    .and_then(|o| disk.get(o..o.checked_add(b.len())?))
                    .ok_or(std::io::ErrorKind::UnexpectedEof)?;
                b.copy_from_slice(s);
                Ok(())
            };
            let _ = touchfw::extract(&read, 0, touchfw::SHA256);
        });
    }
}
