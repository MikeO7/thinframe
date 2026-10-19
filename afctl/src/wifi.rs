//! Write an iwd profile and reconnect.
//!
//! TODO(TASK 7):
//!  - write /var/lib/iwd/<SSID>.psk mode 0600:
//!        [Security]\nPassphrase=<psk>\n\n[Settings]\nAutoConnect=true\n
//!  - `rc-service iwd restart`, then `iwctl station wlan0 connect <SSID>`
//!  - print `iwctl station wlan0 show`
use std::io;

pub fn run(_ssid: &str, _psk: &str) -> io::Result<()> {
    Err(io::Error::other("wifi: not implemented (TASK 7)"))
}
