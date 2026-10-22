//! Agent, second half: applying a server config object and running one-shot actions.
//! Pure functions of (agent state, JSON) plus KioskCmd sends, so they are tested here with a
//! channel and a temp conf file. The socket loop lives in agent.rs.
use crate::agent::Agent;
use crate::b64;
use crate::bench;
use crate::config;
use crate::json::Value;
use crate::kiosk::KioskCmd;
use crate::png;
use crate::sysupdate;
use crate::update::{self, Artifacts};
use std::io;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

/// 48 KiB of PNG in base64 (4/3 ratio, padding and a trailing newline allowed).
const MAX_LOGO_B64: usize = 48 * 1024 * 4 / 3 + 8;

impl Agent {
    pub(crate) fn persist(&self, kv: &[(&str, &str)]) {
        if let Err(e) = config::save_overrides(&self.conf_path, kv) {
            eprintln!("tfctl: agent: cannot write {}: {e}", self.conf_path);
        }
    }

    /// Apply a server config object: persist changed keys, signal the kiosk for the ones that
    /// affect cog. Idempotent: an unchanged object does nothing.
    pub fn apply_config(&mut self, v: &Value) {
        let mut over: Vec<(&str, String)> = Vec::new();
        // The server may send a path ("/frame/kitchen"); resolve it against SERVER_URL.
        let abs = |u: String| {
            if u.starts_with('/') {
                format!("{}{}", self.cfg.server_url.trim_end_matches('/'), u)
            } else {
                u
            }
        };
        if let Some(u) = v
            .field_string("url")
            .map(abs)
            .filter(|u| *u != self.cfg.url)
        {
            over.push(("FRAME_URL", u.clone()));
            self.cfg.url = u.clone();
            if !self.on_fallback {
                let _ = self.send(KioskCmd::SetUrl(u));
            }
        }
        let num = |k: &str| v.field_string(k).and_then(|s| s.parse::<u8>().ok());
        if let Some(r) = num("rotation").filter(|r| *r < 4 && *r != self.cfg.rotation) {
            over.push(("COG_ROTATION", r.to_string()));
            self.cfg.rotation = r;
            let _ = self.send(KioskCmd::SetRotation(r));
        }
        if let Some(b) = num("brightness").filter(|b| *b <= 100 && *b != self.cfg.brightness) {
            over.push(("BRIGHTNESS", b.to_string()));
            self.cfg.brightness = b;
            let _ = self.send(KioskCmd::Brightness(b));
        }
        let sched = v.get("schedule");
        let off = v
            .field_string("screen_off")
            .or_else(|| sched?.field_string("off"));
        let on = v
            .field_string("screen_on")
            .or_else(|| sched?.field_string("on"));
        if off.is_some() || on.is_some() {
            let off = off.unwrap_or_default();
            let on = on.unwrap_or_default();
            if off != self.cfg.screen_off || on != self.cfg.screen_on {
                over.push(("SCREEN_OFF", off.clone()));
                over.push(("SCREEN_ON", on.clone()));
                self.cfg.screen_off = off.clone();
                self.cfg.screen_on = on.clone();
                let _ = self.send(KioskCmd::Schedule(off, on));
            }
        }
        if let Some(p) = v
            .field_string("page_version")
            .filter(|p| *p != self.cfg.page_version)
        {
            over.push(("PAGE_VERSION", p.clone()));
            if !self.cfg.page_version.is_empty() {
                let _ = self.send(KioskCmd::Reload);
            }
            self.cfg.page_version = p;
        }
        // Splash (TASK 15): stored now, drawn from the next start of the kiosk. Empty = keep.
        if let Some(t) = v
            .field_string("splash_text")
            .filter(|t| !t.is_empty() && *t != self.cfg.splash_text)
        {
            over.push(("SPLASH_TEXT", t.clone()));
            self.cfg.splash_text = t;
        }
        if let Some(bg) = v
            .field_string("splash_bg")
            .and_then(|s| config::parse_rgb(&s))
            .filter(|bg| *bg != self.cfg.splash_bg)
        {
            over.push(("SPLASH_BG", format!("#{bg:06x}")));
            self.cfg.splash_bg = bg;
        }
        if let Some(b) = v.field_string("splash_png").filter(|b| !b.is_empty()) {
            if let Err(e) = self.store_logo(&b) {
                eprintln!("tfctl: agent: splash_png: {e}");
            }
        }
        if let Some(ver) = v.field_string("software_version") {
            self.software_version = ver;
        }
        if let Some(ver) = v.field_string("system_version") {
            self.system_version = ver;
        }
        if v.get("artifacts").is_some() {
            self.artifacts = Artifacts::parse(v);
            self.tried.clear();
        }
        if !over.is_empty() {
            let refs: Vec<(&str, &str)> = over.iter().map(|(k, v)| (*k, v.as_str())).collect();
            self.persist(&refs);
        }
    }

