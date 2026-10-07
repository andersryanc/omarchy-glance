//! Microphone state for the glance.mic widget.
//!
//! Tracks whether the default source is muted and whether any other app is
//! recording from it, by listening to `pactl subscribe` and re-reading
//! PulseAudio (PipeWire) state when sources or recording streams change. While
//! another app records and the mic is live, it opens its own small capture with
//! `parec` to compute levels for the waveform, and closes it as soon as that app
//! stops, so the mic is never held open just for the Touch Bar.

use std::collections::{HashSet, VecDeque};
use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};
use std::process::Child;

use serde_json::Value;

use crate::backend::providers::Job;
use crate::proc::{kill, popen};
use crate::{log, now};

const RATE: usize = 8000; // level-meter sample rate (mono s16le)
const FLOOR_DB: f64 = -50.0; // levels below this draw as silence
const RESUBSCRIBE_SECONDS: f64 = 5.0; // retry if `pactl subscribe` dies
const RECHECK_SECONDS: f64 = 5.0; // re-read state even without events
// The default source, the sources and the recording streams, one per line
// (pactl's JSON is one line); no output if any of them fails.
const QUERY: &str = "set -e; d=$(pactl get-default-source); s=$(pactl -f json list sources); \
                     o=$(pactl -f json list source-outputs); printf '%s\\n%s\\n%s\\n' \"$d\" \"${s:-[]}\" \"${o:-[]}\"";

pub struct Mic {
    frame_bytes: usize,
    pub muted: Option<bool>, // None until the first read
    pub in_use: bool,        // another app is recording from the default source
    pub levels: VecDeque<f64>, // 0-1, newest last
    sub: Option<Child>,
    rec: Option<Child>,
    query: Job,     // reads the state without blocking the loop
    requery: bool,  // something changed while it ran
    pcm: Vec<u8>,
    events: Vec<u8>,
    refresh_at: f64,
    sub_at: f64,
    pub changed: bool, // state or levels changed since the renderer looked
    wanted: bool,      // a mic widget is on screen; levels are only captured then
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MicFd {
    Events,
    Audio,
    State,
}

/// Bytes of 16-bit audio per level sample, for `fps` samples a second.
fn frame_bytes(fps: i64) -> usize {
    2 * (RATE / fps.max(1) as usize).max(1)
}

impl Mic {
    pub fn set_fps(&mut self, fps: i64) {
        self.frame_bytes = frame_bytes(fps);
    }

    pub fn new(fps: i64) -> Mic {
        Mic {
            frame_bytes: frame_bytes(fps),
            muted: None,
            in_use: false,
            levels: VecDeque::with_capacity(512),
            sub: None,
            rec: None,
            query: Job::new(QUERY, 0.0),
            requery: false,
            pcm: vec![],
            events: vec![],
            refresh_at: 0.0,
            sub_at: 0.0,
            changed: true,
            wanted: true,
        }
    }

    pub fn close(&mut self) {
        for p in [self.sub.take(), self.rec.take()].into_iter().flatten() {
            kill(p);
        }
        self.query.stop();
    }

    pub fn tick(&mut self, t: f64) {
        if self.sub.as_mut().is_some_and(|p| !matches!(p.try_wait(), Ok(None))) {
            self.sub = None;
        }
        if self.sub.is_none() && t >= self.sub_at {
            self.sub_at = t + RESUBSCRIBE_SECONDS;
            match popen(&["pactl", "subscribe"], true) {
                Ok(p) => {
                    self.sub = Some(p);
                    self.refresh_at = t;
                }
                Err(e) => log(&format!("mic: could not run pactl: {e}")),
            }
        }
        if self.rec.as_mut().is_some_and(|p| !matches!(p.try_wait(), Ok(None))) {
            self.stop_levels();
            self.refresh_at = t;
        }
        if t >= self.refresh_at {
            self.refresh_at = t + RECHECK_SECONDS;
            if self.query.running() {
                self.requery = true;
            } else {
                self.query.next = t;
            }
        }
        if let Some(out) = self.query.tick(t, "mic") {
            self.query_done(&out);
        }
    }

    pub fn set_wanted(&mut self, wanted: bool) {
        if wanted != self.wanted {
            self.wanted = wanted;
            self.refresh_at = 0.0; // start or stop the level meter now
        }
    }

    pub fn deadline(&self) -> f64 {
        self.refresh_at.min(self.query.deadline()).min(if self.sub.is_none() { self.sub_at } else { f64::INFINITY })
    }

    pub fn fds(&self) -> Vec<(RawFd, MicFd)> {
        let mut out = vec![];
        if let Some(fd) = self.sub.as_ref().and_then(|p| p.stdout.as_ref()) {
            out.push((fd.as_raw_fd(), MicFd::Events));
        }
        if let Some(fd) = self.rec.as_ref().and_then(|p| p.stdout.as_ref()) {
            out.push((fd.as_raw_fd(), MicFd::Audio));
        }
        if let Some(fd) = self.query.fd() {
            out.push((fd, MicFd::State));
        }
        out
    }

    pub fn readable(&mut self, which: MicFd) {
        match which {
            MicFd::Events => self.read_events(),
            MicFd::Audio => self.read_audio(),
            MicFd::State => {
                if let Some(out) = self.query.readable() {
                    self.query_done(&out);
                }
            }
        }
    }

