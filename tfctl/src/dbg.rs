//! Debug mode: `DEBUG="1"` in thinframe.conf, or `debug: true` in the server's config for the
//! frame (persisted as DEBUG). Everything that moves is logged with a `tfctl: debug:` prefix:
//! socket traffic both ways, config keys written, cog's command line; cog itself runs with
//! `G_MESSAGES_DEBUG=all`, and a `report` goes up on the first server contact of each process.
//! Off, tfctl logs only what changed or failed, as before.
use std::sync::atomic::{AtomicBool, Ordering};

static ON: AtomicBool = AtomicBool::new(false);

pub fn set(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// `tfctl: debug: <what>: <text>`, text cut at 400 bytes (a config push carries a 48 KiB logo).
pub fn log(what: &str, text: &str) {
    if on() {
        eprintln!("tfctl: debug: {what}: {}", clip(text, 400));
    }
}

pub fn clip(s: &str, n: usize) -> &str {
    if s.len() <= n {
        return s;
    }
    let mut i = n;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_respects_char_boundaries() {
        assert_eq!(clip("abc", 5), "abc");
        assert_eq!(clip("abcdef", 3), "abc");
        assert_eq!(clip("aé", 2), "a");
    }
}
