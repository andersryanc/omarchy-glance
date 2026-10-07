//! Providers: the backend's data sources. Each turns system state into the
//! JSON `state` of the widgets that use it (docs/backend-protocol.md).

use std::collections::VecDeque;
use std::fs;
use std::io::Read;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::SystemTime;

use serde_json::{Value, json};

use crate::config::{Spec, num, text};
use crate::log;
use crate::proc::{kill, popen, shell};
use crate::sources::{self, Source};

pub const COMMAND_TIMEOUT: f64 = 10.0; // kill a command widget's script after this long
pub const MEDIA_STATUS: &str = "omarchy-shell media status"; // the bar's own player selection, as JSON
pub const MEDIA_WATCH: [&str; 4] = [
    "dbus-monitor",
    "--session",
    "type='signal',path='/org/mpris/MediaPlayer2',interface='org.freedesktop.DBus.Properties'",
    "type='signal',interface='org.freedesktop.DBus',member='NameOwnerChanged',arg0namespace='org.mpris.MediaPlayer2'",
];
const USAGE_REFRESH: f64 = 60.0; // seconds between omarchy-agent-usage-update runs per provider
const MIC_LEVELS: usize = 64; // newest levels sent to clients

pub fn usage_dir() -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| crate::config::home().join(".local/state"), PathBuf::from)
        .join("omarchy/agents/usage")
}

/// Match the Omarchy panel's providerHasData, including its validated balance.
pub fn provider_has_data(record: &Value) -> bool {
    if !record.get("id").is_some_and(|id| id.as_str().is_some_and(|s| !s.is_empty())) {
        return false;
    }
    let positive = ["totalPrompts", "totalSessions", "activeDays", "todayPrompts", "todaySessions"]
        .iter().any(|key| record.get(key).and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
            .is_some_and(|n| n.is_finite() && n > 0.0));
    let limits = record.get("limits").and_then(Value::as_array).is_some_and(|l| !l.is_empty());
    let balance = record.get("balance").filter(|b| b.is_object()).and_then(|b| b.get("remaining"))
        .and_then(|v| v.as_f64().or_else(|| v.as_str()?.parse().ok()))
        .is_some_and(|n| n.is_finite() && n >= 0.0);
    positive || limits || balance
}

/// A command widget's text: the first line, or Waybar-style JSON
/// `{"text": ..., "class": ...}` where class urgent/critical marks it urgent.
pub fn parse_command_output(out: &str) -> (String, bool) {
    if out.starts_with('{') && let Ok(Value::Object(data)) = serde_json::from_str::<Value>(out) {
        let urgent = match data.get("class") {
            Some(Value::String(c)) => c == "urgent" || c == "critical",
            Some(Value::Array(a)) => a.iter().any(|c| matches!(c.as_str(), Some("urgent" | "critical"))),
            _ => false,
        };
        return (text(&data, "text", ""), urgent);
    }
    (out.lines().next().unwrap_or("").to_string(), false)
}

// --- shell jobs ------------------------------------------------------------

/// A script run every `interval` seconds (0: once), its output collected
/// without blocking, killed after COMMAND_TIMEOUT.
pub struct Job {
    cmd: String,
    interval: f64,
    pub next: f64,
    proc: Option<Child>,
    exit: Option<OwnedFd>, // a pidfd, once the script closed its output but runs on
    out: Vec<u8>,
    started: f64,
}

impl Job {
    pub fn new(cmd: &str, interval: f64) -> Job {
        Job { cmd: cmd.into(), interval, next: 0.0, proc: None, exit: None, out: vec![], started: 0.0 }
    }

    pub fn running(&self) -> bool {
        self.proc.is_some()
    }

    #[cfg(test)]
    pub fn interval(&self) -> f64 {
        self.interval
    }

    /// A new interval, counted from the last run.
    pub fn set_interval(&mut self, interval: f64) {
        if interval == self.interval {
            return;
        }
        self.interval = interval;
        if self.next > 0.0 {
            self.next = if interval > 0.0 { self.started + interval } else { f64::INFINITY };
        }
    }

