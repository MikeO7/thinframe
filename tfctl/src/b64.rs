//! Standard base64 (RFC 4648, `=` padding) decoding, for the splash logo the server pushes in
//! its config (TASK 15). Whitespace is skipped; anything else outside the alphabet is an error.

fn val(c: u8) -> Option<u32> {
    Some(match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'+' => 62,
        b'/' => 63,
        _ => return None,
    } as u32)
}

pub fn decode(s: &str) -> Option<Vec<u8>> {
    let body: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !body.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(body.len() / 4 * 3);
    for (i, q) in body.chunks(4).enumerate() {
        let last = i == body.len() / 4 - 1;
        let pad = q.iter().rev().take_while(|&&b| b == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for &c in &q[..4 - pad] {
            n = n << 6 | val(c)?;
        }
        n <<= 6 * pad as u32;
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc4648_vectors() {
        for (enc, dec) in [
            ("", ""),
            ("Zg==", "f"),
            ("Zm8=", "fo"),
            ("Zm9v", "foo"),
            ("Zm9vYg==", "foob"),
            ("Zm9vYmE=", "fooba"),
            ("Zm9vYmFy", "foobar"),
        ] {
            assert_eq!(decode(enc).unwrap(), dec.as_bytes(), "{enc}");
        }
    }

    #[test]
    fn binary_whitespace_and_errors() {
        assert_eq!(decode("iVBO\nRw0K\r\nGgo=").unwrap(), b"\x89PNG\r\n\x1a\n");
        assert_eq!(decode("/+8=").unwrap(), [0xff, 0xef]);
        assert!(decode("Zm9").is_none());
        assert!(decode("Zm9!").is_none());
        assert!(decode("Zg==Zm9v").is_none());
        assert!(decode("Z===").is_none());
    }
}
