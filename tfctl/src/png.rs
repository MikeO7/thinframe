//! Minimal PNG decoder for the boot logo (TASK 15): non-interlaced, 8-bit grey / grey+alpha /
//! RGB / RGBA. Includes its own inflate (stored, fixed and dynamic Huffman). No crates.
use std::io::{Error, ErrorKind};

type R<T> = Result<T, Error>;

fn bad(msg: &str) -> Error {
    Error::new(ErrorKind::InvalidData, format!("png: {msg}"))
}

pub struct Image {
    pub w: usize,
    pub h: usize,
    /// Row-major 0xAARRGGBB.
    pub px: Vec<u32>,
}

// ---- inflate ----------------------------------------------------------------------------

struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u32,
    n: u32,
}

impl Bits<'_> {
    fn need(&mut self, k: u32) -> R<()> {
        while self.n < k {
            let b = *self
                .d
                .get(self.pos)
                .ok_or_else(|| bad("truncated deflate"))?;
            self.pos += 1;
            self.buf |= (b as u32) << self.n;
            self.n += 8;
        }
        Ok(())
    }
    fn get(&mut self, k: u32) -> R<u32> {
        if k == 0 {
            return Ok(0);
        }
        self.need(k)?;
        let v = self.buf & ((1u32 << k) - 1);
        self.buf >>= k;
        self.n -= k;
        Ok(v)
    }
}

/// Canonical Huffman table: counts per length + symbols sorted by code.
struct Huff {
    count: [u16; 16],
    sym: Vec<u16>,
}

impl Huff {
    fn new(lengths: &[u8]) -> Huff {
        let mut count = [0u16; 16];
        for &l in lengths {
            count[l as usize] += 1;
        }
        count[0] = 0;
        let mut offs = [0u16; 16];
        for i in 1..16 {
            offs[i] = offs[i - 1] + count[i - 1];
        }
        let mut sym = vec![0u16; lengths.len()];
        for (s, &l) in lengths.iter().enumerate() {
            if l != 0 {
                sym[offs[l as usize] as usize] = s as u16;
                offs[l as usize] += 1;
            }
        }
        Huff { count, sym }
    }
    fn decode(&self, b: &mut Bits) -> R<u16> {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= b.get(1)? as i32;
            let c = self.count[len] as i32;
            if code - c < first {
                return Ok(self.sym[(index + (code - first)) as usize]);
            }
            index += c;
            first = (first + c) << 1;
            code <<= 1;
        }
        Err(bad("bad huffman code"))
    }
}

const LBASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEXT: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DBASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DEXT: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn codes(b: &mut Bits, out: &mut Vec<u8>, lit: &Huff, dist: &Huff, limit: usize) -> R<()> {
    loop {
        let s = lit.decode(b)? as usize;
        match s {
            0..=255 => out.push(s as u8),
            256 => return Ok(()),
            257..=285 => {
                let i = s - 257;
                let len = LBASE[i] as usize + b.get(LEXT[i] as u32)? as usize;
                let d = dist.decode(b)? as usize;
                if d >= 30 {
                    return Err(bad("bad distance"));
                }
                let dist_v = DBASE[d] as usize + b.get(DEXT[d] as u32)? as usize;
                if dist_v > out.len() {
                    return Err(bad("distance too far"));
                }
                for _ in 0..len {
                    out.push(out[out.len() - dist_v]);
                }
            }
            _ => return Err(bad("bad symbol")),
        }
        if out.len() > limit {
            return Err(bad("output too large"));
        }
    }
}