    fn read_chunk(child: &mut Option<Child>) -> Vec<u8> {
        let mut buf = vec![0u8; 65536];
        let n = child.as_mut().and_then(|p| p.stdout.as_mut()).map_or(Ok(0), |s| s.read(&mut buf)).unwrap_or(0);
        buf.truncate(n);
        buf
    }

    // --- state --------------------------------------------------------------
    fn read_events(&mut self) {
        let chunk = Self::read_chunk(&mut self.sub);
        if chunk.is_empty() {
            if let Some(p) = self.sub.take() {
                kill(p);
            }
            return;
        }
        self.events.extend_from_slice(&chunk);
        let len = self.events.len();
        if len > 4096 {
            self.events.drain(..len - 4096);
        }
        // Sources (mute), recording streams, and the server (default source) matter.
        let has = |needle: &[u8]| self.events.windows(needle.len()).any(|w| w == needle);
        if has(b"on source") || has(b"on server") {
            self.refresh_at = self.refresh_at.min(now() + 0.1);
        }
        let keep = self.events.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
        self.events.drain(..keep);
    }

    /// The query finished: use its output, and run it again if asked meanwhile.
    fn query_done(&mut self, out: &str) {
        match Self::parse_state(out) {
            Ok((default, sources, outputs)) => self.apply(&default, &sources, &outputs),
            Err(e) => log(&format!("mic: could not read PulseAudio state: {e}")),
        }
        if std::mem::take(&mut self.requery) {
            self.query.next = 0.0;
        }
    }

    fn parse_state(out: &str) -> Result<(String, Value, Value), String> {
        let mut lines = out.lines();
        let (Some(default), Some(sources), Some(outputs)) = (lines.next(), lines.next(), lines.next()) else {
            return Err("pactl failed".into());
        };
        let json = |s: &str| serde_json::from_str::<Value>(s).map_err(|e| e.to_string());
        Ok((default.trim().to_string(), json(sources)?, json(outputs)?))
    }

    fn apply(&mut self, default: &str, sources: &Value, outputs: &Value) {
        let empty = vec![];
        let sources = sources.as_array().unwrap_or(&empty);
        let outputs = outputs.as_array().unwrap_or(&empty);
        let prop = |v: &Value, k: &str| v.get("properties").and_then(|p| p.get(k)).cloned();
        let source = sources.iter().find(|s| s.get("name").and_then(Value::as_str) == Some(default));
        let muted = source.is_none_or(|s| s.get("mute").and_then(Value::as_bool).unwrap_or(false));
        let mics: HashSet<i64> = sources
            .iter()
            .filter(|s| prop(s, "device.class").and_then(|v| v.as_str().map(String::from)).as_deref() != Some("monitor"))
            .filter_map(|s| s.get("index").and_then(Value::as_i64))
            .collect();
        let own = self.rec.as_ref().map(|p| Value::String(p.id().to_string()));
        let flag = |o: &Value, k: &str| o.get(k).and_then(Value::as_bool).unwrap_or(false);
        let in_use = outputs.iter().any(|o| {
            o.get("source").and_then(Value::as_i64).is_some_and(|s| mics.contains(&s))
                && !flag(o, "corked")
                && !flag(o, "mute")
                && (own.is_none() || prop(o, "application.process.id") != own)
        });
        if (Some(muted), in_use) != (self.muted, self.in_use) {
            self.muted = Some(muted);
            self.in_use = in_use;
            self.changed = true;
        }
        if in_use && !muted && source.is_some() && self.wanted {
            if self.rec.is_none() {
                self.start_levels(default);
            }
        } else if self.rec.is_some() {
            self.stop_levels();
        }
    }

    // --- level meter --------------------------------------------------------
    fn start_levels(&mut self, device: &str) {
        let (rate, dev) = (format!("--rate={RATE}"), format!("--device={device}"));
        let args = ["parec", "--raw", "--format=s16le", &rate, "--channels=1", "--latency-msec=20", &dev,
                    "--client-name=omarchy-glance", "--stream-name=Touch Bar level meter"];
        match popen(&args, true) {
            Ok(p) => {
                self.rec = Some(p);
                self.pcm.clear();
            }
            Err(e) => log(&format!("mic: could not run parec: {e}")),
        }
    }

    fn stop_levels(&mut self) {
        if let Some(p) = self.rec.take() {
            kill(p);
        }
        self.levels.clear();
        self.changed = true;
    }

    fn read_audio(&mut self) {
        let chunk = Self::read_chunk(&mut self.rec);
        if chunk.is_empty() {
            self.stop_levels();
            return;
        }
        self.pcm.extend_from_slice(&chunk);
        let n = self.frame_bytes;
        let mut used = 0;
        while self.pcm.len() - used >= n {
            let peak = self.pcm[used..used + n]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|s| i32::from(i16::from_le_bytes(*s)).abs())
                .fold(1, i32::max);
            used += n;
            let db = 20.0 * (f64::from(peak) / 32768.0).log10();
            if self.levels.len() == 512 {
                self.levels.pop_front();
            }
            self.levels.push_back(((db - FLOOR_DB) / -FLOOR_DB).clamp(0.0, 1.0));
            self.changed = true;
        }
        self.pcm.drain(..used);
    }
}