    /// Start a run if one is due. Returns the output of a run that timed out.
    pub fn tick(&mut self, t: f64, what: &str) -> Option<String> {
        if self.proc.is_some() && t - self.started > COMMAND_TIMEOUT {
            log(&format!("{what}: command timed out"));
            let delivered = self.exit.take().is_some();
            kill(self.proc.take().unwrap());
            return (!delivered).then(|| String::from_utf8_lossy(&self.out).trim().to_string());
        }
        if self.proc.is_none() && t >= self.next {
            match shell(&self.cmd, true) {
                Ok(p) => {
                    self.proc = Some(p);
                    self.out.clear();
                    self.started = t;
                }
                Err(e) => log(&format!("{what}: could not run: {e}")),
            }
            self.next = if self.interval > 0.0 { t + self.interval } else { f64::INFINITY };
        }
        None
    }

    pub fn deadline(&self) -> f64 {
        if self.proc.is_some() { self.started + COMMAND_TIMEOUT } else { self.next }
    }

    pub fn fd(&self) -> Option<RawFd> {
        if let Some(exit) = &self.exit {
            return Some(exit.as_raw_fd());
        }
        self.proc.as_ref().and_then(|p| p.stdout.as_ref()).map(AsRawFd::as_raw_fd)
    }

    /// Read what's available; the trimmed output once the script closes it.
    pub fn readable(&mut self) -> Option<String> {
        if self.exit.take().is_some() {
            // The pidfd is readable: the script has exited, so this doesn't wait.
            if let Some(mut p) = self.proc.take() {
                let _ = p.wait();
            }
            return None;
        }
        let mut buf = vec![0u8; 65536];
        let n = self.proc.as_mut().and_then(|p| p.stdout.as_mut()).map_or(Ok(0), |s| s.read(&mut buf)).unwrap_or(0);
        if n > 0 && self.out.len() < 65536 {
            self.out.extend_from_slice(&buf[..n]);
            return None;
        }
        Some(self.finish())
    }

    /// The output is complete. A script still running after closing it is
    /// watched through a pidfd and counts as running until it exits or times
    /// out, so no run overlaps it and the loop never waits for it.
    fn finish(&mut self) -> String {
        if let Some(mut p) = self.proc.take() {
            drop(p.stdout.take());
            if matches!(p.try_wait(), Ok(None)) {
                let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, p.id(), 0) };
                if fd >= 0 {
                    self.exit = Some(unsafe { OwnedFd::from_raw_fd(fd as RawFd) });
                    self.proc = Some(p);
                } else {
                    kill(p);
                }
            }
        }
        String::from_utf8_lossy(&self.out).trim().to_string()
    }

    pub fn stop(&mut self) {
        self.exit = None;
        if let Some(p) = self.proc.take() {
            kill(p);
        }
    }
}

/// A `type: command` widget's script.
pub struct Command {
    pub job: Job,
    text: String,
    urgent: bool,
}

impl Command {
    pub fn new(spec: &Spec) -> Command {
        Command { job: Job::new(&text(spec, "exec", ""), num(spec, "interval", 0.0)), text: String::new(), urgent: false }
    }

    /// Take a finished run's output; true if the shown text changed.
    pub fn finished(&mut self, out: &str) -> bool {
        let (text, urgent) = parse_command_output(out);
        let changed = (text.as_str(), urgent) != (self.text.as_str(), self.urgent);
        (self.text, self.urgent) = (text, urgent);
        changed
    }

    pub fn state(&self) -> Value {
        json!({"text": self.text, "urgent": self.urgent})
    }

    #[cfg(test)]
    pub fn fixture(&mut self, text: &str, urgent: bool) {
        (self.text, self.urgent) = (text.into(), urgent);
    }
}

// --- media -----------------------------------------------------------------

/// The Omarchy shell's player state, refreshed on MPRIS changes.
pub struct Media {
    pub status: Job,
    pub watch: Option<Child>,
    watch_at: f64,
    state: Value,
}

impl Media {
    pub fn new(interval: f64) -> Media {
        Media { status: Job::new(MEDIA_STATUS, interval), watch: None, watch_at: 0.0, state: json!({}) }
    }

    pub fn set_interval(&mut self, interval: f64) {
        self.status.set_interval(interval);
    }