/// Inflate a raw deflate stream. `limit` caps the output (decompression-bomb guard).
pub fn inflate(data: &[u8], limit: usize) -> R<Vec<u8>> {
    let mut b = Bits {
        d: data,
        pos: 0,
        buf: 0,
        n: 0,
    };
    let mut out = Vec::new();
    loop {
        let last = b.get(1)?;
        match b.get(2)? {
            0 => {
                b.buf = 0;
                b.n = 0;
                let p = b.pos;
                let hdr = data
                    .get(p..p + 4)
                    .ok_or_else(|| bad("truncated stored block"))?;
                let len = u16::from_le_bytes([hdr[0], hdr[1]]) as usize;
                if len != !u16::from_le_bytes([hdr[2], hdr[3]]) as usize & 0xffff {
                    return Err(bad("stored length check"));
                }
                let body = data
                    .get(p + 4..p + 4 + len)
                    .ok_or_else(|| bad("truncated stored"))?;
                out.extend_from_slice(body);
                b.pos = p + 4 + len;
            }
            1 => {
                let mut l = [8u8; 288];
                l[144..256].fill(9);
                l[256..280].fill(7);
                codes(
                    &mut b,
                    &mut out,
                    &Huff::new(&l),
                    &Huff::new(&[5u8; 30]),
                    limit,
                )?;
            }
            2 => {
                let hlit = b.get(5)? as usize + 257;
                let hdist = b.get(5)? as usize + 1;
                let hclen = b.get(4)? as usize + 4;
                const ORDER: [usize; 19] = [
                    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
                ];
                let mut cl = [0u8; 19];
                for &o in ORDER.iter().take(hclen) {
                    cl[o] = b.get(3)? as u8;
                }
                let clh = Huff::new(&cl);
                let mut lens = Vec::with_capacity(hlit + hdist);
                while lens.len() < hlit + hdist {
                    let s = clh.decode(&mut b)?;
                    match s {
                        0..=15 => lens.push(s as u8),
                        16 => {
                            let prev = *lens.last().ok_or_else(|| bad("repeat with no prior"))?;
                            for _ in 0..3 + b.get(2)? {
                                lens.push(prev);
                            }
                        }
                        17 => lens.resize(lens.len() + 3 + b.get(3)? as usize, 0),
                        _ => lens.resize(lens.len() + 11 + b.get(7)? as usize, 0),
                    }
                }
                if lens.len() != hlit + hdist {
                    return Err(bad("code lengths overrun"));
                }
                let (ll, dl) = lens.split_at(hlit);
                codes(&mut b, &mut out, &Huff::new(ll), &Huff::new(dl), limit)?;
            }
            _ => return Err(bad("bad block type")),
        }
        if out.len() > limit {
            return Err(bad("output too large"));
        }
        if last == 1 {
            return Ok(out);
        }
    }
}

