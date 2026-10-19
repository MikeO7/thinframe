//! Agent, second half: applying a server config object and running one-shot actions.
//! Pure functions of (agent state, JSON) plus KioskCmd sends, so they are tested here with a
//! channel and a temp conf file. The socket loop lives in agent.rs.
use crate::agent::Agent;
use crate::config;
use crate::json::Value;
use crate::kiosk::KioskCmd;
use crate::sysupdate;
use crate::update::{self, Artifacts};
use std::io;
use std::process::Command;
use std::sync::atomic::Ordering;
use std::thread;
use std::time::Duration;

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
            "rollback" => {
                let slot = sysupdate::rollback(&self.paths)?;
                self.event("rollback", &format!("booting slot {} next", slot.name()));
                sysupdate::reboot_soon();
                Ok(())
            }
            other => Err(io::Error::other(format!("unknown action '{other}'"))),
        }
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
            if !want.is_empty() && have != want && !self.tried.contains(&want) {
                self.tried.push(want.clone());
                eprintln!("tfctl: agent: software {have} -> {want}");
                match update::software(&self.cfg, &self.artifacts, &want, &self.paths) {
                    Ok(()) => self.exit = true,
                    Err(e) => self.event("software-update-failed", &format!("{want}: {e}")),
                }
            }
        }
        let want = self.system_version.clone();
        if let Some(have) = sysupdate::system_version(&self.paths) {
            if !want.is_empty()
                && have != want
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
    fn rejects_bad_server_url() {
        let (tx, _rx) = mpsc::channel();
        let cfg = config::parse("SERVER_URL=nas:8080\n");
        assert!(Agent::new(&cfg, tx, "/dev/null").is_err());
    }
}