    /// Decode, check it is a PNG we can draw, and write `<data>/splash.png` if it changed.
    /// The server refuses a `splash_png` over 48 KiB; a frame refuses the same before decoding.
    fn store_logo(&self, b64: &str) -> io::Result<()> {
        // The server caps splash_png at 48 KiB; refuse anything past that before decoding.
        if b64.len() > MAX_LOGO_B64 {
            return Err(io::Error::other("splash_png larger than 48 KiB"));
        }
        let bytes = b64::decode(b64).ok_or_else(|| io::Error::other("not base64"))?;
        png::decode(&bytes).map_err(|e| io::Error::other(format!("not a usable PNG: {e}")))?;
        let path = self.paths.data.join("splash.png");
        if std::fs::read(&path).ok().as_deref() == Some(&bytes[..]) {
            return Ok(());
        }
        std::fs::create_dir_all(&self.paths.data)?;
        let tmp = path.with_extension("png.tmp");
        std::fs::write(&tmp, &bytes)?;
        std::fs::rename(&tmp, &path)?;
        self.event("splash", &format!("logo stored, {} bytes", bytes.len()));
        Ok(())
    }

    pub fn action(&mut self, name: &str, v: &Value) -> io::Result<()> {
        match name {
            "restart-kiosk" => self.send(KioskCmd::Restart),
            "reload-page" => self.send(KioskCmd::Reload),
            "reboot" => {
                // Let the ack leave first.
                thread::spawn(|| {
                    thread::sleep(Duration::from_secs(1));
                    let _ = Command::new("reboot").spawn();
                });
                Ok(())
            }
            "screen" => {
                let state = v.field_string("value").or_else(|| v.field_string("state"));
                match state.as_deref() {
                    Some("on") => self.send(KioskCmd::Screen(true)),
                    Some("off") => self.send(KioskCmd::Screen(false)),
                    _ => Err(io::Error::other("screen: value must be on|off")),
                }
            }
            "update-software" => {
                // The respawn that follows would orphan the system update mid-dd.
                if self.busy.load(Ordering::SeqCst) {
                    return Err(io::Error::other("a system update is already running"));
                }
                let ver = self.want(v, &self.software_version)?;
                update::software(&self.cfg, &self.artifacts, &ver, &self.paths)?;
                self.exit = true;
                Ok(())
            }
            "update-system" => {
                let ver = self.want(v, &self.system_version)?;
                self.artifacts.get("system")?;
                self.start_system_update(ver)
            }
            "bench" => self.start_bench(v),
            "rollback" => {
                let slot = sysupdate::rollback(&self.paths)?;
                self.event("rollback", &format!("booting slot {} next", slot.name()));
                sysupdate::reboot_soon();
                Ok(())
            }
            other => Err(io::Error::other(format!("unknown action '{other}'"))),
        }
    }

    /// TASK 13: `tfctl bench` from the server (it needs root, which the agent has). Taps take
    /// seconds, so it runs in its own thread; the result is a `bench` event.
    fn start_bench(&mut self, v: &Value) -> io::Result<()> {
        let taps = v
            .field_string("taps")
            .and_then(|t| t.parse().ok())
            .unwrap_or(bench::TAPS)
            .clamp(1, 50);
        let (cfg, events) = (self.cfg.clone(), self.events.clone());
        thread::Builder::new().name("bench".into()).spawn(move || {
            let (kind, text) =
                match bench::measure(&cfg, taps, &mut |l| eprintln!("tfctl: bench: {l}")) {
                    Ok(s) => ("bench", s),
                    Err(e) => ("bench-failed", e.to_string()),
                };
            eprintln!("tfctl: agent: event {kind}: {text}");
            if let Ok(mut q) = events.lock() {
                q.push((kind.into(), text));
            }
        })?;
        Ok(())
    }