    /// Run `dbus-monitor` (restarted within 5 s if it dies) and, while the
    /// widget is on screen, the status script.
    pub fn tick(&mut self, t: f64, shown: bool) -> bool {
        if self.watch.as_mut().is_some_and(|p| !matches!(p.try_wait(), Ok(None))) {
            kill(self.watch.take().unwrap());
        }
        if self.watch.is_none() && t >= self.watch_at {
            self.watch_at = t + 5.0;
            match popen(&MEDIA_WATCH, true) {
                Ok(p) => self.watch = Some(p),
                Err(e) => log(&format!("media: could not run dbus-monitor: {e}")),
            }
        }
        if shown || self.status.running() {
            if let Some(out) = self.status.tick(t, "media") {
                return self.finished(&out);
            }
        }
        false
    }

    pub fn deadline(&self, shown: bool) -> f64 {
        let watch = if self.watch.is_none() { self.watch_at } else { f64::INFINITY };
        let status = if shown || self.status.running() { self.status.deadline() } else { f64::INFINITY };
        watch.min(status)
    }

    pub fn finished(&mut self, out: &str) -> bool {
        let state = serde_json::from_str::<Value>(out).ok().filter(Value::is_object).unwrap_or(json!({}));
        let changed = state != self.state;
        self.state = state;
        changed
    }

    /// dbus-monitor output: ask for state soon if a player changed.
    pub fn watch_readable(&mut self, now: f64) {
        let mut buf = vec![0u8; 65536];
        let n = self.watch.as_mut().and_then(|p| p.stdout.as_mut()).map_or(Ok(0), |s| s.read(&mut buf)).unwrap_or(0);
        if n == 0 {
            if let Some(p) = self.watch.take() {
                kill(p);
            }
            return;
        }
        let chunk = &buf[..n];
        let has = |needle: &[u8]| chunk.windows(needle.len()).any(|w| w == needle);
        if has(b"PropertiesChanged") || has(b"NameOwnerChanged") {
            self.status.next = self.status.next.min(now + 0.15);
        }
    }

    pub fn watch_fd(&self) -> Option<RawFd> {
        self.watch.as_ref().and_then(|p| p.stdout.as_ref()).map(AsRawFd::as_raw_fd)
    }

    pub fn state(&self) -> Value {
        self.state.clone()
    }

    #[cfg(test)]
    pub fn fixture(&mut self, state: Value) {
        self.state = state;
    }

    pub fn stop(&mut self) {
        self.status.stop();
        if let Some(p) = self.watch.take() {
            kill(p);
        }
    }
}

// --- agent usage -----------------------------------------------------------

/// One agent's Omarchy usage record, and the job that refreshes it.
pub struct Usage {
    agent: String,
    mtime: Option<Option<SystemTime>>, // None until first read
    record: Option<Value>,
    refresh_next: f64,
    refresh_job: Option<Child>,
}

impl Usage {
    pub fn new(agent: &str) -> Usage {
        Usage { agent: agent.into(), mtime: None, record: None, refresh_next: 0.0, refresh_job: None }
    }

    /// Re-read the record if it changed; true if it did.
    pub fn poll(&mut self, dir: &Path) -> bool {
        let path = dir.join(format!("{}.json", self.agent));
        let mt = fs::metadata(&path).and_then(|m| m.modified()).ok();
        if self.mtime == Some(mt) {
            return false;
        }
        self.mtime = Some(mt);
        let record = fs::read_to_string(&path).map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string()));
        if let Err(e) = &record && mt.is_some() {
            log(&format!("unreadable usage record {}: {e}", path.display()));
        }
        let record = record.ok();
        let changed = record != self.record;
        self.record = record;
        changed
    }

    /// Run Omarchy's collector every minute, one run at a time, even while
    /// the widget is hidden (that's how it gets data to show).
    pub fn refresh(&mut self, t: f64) {
        let agent = &self.agent;
        // The updater interprets leading dashes as options.
        if agent.is_empty() || !agent.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            || agent.starts_with('-') {
            return;
        }
        if let Some(child) = self.refresh_job.as_mut() {
            match child.try_wait() {
                Ok(None) => return,
                Ok(Some(status)) if !status.success() => log(&format!("usage refresh for {agent} exited with {status}")),
                Ok(Some(_)) => {}
                Err(e) => log(&format!("usage refresh for {agent}: {e}")),
            }
            self.refresh_job = None;
        }
        if t < self.refresh_next {
            return;
        }
        self.refresh_next = t + USAGE_REFRESH;
        match popen(&["timeout", "--kill-after=5s", "120s", "omarchy-agent-usage-update", agent], false) {
            Ok(child) => self.refresh_job = Some(child),
            Err(e) => log(&format!("could not refresh usage for {agent}: {e}")),
        }
    }

    #[cfg(test)]
    pub fn fixture(&mut self, record: Option<Value>) {
        self.record = record;
    }

    pub fn visible(&self) -> bool {
        self.record.as_ref().is_some_and(provider_has_data)
    }

    pub fn state(&self) -> Value {
        let mut limits = vec![];
        for entry in self.record.iter().filter_map(|r| r.get("limits")?.as_array()).flatten() {
            let Some(entry) = entry.as_object() else { continue };
            let frac = num(entry, "percent", -1.0);
            if frac >= 0.0 {
                let label = text(entry, "label", "");
                let label = label.split(" (").next().unwrap_or("").to_string();
                let resets = entry.get("resetsAt").and_then(Value::as_str);
                limits.push(json!({"label": if label.is_empty() { "Limit".into() } else { label },
                                   "fraction": frac, "resetsAt": resets}));
            }
        }
        if limits.is_empty() {
            limits = ["Session", "Weekly"].iter().map(|l| json!({"label": l, "fraction": null, "resetsAt": null})).collect();
        }
        json!({"visible": self.visible(), "limits": limits})
    }

    pub fn stop(&mut self) {
        if let Some(p) = self.refresh_job.take() {
            kill(p);
        }
    }
}