// ---- PNG --------------------------------------------------------------------------------

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let (ia, ib, ic) = (a as i32, b as i32, c as i32);
    let p = ia + ib - ic;
    let (pa, pb, pc) = ((p - ia).abs(), (p - ib).abs(), (p - ic).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

pub fn decode(data: &[u8]) -> R<Image> {
    if data.len() < 8 || data[..8] != [0x89, b'P', b'N', b'G', 13, 10, 26, 10] {
        return Err(bad("not a png"));
    }
    let (mut pos, mut hdr, mut idat) = (8usize, None, Vec::new());
    while pos + 8 <= data.len() {
        let len =
            u32::from_be_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]) as usize;
        let kind = &data[pos + 4..pos + 8];
        let body = data
            .get(pos + 8..pos + 8 + len)
            .ok_or_else(|| bad("truncated chunk"))?;
        match kind {
            b"IHDR" if len == 13 => hdr = Some(body.to_vec()),
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        pos += 12 + len;
    }
    let h = hdr.ok_or_else(|| bad("no IHDR"))?;
    let w = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as usize;
    let ht = u32::from_be_bytes([h[4], h[5], h[6], h[7]]) as usize;
    let (depth, ctype, interlace) = (h[8], h[9], h[12]);
    if depth != 8 || interlace != 0 || w == 0 || ht == 0 || w > 4096 || ht > 4096 {
        return Err(bad("unsupported: need 8-bit, non-interlaced, <=4096 px"));
    }
    let bpp = match ctype {
        0 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        _ => return Err(bad("unsupported colour type")),
    };
    if idat.len() < 6 {
        return Err(bad("no image data"));
    }
    let stride = w * bpp;
    let raw = inflate(&idat[2..], (stride + 1) * ht)?;
    if raw.len() != (stride + 1) * ht {
        return Err(bad("wrong decoded size"));
    }
    let mut cur = vec![0u8; stride * ht];
    for y in 0..ht {
        let f = raw[y * (stride + 1)];
        let line = &raw[y * (stride + 1) + 1..(y + 1) * (stride + 1)];
        for x in 0..stride {
            let a = if x >= bpp {
                cur[y * stride + x - bpp]
            } else {
                0
            };
            let b = if y > 0 { cur[(y - 1) * stride + x] } else { 0 };
            let c = if x >= bpp && y > 0 {
                cur[(y - 1) * stride + x - bpp]
            } else {
                0
            };
            cur[y * stride + x] = line[x].wrapping_add(match f {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((a as u16 + b as u16) / 2) as u8,
                4 => paeth(a, b, c),
                _ => return Err(bad("bad filter")),
            });
        }
    }
    let px = cur
        .chunks(bpp)
        .map(|p| {
            let (r, g, b, a) = match bpp {
                1 => (p[0], p[0], p[0], 255),
                2 => (p[0], p[0], p[0], p[1]),
                3 => (p[0], p[1], p[2], 255),
                _ => (p[0], p[1], p[2], p[3]),
            };
            (a as u32) << 24 | (r as u32) << 16 | (g as u32) << 8 | b as u32
        })
        .collect();
    Ok(Image { w, h: ht, px })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text() -> Vec<u8> {
        let mut t = "thinframe boots fast; thinframe renders html; "
            .repeat(6)
            .into_bytes();
        for _ in 0..3 {
            t.extend(40u8..90);
        }
        t
    }

    const DYNAMIC: &[u8] = &[
        0x2b, 0xc9, 0xc8, 0xcc, 0x4b, 0x2b, 0x4a, 0xcc, 0x4d, 0x55, 0x48, 0xca, 0xcf, 0x2f, 0x29,
        0x56, 0x48, 0x4b, 0x2c, 0x2e, 0xb1, 0x56, 0x28, 0x81, 0x8b, 0x16, 0xa5, 0xe6, 0xa5, 0xa4,
        0x16, 0x15, 0x2b, 0x64, 0x94, 0xe4, 0xe6, 0x20, 0x8b, 0x0f, 0x67, 0xd5, 0x1a, 0x9a, 0x5a,
        0xda, 0x3a, 0xba, 0x7a, 0xfa, 0x06, 0x86, 0x46, 0xc6, 0x26, 0xa6, 0x66, 0xe6, 0x16, 0x96,
        0x56, 0xd6, 0x36, 0xb6, 0x76, 0xf6, 0x0e, 0x8e, 0x4e, 0xce, 0x2e, 0xae, 0x6e, 0xee, 0x1e,
        0x9e, 0x5e, 0xde, 0x3e, 0xbe, 0x7e, 0xfe, 0x01, 0x81, 0x41, 0xc1, 0x21, 0xa1, 0x61, 0xe1,
        0x11, 0x91, 0xf4, 0xd0, 0x01, 0x00,
    ];
    const FIXED: &[u8] = &[
        0x2b, 0xc9, 0xc8, 0xcc, 0x4b, 0x2b, 0x4a, 0xcc, 0x4d, 0x55, 0x48, 0xca, 0xcf, 0x2f, 0x29,
        0x56, 0x48, 0x4b, 0x2c, 0x2e, 0xb1, 0x56, 0x28, 0x81, 0x8b, 0x16, 0xa5, 0xe6, 0xa5, 0xa4,
        0x16, 0x15, 0x2b, 0x64, 0x94, 0xe4, 0xe6, 0x20, 0x8b, 0x0f, 0x67, 0xd5, 0x1a, 0x9a, 0x5a,
        0xda, 0x3a, 0xba, 0x7a, 0xfa, 0x06, 0x86, 0x46, 0xc6, 0x26, 0xa6, 0x66, 0xe6, 0x16, 0x96,
        0x56, 0xd6, 0x36, 0xb6, 0x76, 0xf6, 0x0e, 0x8e, 0x4e, 0xce, 0x2e, 0xae, 0x6e, 0xee, 0x1e,
        0x9e, 0x5e, 0xde, 0x3e, 0xbe, 0x7e, 0xfe, 0x01, 0x81, 0x41, 0xc1, 0x21, 0xa1, 0x61, 0xe1,
        0x11, 0x91, 0xf4, 0xd0, 0x01, 0x00,
    ];
    const STORED: &[u8] = &[
        0x01, 0xaa, 0x01, 0x55, 0xfe, 0x74, 0x68, 0x69, 0x6e, 0x66, 0x72, 0x61,
    ]; // header only, body appended below

    #[test]
    fn inflate_dynamic_fixed_stored() {
        assert_eq!(inflate(DYNAMIC, 1 << 20).unwrap(), text());
        assert_eq!(inflate(FIXED, 1 << 20).unwrap(), text());
        let mut s = STORED.to_vec();
        s.clear();
        s.extend_from_slice(&[1, 5, 0, 0xfa, 0xff]);
        s.extend_from_slice(b"hello");
        assert_eq!(inflate(&s, 100).unwrap(), b"hello");
    }

    const DYN_BLOCK: &[u8] = &[
        0x4d, 0x54, 0x87, 0x81, 0xc3, 0x30, 0x08, 0x5c, 0x85, 0xd5, 0x68, 0xfb, 0xaf, 0xf0, 0x57,
        0x90, 0x3f, 0xa9, 0xb6, 0x8c, 0x80, 0x2b, 0x28, 0xb3, 0x6b, 0x32, 0x3b, 0x3b, 0x72, 0xab,
        0x07, 0xd7, 0x85, 0xa5, 0xee, 0xcc, 0xe8, 0x5c, 0xde, 0xf1, 0x61, 0xe1, 0x8a, 0x91, 0x9d,
        0x0c, 0xc6, 0x17, 0xaf, 0xc1, 0xbb, 0x8a, 0x11, 0xd3, 0xab, 0x50, 0xbc, 0x42, 0x3b, 0x94,
        0xb1, 0xf4, 0x46, 0x9e, 0x9a, 0xc2, 0xce, 0xd2, 0x26, 0x26, 0xde, 0xf4, 0xa5, 0x3e, 0x91,
        0x53, 0x4c, 0x33, 0xa3, 0x68, 0x5c, 0x45, 0x45, 0x4c, 0x68, 0x6b, 0x4f, 0x74, 0xb7, 0x96,
        0xaf, 0x4d, 0x34, 0x17, 0x8d, 0xd0, 0xee, 0x74, 0x42, 0x95, 0x65, 0x6f, 0xfe, 0x75, 0x5f,
        0x88, 0xe5, 0xe7, 0x01, 0xe0, 0xfd, 0x30, 0x03, 0x82, 0xf0, 0x6d, 0xc7, 0x11, 0x26, 0x9f,
        0x13, 0x6d, 0x01, 0x3e, 0xbb, 0x11, 0x1c, 0x43, 0x76, 0xf2, 0x28, 0xe4, 0x20, 0xe0, 0x58,
        0x81, 0x1b, 0x51, 0x14, 0xe5, 0x00, 0x3c, 0x24, 0x17, 0x48, 0x43, 0xfa, 0x58, 0x5a, 0xdb,
        0x9a, 0x84, 0xbc, 0x74, 0x2d, 0xfe, 0x46, 0x59, 0xd5, 0x65, 0x8b, 0xde, 0x10, 0x7c, 0xa4,
        0x7b, 0xa8, 0x47, 0xc1, 0x1d, 0x51, 0x6a, 0x0f, 0x24, 0xa0, 0x7d, 0x36, 0x63, 0xba, 0xbb,
        0x8c, 0x37, 0x94, 0xe3, 0xfa, 0x27, 0xab, 0xc5, 0x0e, 0x05, 0x1e, 0x45, 0xa3, 0x0d, 0x21,
        0x0f, 0x37, 0xf5, 0x22, 0x55, 0xad, 0x1a, 0x5e, 0xe6, 0x2e, 0x00, 0x60, 0xbd, 0xa4, 0x30,
        0x28, 0x02, 0x15, 0x4e, 0x31, 0x33, 0x1b, 0xea, 0x2a, 0x6b, 0x2d, 0xef, 0x92, 0x70, 0xc2,
        0x93, 0x46, 0xe5, 0xa2, 0x56, 0x50, 0x3d, 0x4f, 0xf6, 0x57, 0x53, 0xb2, 0x89, 0x88, 0xea,
        0xa7, 0xf2, 0xd2, 0x10, 0xe4, 0xef, 0x19, 0x04, 0xa2, 0x07, 0xfb, 0xb5, 0x6a, 0x63, 0xc0,
        0xf6, 0x85, 0x32, 0x53, 0x30, 0xb9, 0xcd, 0xbc, 0xb1, 0xde, 0xd0, 0x65, 0xa3, 0x3a, 0x70,
        0x85, 0x5c, 0x73, 0x2c, 0x4c, 0xce, 0x11, 0xc2, 0x7f, 0x1a, 0x67, 0xa5, 0xa0, 0x4c, 0xac,
        0xf8, 0xb5, 0x4b, 0x44, 0xf6, 0x9c, 0x5b, 0x2d, 0x8d, 0xea, 0x49, 0x4b, 0xd9, 0xab, 0xe4,
        0xdc, 0xdd, 0x13, 0xbf, 0xec, 0x9f, 0xc3, 0xca, 0x8c, 0x24, 0xa1, 0x96, 0xd5, 0x35, 0x11,
        0xb0, 0x4d, 0xb1, 0xfe, 0xc7, 0x7a, 0x98, 0x20, 0x12, 0xaf, 0x1e, 0x72, 0xdb, 0x84, 0xd4,
        0x7c, 0x86, 0xfd, 0x51, 0x07, 0x42, 0x78, 0x56, 0x48, 0x4f, 0x94, 0x66, 0xc8, 0x73, 0xc3,
        0x1d, 0xe3, 0x3d, 0xbb, 0x02, 0x2b, 0x40, 0x74, 0x99, 0x28, 0xa1, 0x33, 0x84, 0x0f, 0x77,
        0xfb, 0xda, 0xa3, 0x8b, 0xe8, 0x03, 0x4d, 0xa9, 0x15, 0x8c, 0xbe, 0x0b, 0x42, 0x86, 0x2f,
        0x89, 0x99, 0xee, 0x33, 0x7c, 0x74, 0x67, 0xc7, 0x50, 0x08, 0xd4, 0x69, 0xa9, 0x3f, 0x2b,
        0x2a, 0xda, 0x4d, 0x4a, 0xb3, 0xb6, 0xcd, 0x88, 0x54, 0xc3, 0x2a, 0xcd, 0x34, 0xd7, 0xd4,
        0xb5, 0x34, 0x53, 0x6b, 0x17, 0xb5, 0x45, 0x46, 0x8b, 0x23, 0x95, 0xdc, 0x19, 0x69, 0xda,
        0xa3, 0x62, 0x6d, 0x2a, 0x54, 0x6f, 0xd9, 0x3f, 0x34, 0x39, 0x06, 0x7d, 0x1c, 0x32, 0x01,
        0xd3, 0xdb, 0x5f, 0xb6, 0x37, 0x47, 0xe1, 0x93, 0xbd, 0x5a, 0x27, 0xc5, 0x3b, 0x70, 0x74,
        0xf6, 0x58, 0x4b, 0x74, 0x11, 0x3e, 0x09, 0x94, 0xdb, 0xc1, 0x19, 0x91, 0x77, 0x68, 0xe9,
        0xa8, 0xcb, 0xcf, 0x72, 0xf3, 0x29, 0x90, 0x1b, 0x37, 0x09, 0x57, 0xe1, 0x56, 0xd3, 0xc7,
        0x9d, 0xe2, 0xac, 0xcd, 0xfe, 0x1f, 0x3d, 0x4a, 0x66, 0x1b, 0xd7, 0x73, 0x5e, 0xbd, 0x42,
        0x37, 0x0d, 0x23, 0x43, 0x79, 0x10, 0xd0, 0xf0, 0xf8, 0x44, 0x43, 0xc0, 0x92, 0xbe, 0xa0,
        0x98, 0x7d, 0xdd, 0xfa, 0xef, 0x06, 0x7e, 0xa4, 0xe2, 0xdc, 0x89, 0xe2, 0x64, 0xa3, 0xfb,
        0xe9, 0x27, 0xf0, 0xb8, 0xd3, 0x9b, 0x23, 0x2e, 0xb8, 0x5b, 0x92, 0xfb, 0x07,
    ];

    #[test]
    fn inflate_real_dynamic_block() {
        let mut x = 1u32;
        let alpha = b"aaaaaaaaaabbbbbcccdde ";
        let want: Vec<u8> = (0..1500)
            .map(|_| {
                x = x.wrapping_mul(1103515245).wrapping_add(12345);
                alpha[((x >> 16) as usize) % alpha.len()]
            })
            .collect();
        assert_eq!(
            (DYN_BLOCK[0] >> 1) & 3,
            2,
            "fixture must be a dynamic block"
        );
        assert_eq!(inflate(DYN_BLOCK, 1 << 20).unwrap(), want);
    }

    #[test]
    fn inflate_rejects_bombs_and_garbage() {
        assert!(inflate(DYNAMIC, 10).is_err());
        assert!(inflate(&DYNAMIC[..DYNAMIC.len() / 2], 1 << 20).is_err());
        assert!(inflate(&[0xff, 0xff, 0xff], 100).is_err());
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn decodes_rgba_with_sub_up_paeth_filters() {
        let img = decode(&hex("89504e470d0a1a0a0000000d4948445200000004000000030806000000b4f4aec60000003c4944415478da63745168b2d9792cbf2dcf8bc327662dc7352697d04d6942aa324e1176f7195631ff99cca2c168e3d9cab0507f8e666a796563dc7300cef0126b1de370680000000049454e44ae426082")).unwrap();
        assert_eq!((img.w, img.h), (4, 3));
        assert_eq!(
            img.px,
            [
                0x3c442082, 0xc2fde6f1, 0x0e6b30f9, 0xe4c7dd01, 0xa2887534, 0x040f0b0d, 0x0ec36ed8,
                0x7771e0fd, 0xebb07670, 0x33940bd5, 0xaa5f973d, 0x91d8619b
            ]
        );
    }

    #[test]
    fn rejects_non_png_and_truncated() {
        assert!(decode(b"GIF89a....").is_err());
        let p = hex("89504e470d0a1a0a0000000d4948445200000004000000030806000000b4f4aec60000003c4944415478da63745168b2d9792cbf2dcf8bc327662dc7352697d04d6942aa324e1176f7195631ff99cca2c168e3d9cab0507f8e666a796563dc7300cef0126b1de370680000000049454e44ae426082");
        assert!(decode(&p[..40]).is_err());
    }
}