    /// Version from the action's `version` field, else the last config's.
    fn want(&self, v: &Value, fallback: &str) -> io::Result<String> {
        v.field_string("version")
            .filter(|s| !s.is_empty())
            .or_else(|| (!fallback.is_empty()).then(|| fallback.to_string()))
            .ok_or_else(|| io::Error::other("no version known; wait for a config"))
    }

    /// The system update downloads ~150 MB and writes a partition: minutes, so it runs in its
    /// own thread and the action is acked as accepted. The result arrives as an `event`; on
    /// success the frame reboots into the new slot.
    fn start_system_update(&mut self, ver: String) -> io::Result<()> {
        if self.busy.swap(true, Ordering::SeqCst) {
            return Err(io::Error::other("a system update is already running"));
        }
        let (cfg, arts, paths) = (self.cfg.clone(), self.artifacts.clone(), self.paths.clone());
        let (events, busy) = (self.events.clone(), self.busy.clone());
        thread::Builder::new()
            .name("sysupdate".into())
            .spawn(move || {
                let res = sysupdate::system(&cfg, &arts, &ver, &paths);
                if res.is_ok() {
                    if let Err(e) = sysupdate::mark_attempt(&paths, &ver) {
                        eprintln!("tfctl: agent: {e}");
                    }
                }
                let (kind, text) = match &res {
                    Ok(slot) => (
                        "system-update",
                        format!("{ver} written to slot {}, rebooting", slot.name()),
                    ),
                    Err(e) => ("system-update-failed", format!("{ver}: {e}")),
                };
                eprintln!("tfctl: agent: event {kind}: {text}");
                if let Ok(mut q) = events.lock() {
                    q.push((kind.into(), text));
                }
                busy.store(false, Ordering::SeqCst);
                if res.is_ok() {
                    sysupdate::reboot_soon();
                }
            })?;
        Ok(())
    }