// --- graphs ----------------------------------------------------------------

/// A graph source and its history, shared by the widgets with the same id
/// and provider options.
pub struct Graph {
    id: String,
    source: Source,
    interval: f64,
    pub next: f64,
    history: Vec<VecDeque<f64>>,
    pub columns: usize,
    error: bool,
}

impl Graph {
    pub fn new(kind: sources::Kind, spec: &Spec, columns: usize) -> Graph {
        let source = Source::new(kind, spec);
        let series = if source.mirrored() { 2 } else { 1 };
        Graph {
            id: text(spec, "id", ""),
            source,
            interval: num(spec, "interval", 1.0).max(0.25),
            next: 0.0,
            history: vec![VecDeque::with_capacity(columns); series],
            columns,
            error: false,
        }
    }

    pub fn set_columns(&mut self, columns: usize) {
        self.columns = columns;
        for h in &mut self.history {
            while h.len() > columns {
                h.pop_front();
            }
        }
    }

    /// Fixed history (oldest first; only the newest that fit are kept) and
    /// source output.
    #[cfg(test)]
    pub fn fixture(&mut self, history: Vec<Vec<f64>>, lines: sources::Lines, cores: Vec<f64>, battery: Option<(i64, String)>) {
        for (h, values) in self.history.iter_mut().zip(history) {
            h.clear();
            h.extend(&values[values.len().saturating_sub(self.columns)..]);
        }
        (self.source.lines, self.source.cores, self.source.battery) = (lines, cores, battery);
    }

    /// Take a sample if one is due; true if it did.
    pub fn tick(&mut self, t: f64) -> bool {
        if t < self.next {
            return false;
        }
        self.next = t + self.interval;
        match self.source.sample() {
            Ok(values) => {
                for (history, v) in self.history.iter_mut().zip(values.unwrap_or_default()) {
                    if history.len() == self.columns {
                        history.pop_front();
                    }
                    history.push_back(v);
                }
                self.error = false;
            }
            Err(e) => {
                if !self.error {
                    log(&format!("{}: {e}", self.id));
                }
                self.error = true;
            }
        }
        true
    }

    pub fn state(&self) -> Value {
        let scale: Vec<f64> = self.history.iter().map(|h| match self.source.scale(h) {
            s if s != 0.0 => s,
            _ => 1.0,
        }).collect();
        json!({
            "series": self.history,
            "scale": scale,
            "lines": self.source.lines,
            "cores": self.source.cores,
            "battery": self.source.battery.as_ref().map(|(charge, status)| json!({"charge": charge, "status": status})),
            "error": self.error,
        })
    }
}

/// The newest mic levels for clients.
pub fn mic_state(mic: &crate::mic::Mic, fps: i64) -> Value {
    let skip = mic.levels.len().saturating_sub(MIC_LEVELS);
    let levels: Vec<f64> = mic.levels.iter().skip(skip).copied().collect();
    json!({"muted": mic.muted, "inUse": mic.in_use, "levels": levels, "fps": fps})
}