    /// TASK 11/12: a config whose versions differ from what this root carries starts the
    /// update by itself. Only on a frame that has the version files; a dev box never updates.
    pub(crate) fn maybe_auto_update(&mut self) {
        let want = self.software_version.clone();
        if let Some(have) = update::installed_version(&self.paths) {
            // Rolled back after crashing at start (update.rs): installing it again by itself
            // would crash and roll back forever. The `update-software` action still can.
            let failed = update::failed_version(&self.paths).as_deref() == Some(&want);
            if failed && have != want && !self.tried.contains(&want) {
                self.tried.push(want.clone());
                self.event(
                    "software-update-skipped",
                    &format!("{want} crashed at start and was rolled back; not retrying"),
                );
            }
            if !want.is_empty()
                && have != want
                && !self.tried.contains(&want)
                && !self.busy.load(Ordering::SeqCst)
            {
                self.tried.push(want.clone());
                eprintln!("tfctl: agent: software {have} -> {want}");
                match update::software(&self.cfg, &self.artifacts, &want, &self.paths) {
                    Ok(()) => self.exit = true,
                    Err(e) => self.event("software-update-failed", &format!("{want}: {e}")),
                }
            }
        }
        if self.exit {
            // The process restarts in a moment; a system update started now would be
            // orphaned mid-dd and started again by the new binary.
            return;
        }
        let want = self.system_version.clone();
        if let Some(have) = sysupdate::system_version(&self.paths) {
            // Written before and not what runs now: it failed to boot (U-Boot fell back) or was
            // rolled back on purpose. Either way, flashing it again by itself would undo that.
            let failed_before =
                have != want && sysupdate::attempted(&self.paths).as_deref() == Some(&want);
            if failed_before && !self.tried.contains(&want) {
                self.tried.push(want.clone());
                self.event(
                    "system-update-skipped",
                    &format!("{want} was installed before and is not running (failed to boot or rolled back); not retrying"),
                );
            }
            if !want.is_empty()
                && have != want
                && !failed_before
                && !self.tried.contains(&want)
                && self.artifacts.get("system").is_ok()
                && !self.busy.load(Ordering::SeqCst)
            {
                self.tried.push(want.clone());
                eprintln!("tfctl: agent: system {have} -> {want}");
                if let Err(e) = self.start_system_update(want) {
                    eprintln!("tfctl: agent: {e}");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::json;
    use std::sync::mpsc;

    fn agent(tag: &str) -> (Agent, mpsc::Receiver<KioskCmd>, String) {
        let (tx, rx) = mpsc::channel();
        let mut cfg = config::parse("SERVER_URL=http://nas:8080\nFRAME_ID=k\nPAGE_VERSION=1\n");
        cfg.fallback_url = "file:///fb.html".into();
        let path =
            std::env::temp_dir().join(format!("tfctl-agent-{}-{tag}.conf", std::process::id()));
        let p = path.to_str().unwrap().to_string();
        (Agent::new(&cfg, tx, &p).unwrap(), rx, p)
    }

    #[test]
    fn config_persists_and_signals_only_changes() {
        let (mut a, rx, p) = agent("config");
        let v = json::parse(
            r#"{"url":"http://nas:8080/frame/k","rotation":"1","brightness":40,
                "schedule":{"off":"23:00","on":"06:30"},"page_version":"2"}"#,
        )
        .unwrap();
        a.apply_config(&v);
        let got: Vec<_> = rx.try_iter().collect();
        assert_eq!(
            got,
            vec![
                KioskCmd::SetRotation(1),
                KioskCmd::Brightness(40),
                KioskCmd::Schedule("23:00".into(), "06:30".into()),
                KioskCmd::Reload,
            ]
        );
        let saved = config::load(&p).unwrap();
        assert_eq!((saved.rotation, saved.brightness), (1, 40));
        assert_eq!(
            (saved.screen_off.as_str(), saved.page_version.as_str()),
            ("23:00", "2")
        );
        a.apply_config(&v);
        assert!(rx.try_iter().next().is_none());
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn splash_fields_persist_and_logo_is_stored_once() {
        let (mut a, rx, p) = agent("splash");
        let base = std::env::temp_dir().join(format!("tfctl-splash-{}", std::process::id()));
        a.paths = crate::update::Paths::under(&base);
        let png = "iVBORw0KGgoAAAANSUhEUgAAAAQAAAADCAYAAAC09K7GAAAAPElEQVR42mN0UWiy2Xksvy3Pi8MnZi3HNSaX0E1pQqoyThF29xlWMf+ZzKLBaOPZyrBQf45manllY9xzAM7wEmsd43BoAAAAAElFTkSuQmCC";
        let cfg =
            format!(r##"{{"splash_text":"Kitchen","splash_bg":"#203040","splash_png":"{png}"}}"##);
        a.apply_config(&json::parse(&cfg).unwrap());
        let c = config::load(&p).unwrap();
        assert_eq!((c.splash_text.as_str(), c.splash_bg), ("Kitchen", 0x203040));
        let logo = std::fs::read(base.join("data/splash.png")).unwrap();
        assert_eq!(&logo[1..4], b"PNG");
        assert_eq!(a.events.lock().unwrap().len(), 1, "one splash event");
        a.apply_config(&json::parse(&cfg).unwrap());
        assert_eq!(
            a.events.lock().unwrap().len(),
            1,
            "unchanged logo not rewritten"
        );
        a.apply_config(&json::parse(r#"{"splash_png":"bm90IGEgcG5n","splash_text":""}"#).unwrap());
        assert_eq!(std::fs::read(base.join("data/splash.png")).unwrap(), logo);
        assert_eq!(config::load(&p).unwrap().splash_text, "Kitchen");
        assert!(
            rx.try_iter().next().is_none(),
            "splash changes wait for the next start"
        );
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn boot_opens_on_status_page_until_the_server_answers() {
        let (mut a, rx, _) = agent("boot");
        a.start_on_status_page();
        assert_eq!(a.page_url(), "file:///fb.html");
        a.server_failed();
        assert!(rx.try_iter().next().is_none(), "stays on the status page");
        a.server_ok();
        assert_eq!(
            rx.try_recv().unwrap(),
            KioskCmd::SetUrl("http://nas:8080/frame/k".into())
        );
        let _ = std::fs::remove_file(&a.conf_path);
    }

    #[test]
    fn fallback_after_three_failures_and_back() {
        let (mut a, rx, _) = agent("fallback");
        a.server_failed();
        a.server_failed();
        assert!(rx.try_iter().next().is_none());
        a.server_failed();
        assert_eq!(
            rx.try_recv().unwrap(),
            KioskCmd::SetUrl("file:///fb.html".into())
        );
        assert_eq!(a.page_url(), "file:///fb.html");
        a.apply_config(&json::parse(r#"{"url":"http://nas:8080/frame/other"}"#).unwrap());
        assert!(
            rx.try_iter().next().is_none(),
            "url change deferred while on fallback"
        );
        a.server_ok();
        assert_eq!(
            rx.try_recv().unwrap(),
            KioskCmd::SetUrl("http://nas:8080/frame/other".into())
        );
        let _ = std::fs::remove_file(&a.conf_path);
    }

    #[test]
    fn relative_url_resolves_against_server() {
        let (mut a, rx, _) = agent("relurl");
        a.apply_config(&json::parse(r#"{"url":"/frame/rel"}"#).unwrap());
        let want = format!("{}/frame/rel", a.cfg.server_url.trim_end_matches('/'));
        assert_eq!(a.cfg.url, want);
        assert_eq!(rx.try_recv().unwrap(), KioskCmd::SetUrl(want));
        let _ = std::fs::remove_file(&a.conf_path);
    }

    #[test]
    fn actions_map_to_kiosk_commands() {
        let (mut a, rx, _) = agent("actions");
        let screen = json::parse(r#"{"value":"off"}"#).unwrap();
        assert!(a.action("screen", &screen).is_ok());
        assert!(a.action("restart-kiosk", &Value::obj()).is_ok());
        assert!(a.action("update-system", &Value::obj()).is_err());
        assert!(a.action("update-software", &Value::obj()).is_err());
        assert!(a.action("bogus", &Value::obj()).is_err());
        let got: Vec<_> = rx.try_iter().collect();
        assert_eq!(got, vec![KioskCmd::Screen(false), KioskCmd::Restart]);
        let h = a.hello().to_string();
        assert!(h.starts_with(r#"{"type":"hello","frame_id":"k","#));
        assert!(h.contains(r#""page_version":"1""#));
    }

    #[test]
    fn config_carries_versions_and_artifacts() {
        let (mut a, _rx, _) = agent("versions");
        let sum = "c".repeat(64);
        let v = json::parse(&format!(
            r#"{{"software_version":"2026.10.2","system_version":"2026.10.1",
                "artifacts":{{"tfctl":"tfctl-armv7:{sum}","system":"system-2026.10.1.tar:{sum}"}}}}"#
        ))
        .unwrap();
        a.apply_config(&v);
        assert_eq!(a.software_version, "2026.10.2");
        assert_eq!(a.system_version, "2026.10.1");
        assert_eq!(a.artifacts.get("tfctl").unwrap().0, "tfctl-armv7");
        // No version files on a dev box: nothing is attempted.
        a.paths = update::Paths::under(&std::env::temp_dir().join("tfctl-nowhere"));
        a.maybe_auto_update();
        assert!(!a.exit && a.tried.is_empty());
        // The explicit action fails cleanly when the server is not there.
        a.cfg.server_url = "http://127.0.0.1:1".into();
        let e = a
            .action(
                "update-software",
                &json::parse(r#"{"version":"x"}"#).unwrap(),
            )
            .unwrap_err();
        assert!(!a.exit, "{e}");
        let _ = std::fs::remove_file(&a.conf_path);
    }

    #[test]
    fn running_the_attempted_version_is_no_skip() {
        // Slot b booted the update fine: the attempt marker still names it, but it runs.
        let (mut a, _rx, _) = agent("ranfine");
        let base = std::env::temp_dir().join(format!("tfctl-ok-{}", std::process::id()));
        a.paths = update::Paths::under(&base);
        std::fs::create_dir_all(base.join("root/etc")).unwrap();
        std::fs::write(
            base.join("root/etc/thinframe.system-version"),
            "2026.10.b\n",
        )
        .unwrap();
        sysupdate::mark_attempt(&a.paths, "2026.10.b").unwrap();
        a.apply_config(&json::parse(r#"{"system_version":"2026.10.b"}"#).unwrap());
        a.maybe_auto_update();
        let ev = a.events.lock().unwrap().clone();
        assert!(
            ev.iter().all(|(k, _)| k != "system-update-skipped"),
            "{ev:?}"
        );
        assert!(!a.busy.load(Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_file(&a.conf_path);
    }

    #[test]
    fn rolled_back_system_version_is_not_flashed_again() {
        let (mut a, _rx, _) = agent("rollback");
        let base = std::env::temp_dir().join(format!("tfctl-rb-{}", std::process::id()));
        a.paths = update::Paths::under(&base);
        std::fs::create_dir_all(base.join("root/etc")).unwrap();
        std::fs::write(
            base.join("root/etc/thinframe.system-version"),
            "2026.10.a\n",
        )
        .unwrap();
        sysupdate::mark_attempt(&a.paths, "2026.10.b").unwrap();
        let sum = "c".repeat(64);
        a.apply_config(
            &json::parse(&format!(
                r#"{{"system_version":"2026.10.b","artifacts":{{"system":"system-2026.10.b.tar:{sum}"}}}}"#
            ))
            .unwrap(),
        );
        a.maybe_auto_update();
        assert!(
            !a.busy.load(Ordering::SeqCst),
            "must not start a second flash"
        );
        assert!(a.tried.contains(&"2026.10.b".to_string()));
        let ev = a.events.lock().unwrap().clone();
        assert!(
            ev.iter().any(|(k, _)| k == "system-update-skipped"),
            "{ev:?}"
        );
        // Reported once, not on every config push.
        a.maybe_auto_update();
        let n = a.events.lock().unwrap().len();
        assert_eq!(n, ev.len());
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_file(&a.conf_path);
    }

    #[test]
    fn update_software_refused_while_a_system_update_runs() {
        let (mut a, _rx, _) = agent("busy");
        a.busy.store(true, Ordering::SeqCst);
        let e = a
            .action(
                "update-software",
                &json::parse(r#"{"version":"x"}"#).unwrap(),
            )
            .unwrap_err();
        assert!(e.to_string().contains("already running"), "{e}");
        assert!(!a.exit);
        let _ = std::fs::remove_file(&a.conf_path);
    }

    #[test]
    fn rolled_back_software_version_is_not_reinstalled() {
        let (mut a, _rx, _) = agent("swfailed");
        let base = std::env::temp_dir().join(format!("tfctl-swf-{}", std::process::id()));
        a.paths = update::Paths::under(&base);
        std::fs::create_dir_all(base.join("root/etc")).unwrap();
        std::fs::create_dir_all(base.join("data")).unwrap();
        std::fs::write(base.join("root/etc/thinframe.version"), "1.0\n").unwrap();
        std::fs::write(base.join("data/software-failed"), "2.0\n").unwrap();
        a.cfg.server_url = "http://127.0.0.1:1".into();
        a.apply_config(&json::parse(r#"{"software_version":"2.0"}"#).unwrap());
        a.maybe_auto_update();
        assert!(!a.exit);
        let ev = a.events.lock().unwrap().clone();
        assert_eq!(ev.len(), 1, "{ev:?}");
        assert_eq!(ev[0].0, "software-update-skipped");
        a.maybe_auto_update();
        assert_eq!(a.events.lock().unwrap().len(), 1, "reported once");
        let _ = std::fs::remove_dir_all(&base);
        let _ = std::fs::remove_file(&a.conf_path);
    }

    #[test]
    fn rejects_bad_server_url() {
        let (tx, _rx) = mpsc::channel();
        let cfg = config::parse("SERVER_URL=nas:8080\n");
        assert!(Agent::new(&cfg, tx, "/dev/null").is_err());
    }
}
