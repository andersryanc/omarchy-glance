//! The backend service: config, providers, widget state and actions for every
//! connected output client (docs/backend-protocol.md, ADR 0001).
//!
//! `Backend` is transport-agnostic: callers feed it request lines per session
//! and drain reply lines; `server` runs it on the Unix socket.

pub mod providers;
pub mod server;
#[cfg(test)]
mod tests;

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs;
use std::os::fd::RawFd;
use std::path::PathBuf;
use std::process::Child;
use std::time::SystemTime;

use serde_json::{Map, Value, json};

use crate::config::{Config, Layer, Rgb, Spec, int, text, truthy};
use crate::mic::{Mic, MicFd};
use crate::proc::shell;
use crate::protocol::{self, Output, WidgetKind, error};
use crate::sources;
use crate::log;
use providers::{Command, Graph, Media, Usage};

const POLL_SECONDS: f64 = 1.0; // config files and usage records
const UPDATE_SECONDS: f64 = 0.016; // at most one update per session this often
const MAX_REPLIES: usize = 256; // unsent replies before a client is dropped
const AGENTS_TOGGLE: &str = "omarchy-shell -q omarchy.agents toggle";
const ACTIVITY: &str = "omarchy-launch-or-focus-tui btop"; // what Super+Ctrl+T opens
const MIC_TOGGLE: &str = "omarchy-audio-input-mute"; // what the mic-mute key runs (with OSD)
const MEDIA_ZONES: [&str; 3] = ["previous", "playPause", "next"];

// Options that change what a graph provider samples; widgets that agree on
// these and the id share one provider.
const GRAPH_PROVIDER_OPTIONS: [&str; 18] = [
    "interval", "sensor", "card", "interface", "device", "show", "mount", "battery", "fan", "temperature",
    "cores", "graph", "detail", "low", "alarm", "temperatureAlarm", "minScale", "maxPower",
];

/// Where the backend reads its files; tests point these at a temp dir.
pub struct Paths {
    pub config_dir: PathBuf,
    pub usage_dir: PathBuf,
}

impl Paths {
    pub fn user() -> Paths {
        let touchbar = crate::config::user_config();
        Paths { config_dir: touchbar.parent().unwrap().to_path_buf(), usage_dir: providers::usage_dir() }
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum ProviderKey {
    Graph(String),
    Command(String),
    Usage(String),
    Media,
    Mic,
}

enum Provider {
    Graph(Graph),
    Command(Command),
    Usage(Usage),
    Media(Media),
    Mic(Mic, i64), // fps
}

impl Provider {
    fn stop(&mut self) {
        match self {
            Provider::Command(c) => c.job.stop(),
            Provider::Usage(u) => u.stop(),
            Provider::Media(m) => m.stop(),
            Provider::Mic(m, _) => m.close(),
            Provider::Graph(_) => {}
        }
    }
}

/// A file descriptor the backend wants polled, and what it belongs to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Token {
    Command(String),
    MediaStatus,
    MediaWatch,
    Mic(MicFd),
}

struct WidgetDef {
    key: String,
    id: String,
    kind: WidgetKind,
    layer: Layer,
    section: &'static str,
    spec: Spec,
    unsupported: Option<&'static str>,
    provider: Option<ProviderKey>,
}

impl WidgetDef {
    fn layer_name(&self) -> &'static str {
        if self.layer == Layer::Fn { "fn" } else { "default" }
    }

    /// The configured backend action for a press (zone for media).
    fn command(&self, zone: Option<&str>) -> Option<String> {
        let on_tap = |default: &str| Some(text(&self.spec, "onTap", default)).filter(|c| !c.is_empty());
        match self.kind {
            WidgetKind::Button if truthy(self.spec.get("key")) => None,
            WidgetKind::Button => Some(text(&self.spec, "exec", "")).filter(|c| !c.is_empty()),
            WidgetKind::Command => on_tap(""),
            WidgetKind::Agents => on_tap(AGENTS_TOGGLE),
            WidgetKind::Graph => on_tap(ACTIVITY),
            WidgetKind::Mic => on_tap(MIC_TOGGLE),
            WidgetKind::Media => match zone? {
                z if MEDIA_ZONES.contains(&z) => Some(format!("omarchy-shell -q media {z}")),
                "title" => on_tap(""),
                _ => None,
            },
            WidgetKind::Esc | WidgetKind::Spacer => None,
        }
    }

    fn repeats(&self) -> bool {
        self.kind == WidgetKind::Button && truthy(self.spec.get("repeat"))
    }

    /// What a press does, for clients.
    fn action(&self) -> Value {
        match self.kind {
            WidgetKind::Esc => json!({"key": "esc", "repeat": false}),
            WidgetKind::Button if truthy(self.spec.get("key")) => {
                json!({"key": text(&self.spec, "key", "").to_lowercase(), "repeat": self.repeats()})
            }
            WidgetKind::Media => {
                let mut zones: Vec<&str> = MEDIA_ZONES.to_vec();
                if self.command(Some("title")).is_some() {
                    zones.push("title");
                }
                json!({"zones": zones})
            }
            _ if self.command(None).is_some() => json!({"press": true, "repeat": self.repeats()}),
            _ => Value::Null,
        }
    }

    /// The config object without action commands.
    fn options(&self) -> Value {
        let mut options = self.spec.clone();
        options.remove("exec");
        options.remove("onTap");
        Value::Object(options)
    }
}

struct OutputConfig {
    generation: u64,
    source: Option<(PathBuf, Option<SystemTime>)>,
    path_label: String,
    error: Option<String>,
    config: Config,
    widgets: Vec<WidgetDef>,
}

impl OutputConfig {
    fn widget(&self, key: &str) -> Option<&WidgetDef> {
        self.widgets.iter().find(|w| w.key == key)
    }
}

enum Out {
    Line(String),
    Snapshot,
}

struct Press {
    widget: String,
    next: Option<f64>, // next repeat
}

struct Session {
    name: String,
    output: Option<Output>, // None until hello
    layer: Layer,
    shown: bool,
    rev: u64,
    out: VecDeque<Out>,
    dirty: HashSet<String>,
    last_update: f64,
    pointers: HashMap<i64, Press>,
    closing: bool,
}

pub struct Backend {
    paths: Paths,
    outputs: HashMap<Output, OutputConfig>,
    providers: HashMap<ProviderKey, Provider>,
    changed: HashSet<ProviderKey>,
    sessions: HashMap<u64, Session>,
    next_session: u64,
    next_poll: f64,
    children: Vec<Child>, // actions, reaped as they exit
}

fn hex(c: Rgb) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0] as u8, c[1] as u8, c[2] as u8)
}

fn reply(id: &Value, extra: Value) -> String {
    let mut msg = Map::new();
    msg.insert("type".into(), "ack".into());
    msg.insert("id".into(), id.clone());
    if let Value::Object(extra) = extra {
        msg.extend(extra);
    }
    Value::Object(msg).to_string()
}

fn error_line(id: Option<&Value>, code: &str, message: &str) -> String {
    let mut msg = json!({"type": "error", "code": code, "message": message});
    if let Some(id) = id {
        msg["id"] = id.clone();
    }
    msg.to_string()
}

impl Backend {
    pub fn new(paths: Paths) -> Backend {
        Backend {
            paths,
            outputs: HashMap::new(),
            providers: HashMap::new(),
            changed: HashSet::new(),
            sessions: HashMap::new(),
            next_session: 1,
            next_poll: 0.0,
            children: vec![],
        }
    }

    // --- sessions -----------------------------------------------------------
    pub fn connect(&mut self) -> u64 {
        let id = self.next_session;
        self.next_session += 1;
        let name = format!("{:04x}", (id.wrapping_mul(0x9E37) ^ std::process::id() as u64) & 0xFFFF);
        self.sessions.insert(id, Session {
            name,
            output: None,
            layer: Layer::Default,
            shown: true,
            rev: 0,
            out: VecDeque::new(),
            dirty: HashSet::new(),
            last_update: f64::NEG_INFINITY,
            pointers: HashMap::new(),
            closing: false,
        });
        id
    }

    /// Forget a session: its presses, repeats and demand go with it.
    pub fn disconnect(&mut self, id: u64, t: f64) {
        if let Some(s) = self.sessions.remove(&id)
            && let Some(output) = s.output {
            log(&format!("session {} ({}) disconnected", s.name, output.name()));
        }
        self.sync_providers(t);
    }

    /// Whether the session hit a fatal error and should be closed once its
    /// queued lines are sent.
    pub fn closing(&self, id: u64) -> bool {
        self.sessions.get(&id).is_none_or(|s| s.closing)
    }

    fn push(&mut self, id: u64, line: String) {
        let Some(s) = self.sessions.get_mut(&id) else { return };
        if s.closing {
            return;
        }
        let replies = s.out.iter().filter(|o| matches!(o, Out::Line(_))).count();
        if replies >= MAX_REPLIES {
            s.out.push_back(Out::Line(error_line(None, error::QUEUE_OVERFLOW, "too many unread replies")));
            s.closing = true;
            return;
        }
        s.out.push_back(Out::Line(line));
    }

    fn fail(&mut self, id: u64, req: Option<&Value>, code: &str, message: &str) {
        self.push(id, error_line(req, code, message));
        if error::fatal(code) && let Some(s) = self.sessions.get_mut(&id) {
            s.closing = true;
        }
    }

    fn queue_snapshot(&mut self, id: u64) {
        if let Some(s) = self.sessions.get_mut(&id)
            && !s.out.iter().any(|o| matches!(o, Out::Snapshot)) {
            s.out.push_back(Out::Snapshot);
        }
    }

    /// A line longer than the protocol allows (the transport noticed).
    pub fn too_large(&mut self, id: u64) {
        self.fail(id, None, error::TOO_LARGE, "message longer than 1 MiB");
    }

    /// One request line from a client.
    pub fn handle_line(&mut self, id: u64, line: &str, t: f64) {
        let msg: Value = match serde_json::from_str(line) {
            Ok(Value::Object(m)) => Value::Object(m),
            Ok(_) => return self.fail(id, None, error::BAD_MESSAGE, "expected a JSON object"),
            Err(e) => return self.fail(id, None, error::BAD_MESSAGE, &format!("invalid JSON: {e}")),
        };
        let req = msg.get("id").filter(|v| v.is_i64() || v.is_u64());
        let Some(kind) = msg.get("type").and_then(Value::as_str) else {
            return self.fail(id, req, error::BAD_MESSAGE, "missing \"type\"");
        };
        let Some(req) = req.cloned() else {
            return self.fail(id, None, error::BAD_MESSAGE, &format!("{kind}: missing integer \"id\""));
        };
        let ready = self.sessions.get(&id).is_some_and(|s| s.output.is_some());
        match kind {
            "hello" if ready => self.fail(id, Some(&req), error::BAD_MESSAGE, "already said hello"),
            "hello" => self.hello(id, &req, &msg, t),
            "press" | "release" | "activate" | "view" | "resync" if !ready => {
                self.fail(id, Some(&req), error::NOT_READY, "send hello first")
            }
            "press" | "activate" => self.press(id, &req, &msg, kind == "press", t),
            "release" => self.release(id, &req, &msg),
            "view" => self.view(id, &req, &msg, t),
            "resync" => {
                self.push(id, reply(&req, Value::Null));
                self.queue_snapshot(id);
            }
            _ => self.fail(id, Some(&req), error::BAD_MESSAGE, &format!("unknown message type {kind:?}")),
        }
    }

    fn hello(&mut self, id: u64, req: &Value, msg: &Value, t: f64) {
        if msg.get("protocol").and_then(Value::as_i64) != Some(protocol::PROTOCOL) {
            return self.fail(id, Some(req), error::UNSUPPORTED_PROTOCOL, "this backend speaks protocol 1");
        }
        let name = msg.get("output").and_then(Value::as_str).unwrap_or("");
        let Some(output) = Output::parse(name) else {
            return self.fail(id, Some(req), error::UNSUPPORTED_OUTPUT, &format!("unknown output {name:?}"));
        };
        if !self.outputs.contains_key(&output) {
            self.load_config(output);
        }
        let s = self.sessions.get_mut(&id).unwrap();
        s.output = Some(output);
        let session = s.name.clone();
        let client = text(msg.as_object().unwrap(), "client", "?");
        log(&format!("session {session} ({}) connected: {client}", output.name()));
        self.push(id, reply(req, Value::Null));
        self.push(id, json!({"type": "welcome", "protocol": protocol::PROTOCOL, "session": session,
                             "backend": concat!("omarchy-glance ", env!("CARGO_PKG_VERSION")),
                             "features": ["repeat", "demand"]}).to_string());
        self.queue_snapshot(id);
        self.sync_providers(t);
    }

    fn press(&mut self, id: u64, req: &Value, msg: &Value, hold: bool, t: f64) {
        let s = &self.sessions[&id];
        let output = s.output.unwrap();
        let oc = &self.outputs[&output];
        let key = msg.get("widget").and_then(Value::as_str).unwrap_or("");
        let pointer = msg.get("pointer").and_then(Value::as_i64);
        if hold && pointer.is_none() {
            return self.fail(id, Some(req), error::BAD_MESSAGE, "press needs an integer \"pointer\"");
        }
        if msg.get("generation").and_then(Value::as_u64) != Some(oc.generation) {
            return self.fail(id, Some(req), error::STALE_CONFIG,
                             &format!("config generation is {}", oc.generation));
        }
        let Some(w) = oc.widget(key) else {
            return self.fail(id, Some(req), error::UNKNOWN_WIDGET, &format!("no widget {key}"));
        };
        if w.unsupported.is_some() {
            let message = format!("{key} ({}) has no action on {}", w.id, output.name());
            return self.fail(id, Some(req), error::NOT_PRESSABLE, &message);
        }
        if !self.widget_visible(w) {
            return self.fail(id, Some(req), error::HIDDEN, &format!("{key} is hidden"));
        }
        let zone = msg.get("zone").and_then(Value::as_str);
        if w.kind == WidgetKind::Media && zone.is_none() {
            return self.fail(id, Some(req), error::BAD_MESSAGE, "media needs a \"zone\"");
        }
        let Some(cmd) = w.command(zone) else {
            return self.fail(id, Some(req), error::NOT_PRESSABLE, &format!("{key} ({}) has no backend action", w.id));
        };
        let (repeats, is_media) = (w.repeats(), w.kind == WidgetKind::Media);
        let delay = oc.config.repeat_delay;
        self.spawn(&cmd);
        if is_media && let Some(Provider::Media(m)) = self.providers.get_mut(&ProviderKey::Media) {
            m.status.next = m.status.next.min(t + 0.3); // show the new state soon
        }
        let s = self.sessions.get_mut(&id).unwrap();
        if let Some(pointer) = pointer.filter(|_| hold) {
            s.pointers.insert(pointer, Press { widget: key.to_string(), next: repeats.then_some(t + delay) });
        }
        self.push(id, reply(req, Value::Null));
    }

    fn release(&mut self, id: u64, req: &Value, msg: &Value) {
        let Some(pointer) = msg.get("pointer").and_then(Value::as_i64) else {
            return self.fail(id, Some(req), error::BAD_MESSAGE, "release needs an integer \"pointer\"");
        };
        if self.sessions.get_mut(&id).unwrap().pointers.remove(&pointer).is_none() {
            return self.fail(id, Some(req), error::UNKNOWN_POINTER, &format!("pointer {pointer} isn't pressed"));
        }
        self.push(id, reply(req, Value::Null));
    }

    fn view(&mut self, id: u64, req: &Value, msg: &Value, t: f64) {
        let layer = match msg.get("layer").and_then(Value::as_str) {
            Some("default") => Layer::Default,
            Some("fn") => Layer::Fn,
            _ => return self.fail(id, Some(req), error::BAD_MESSAGE, "view needs \"layer\": \"default\" or \"fn\""),
        };
        let Some(shown) = msg.get("shown").and_then(Value::as_bool) else {
            return self.fail(id, Some(req), error::BAD_MESSAGE, "view needs a boolean \"shown\"");
        };
        let s = self.sessions.get_mut(&id).unwrap();
        (s.layer, s.shown) = (layer, shown);
        self.push(id, reply(req, Value::Null));
        self.sync_providers(t);
    }

    fn spawn(&mut self, cmd: &str) {
        match shell(cmd, false) {
            Ok(p) => self.children.push(p),
            Err(e) => log(&format!("could not run {cmd:?}: {e}")),
        }
    }

    // --- output -------------------------------------------------------------
    /// The session's queued lines, then a coalesced update if one is due.
    /// Call when the transport can take more.
    pub fn drain(&mut self, id: u64, t: f64) -> Vec<String> {
        let mut lines = vec![];
        loop {
            let Some(s) = self.sessions.get_mut(&id) else { return lines };
            match s.out.pop_front() {
                Some(Out::Line(l)) => lines.push(l),
                Some(Out::Snapshot) => {
                    let snapshot = self.snapshot(id);
                    lines.push(snapshot);
                }
                None => break,
            }
        }
        let s = self.sessions.get_mut(&id).unwrap();
        if s.closing || s.output.is_none() || s.dirty.is_empty() || t - s.last_update < UPDATE_SECONDS {
            return lines;
        }
        s.rev += 1;
        s.last_update = t;
        let (rev, dirty) = (s.rev, std::mem::take(&mut s.dirty));
        let oc = &self.outputs[&s.output.unwrap()];
        let mut widgets = Map::new();
        for key in dirty {
            if let Some(w) = oc.widget(&key) {
                widgets.insert(key, self.widget_state(w));
            }
        }
        lines.push(json!({"type": "update", "rev": rev, "widgets": widgets}).to_string());
        lines
    }

    /// When the session will have an update to send, if it has one waiting.
    pub fn update_due(&self, id: u64) -> Option<f64> {
        let s = self.sessions.get(&id)?;
        (!s.dirty.is_empty() && s.output.is_some()).then_some(s.last_update + UPDATE_SECONDS)
    }

    fn snapshot(&mut self, id: u64) -> String {
        let s = self.sessions.get_mut(&id).unwrap();
        s.rev += 1;
        s.dirty.clear();
        let rev = s.rev;
        let oc = &self.outputs[&s.output.unwrap()];
        let c = &oc.config;
        let d = &c.desktop;
        let settings = if s.output == Some(Output::Desktop) { json!({
            "colors": d.colors,
            "font": d.font,
            "monitors": d.monitors,
            "height": d.height,
            "repeatDelay": c.repeat_delay,
            "repeatInterval": c.repeat_interval,
            "hasFn": false,
        }) } else { json!({
            "colors": {"background": hex(c.background), "key": hex(c.key), "keyPressed": hex(c.key_pressed),
                       "text": hex(c.text), "urgent": hex(c.urgent), "debugBackground": hex(c.debug_bg),
                       "debugBackgroundFn": hex(c.debug_bg_fn)},
            "font": c.font,
            "idleDimSeconds": c.idle_dim,
            "repeatDelay": c.repeat_delay,
            "repeatInterval": c.repeat_interval,
            "debug": {"background": c.debug_background, "border": c.border, "testPattern": c.test_pattern},
            "hasFn": c.has_fn,
        }) };
        let widgets: Vec<Value> = oc.widgets.iter().map(|w| {
            let mut v = json!({
                "key": w.key, "id": w.id, "kind": w.kind.name(), "layer": w.layer_name(), "section": w.section,
                "supported": w.unsupported.is_none(), "options": w.options(),
                "action": if w.unsupported.is_some() { Value::Null } else { w.action() },
                "state": self.widget_state(w),
            });
            if let Some(reason) = w.unsupported {
                v["reason"] = reason.into();
            }
            v
        }).collect();
        json!({"type": "snapshot", "rev": rev,
               "config": {"generation": oc.generation, "path": oc.path_label, "error": oc.error},
               "settings": settings, "widgets": widgets}).to_string()
    }

    fn widget_state(&self, w: &WidgetDef) -> Value {
        match w.provider.as_ref().and_then(|k| self.providers.get(k)) {
            Some(Provider::Graph(g)) => g.state(),
            Some(Provider::Command(c)) => c.state(),
            Some(Provider::Usage(u)) => u.state(),
            Some(Provider::Media(m)) => m.state(),
            Some(Provider::Mic(m, fps)) => providers::mic_state(m, *fps),
            None => match w.kind {
                WidgetKind::Command => json!({"text": "", "urgent": false}),
                WidgetKind::Agents => json!({"visible": false, "limits": []}),
                _ => json!({}),
            },
        }
    }

    fn widget_visible(&self, w: &WidgetDef) -> bool {
        match w.provider.as_ref().and_then(|k| self.providers.get(k)) {
            Some(Provider::Usage(u)) => u.visible(),
            _ => w.kind != WidgetKind::Agents,
        }
    }

    // --- config -------------------------------------------------------------
    /// (Re)load an output's config; keeps the previous one on errors.
    fn load_config(&mut self, output: Output) {
        let path = self.paths.config_dir.join(output.file_name());
        let (default_name, default) = output.default_config();
        let user = path.exists();
        let source = user.then(|| (path.clone(), fs::metadata(&path).and_then(|m| m.modified()).ok()));
        let previous = self.outputs.get(&output).map(|p| (p.generation, p.source == source));
        if previous.is_some_and(|(_, same)| same) {
            return;
        }
        let label = if user { path.display().to_string() } else { format!("<built-in {default_name}>") };
        let text = if user { fs::read_to_string(&path).map_err(|e| e.to_string()) } else { Ok(default.to_string()) };
        let parsed = text.and_then(|s| output.parse_config(&s));
        let (config, error, label) = match parsed {
            Ok(c) => (c, None, label),
            Err(e) => {
                log(&format!("{label}: {e}; {}", if previous.is_some() { "keeping the previous config" } else { "using the default" }));
                if let Some(p) = self.outputs.get_mut(&output) {
                    p.source = source;
                    p.error = Some(e);
                    // Same config and generation, but clients show the error.
                    let ids: Vec<u64> = self.sessions.iter().filter(|(_, s)| s.output == Some(output)).map(|(id, _)| *id).collect();
                    for id in ids {
                        self.queue_snapshot(id);
                    }
                    return;
                }
                (output.parse_config(default).expect("built-in config"), Some(e), format!("<built-in {default_name}>"))
            }
        };
        log(&format!("loaded {label} for {}", output.name()));
        let generation = previous.map_or(1, |(g, _)| g + 1);
        let widgets = Self::widgets(output, &config);
        self.outputs.insert(output, OutputConfig { generation, source, path_label: label, error, config, widgets });
        // Commands that run once run again for the new config.
        let keys: Vec<ProviderKey> = self.outputs[&output].widgets.iter().filter_map(|w| w.provider.clone()).collect();
        for key in keys {
            if let Some(Provider::Command(c)) = self.providers.get_mut(&key) {
                c.job.next = 0.0;
            }
        }
        let ids: Vec<u64> = self.sessions.iter().filter(|(_, s)| s.output == Some(output)).map(|(id, _)| *id).collect();
        for id in ids {
            self.sessions.get_mut(&id).unwrap().pointers.clear(); // repeats end with the old config
            self.queue_snapshot(id);
        }
    }

    fn widgets(output: Output, config: &Config) -> Vec<WidgetDef> {
        let mut out = vec![];
        for (layer, section, items) in &config.layers {
            let lname = if *layer == Layer::Fn { "fn" } else { "default" };
            for (i, spec) in items.iter().enumerate() {
                let id = text(spec, "id", "");
                let at = match output {
                    Output::Touchbar => format!("{}: layers.{lname}.{section}[{i}]", output.file_name()),
                    Output::Desktop => format!("{}: {section}[{i}]", output.file_name()),
                };
                let Some(kind) = protocol::kind_of(spec) else {
                    log(&format!("{at}: unknown widget {id:?}"));
                    continue;
                };
                let unsupported = output.unsupported(kind, spec);
                if let Some(reason) = unsupported {
                    log(&format!("{at}: {id} is not supported on {} ({reason})", output.name()));
                }
                let provider = if unsupported.is_some() { None } else { Self::provider_key(kind, spec) };
                out.push(WidgetDef { key: format!("{lname}.{section}.{i}"), id, kind, layer: *layer, section,
                                     spec: spec.clone(), unsupported, provider });
            }
        }
        out
    }

    fn provider_key(kind: WidgetKind, spec: &Spec) -> Option<ProviderKey> {
        match kind {
            WidgetKind::Graph => {
                let mut options = Map::new();
                for k in GRAPH_PROVIDER_OPTIONS {
                    if let Some(v) = spec.get(k) {
                        options.insert(k.into(), v.clone());
                    }
                }
                Some(ProviderKey::Graph(format!("{}|{}", text(spec, "id", ""), Value::Object(options))))
            }
            WidgetKind::Command => {
                let exec = text(spec, "exec", "");
                (!exec.is_empty()).then(|| ProviderKey::Command(format!("{}|{}", spec.get("interval").map_or("".into(), Value::to_string), exec)))
            }
            WidgetKind::Agents => Some(ProviderKey::Usage(text(spec, "agent", "claude"))),
            WidgetKind::Media => Some(ProviderKey::Media),
            WidgetKind::Mic => Some(ProviderKey::Mic),
            _ => None,
        }
    }

    // --- demand -------------------------------------------------------------
    /// Outputs with at least one session that said hello.
    fn active_outputs(&self) -> HashSet<Output> {
        self.sessions.values().filter_map(|s| s.output).collect()
    }

    /// Start providers that connected outputs need, stop the rest.
    fn sync_providers(&mut self, t: f64) {
        let active = self.active_outputs();
        self.outputs.retain(|o, _| active.contains(o));
        let mut needed: HashMap<ProviderKey, (WidgetKind, Spec, usize)> = HashMap::new();
        for oc in self.outputs.values() {
            for w in &oc.widgets {
                let Some(key) = &w.provider else { continue };
                let columns = if w.kind == WidgetKind::Graph {
                    let sp = int(&w.spec, "dotSpacing", 4).max(2);
                    (int(&w.spec, "graphWidth", 100) / sp).max(1) as usize
                } else {
                    0
                };
                let entry = needed.entry(key.clone()).or_insert((w.kind, w.spec.clone(), columns));
                entry.2 = entry.2.max(columns);
            }
        }
        self.providers.retain(|key, p| {
            let keep = needed.contains_key(key);
            if !keep {
                p.stop();
            }
            keep
        });
        for (key, (kind, spec, columns)) in needed {
            match self.providers.get_mut(&key) {
                Some(Provider::Graph(g)) if g.columns != columns => g.set_columns(columns),
                Some(_) => {}
                None => {
                    let provider = match kind {
                        WidgetKind::Graph => Provider::Graph(Graph::new(sources::kind_of(&text(&spec, "id", "")).unwrap(), &spec, columns)),
                        WidgetKind::Command => Provider::Command(Command::new(&spec)),
                        WidgetKind::Agents => {
                            let mut usage = Usage::new(&text(&spec, "agent", "claude"));
                            usage.poll(&self.paths.usage_dir); // so the first snapshot has it
                            Provider::Usage(usage)
                        }
                        WidgetKind::Media => Provider::Media(Media::new(crate::config::num(&spec, "interval", 30.0))),
                        WidgetKind::Mic => {
                            let fps = int(&spec, "fps", 20);
                            Provider::Mic(Mic::new(fps), fps)
                        }
                        _ => continue,
                    };
                    // New providers only come with a new session or config, which
                    // brings a snapshot; no update needed.
                    self.providers.insert(key, provider);
                }
            }
        }
        let mic_wanted = self.shown_widgets().any(|w| w.kind == WidgetKind::Mic
            && w.spec.get("waveform").is_none_or(|v| truthy(Some(v))));
        if let Some(Provider::Mic(m, _)) = self.providers.get_mut(&ProviderKey::Mic) {
            m.set_wanted(mic_wanted);
        }
        // Visibility may have changed under pressed pointers.
        self.drop_hidden_pointers();
        let _ = t;
    }

    /// Widgets on the layer each shown session displays.
    fn shown_widgets(&self) -> impl Iterator<Item = &WidgetDef> {
        self.sessions.values().filter(|s| s.shown).filter_map(|s| {
            let oc = self.outputs.get(&s.output?)?;
            Some(oc.widgets.iter().filter(move |w| w.layer == s.layer && w.unsupported.is_none()))
        }).flatten()
    }

    fn drop_hidden_pointers(&mut self) {
        let mut hidden: HashSet<(Output, String)> = HashSet::new();
        for (output, oc) in &self.outputs {
            for w in &oc.widgets {
                if !self.widget_visible(w) {
                    hidden.insert((*output, w.key.clone()));
                }
            }
        }
        for s in self.sessions.values_mut() {
            if let Some(output) = s.output {
                s.pointers.retain(|_, p| !hidden.contains(&(output, p.widget.clone())));
            }
        }
    }

    // --- running ------------------------------------------------------------
    /// Do everything that's due at time `t`.
    pub fn tick(&mut self, t: f64) {
        self.children.retain_mut(|p| matches!(p.try_wait(), Ok(None)));
        if t >= self.next_poll {
            self.next_poll = t + POLL_SECONDS;
            for output in self.active_outputs() {
                self.load_config(output);
            }
            self.sync_providers(t);
            let dir = self.paths.usage_dir.clone();
            for (key, p) in &mut self.providers {
                if let Provider::Usage(u) = p {
                    u.refresh(t);
                    if u.poll(&dir) {
                        self.changed.insert(key.clone());
                    }
                }
            }
        }
        let media_shown = self.shown_widgets().any(|w| w.kind == WidgetKind::Media);
        for (key, p) in &mut self.providers {
            let changed = match p {
                Provider::Graph(g) => g.tick(t),
                Provider::Command(c) => {
                    let what = match key { ProviderKey::Command(k) => k.clone(), _ => String::new() };
                    c.job.tick(t, &what).is_some_and(|out| c.finished(&out))
                }
                Provider::Media(m) => m.tick(t, media_shown),
                Provider::Mic(m, _) => {
                    m.tick(t);
                    std::mem::take(&mut m.changed)
                }
                Provider::Usage(_) => false,
            };
            if changed {
                self.changed.insert(key.clone());
            }
        }
        self.run_repeats(t);
        self.publish();
    }

    fn run_repeats(&mut self, t: f64) {
        let mut due = vec![];
        for s in self.sessions.values_mut() {
            let Some(oc) = s.output.and_then(|o| self.outputs.get(&o)) else { continue };
            for p in s.pointers.values_mut() {
                if let Some(next) = p.next.filter(|n| t >= *n) {
                    p.next = Some((next + oc.config.repeat_interval).max(t));
                    if let Some(cmd) = oc.widget(&p.widget).and_then(|w| w.command(None)) {
                        due.push(cmd);
                    }
                }
            }
        }
        for cmd in due {
            self.spawn(&cmd);
        }
    }

    /// Mark widgets of changed providers dirty in every session that has them.
    fn publish(&mut self) {
        if self.changed.is_empty() {
            return;
        }
        let changed = std::mem::take(&mut self.changed);
        let visibility = changed.iter().any(|k| matches!(k, ProviderKey::Usage(_)));
        for s in self.sessions.values_mut() {
            let Some(oc) = s.output.and_then(|o| self.outputs.get(&o)) else { continue };
            for w in &oc.widgets {
                if w.provider.as_ref().is_some_and(|k| changed.contains(k)) {
                    s.dirty.insert(w.key.clone());
                }
            }
        }
        if visibility {
            self.drop_hidden_pointers();
        }
    }

    /// When `tick` next has something to do.
    pub fn deadline(&self) -> f64 {
        let media_shown = self.shown_widgets().any(|w| w.kind == WidgetKind::Media);
        let mut d = self.next_poll;
        for p in self.providers.values() {
            d = d.min(match p {
                Provider::Graph(g) => g.next,
                Provider::Command(c) => c.job.deadline(),
                Provider::Media(m) => m.deadline(media_shown),
                Provider::Mic(m, _) => m.deadline(),
                Provider::Usage(_) => f64::INFINITY,
            });
        }
        for s in self.sessions.values() {
            for p in s.pointers.values() {
                d = d.min(p.next.unwrap_or(f64::INFINITY));
            }
        }
        d
    }

    /// File descriptors to poll for reading.
    pub fn fds(&self) -> Vec<(RawFd, Token)> {
        let mut out = vec![];
        for (key, p) in &self.providers {
            match (key, p) {
                (ProviderKey::Command(k), Provider::Command(c)) => {
                    out.extend(c.job.fd().map(|fd| (fd, Token::Command(k.clone()))));
                }
                (_, Provider::Media(m)) => {
                    out.extend(m.status.fd().map(|fd| (fd, Token::MediaStatus)));
                    out.extend(m.watch_fd().map(|fd| (fd, Token::MediaWatch)));
                }
                (_, Provider::Mic(m, _)) => out.extend(m.fds().into_iter().map(|(fd, w)| (fd, Token::Mic(w)))),
                _ => {}
            }
        }
        out
    }

    pub fn readable(&mut self, token: Token, t: f64) {
        match token {
            Token::Command(k) => {
                let key = ProviderKey::Command(k);
                if let Some(Provider::Command(c)) = self.providers.get_mut(&key)
                    && let Some(out) = c.job.readable()
                    && c.finished(&out) {
                    self.changed.insert(key);
                }
            }
            Token::MediaStatus => {
                if let Some(Provider::Media(m)) = self.providers.get_mut(&ProviderKey::Media)
                    && let Some(out) = m.status.readable()
                    && m.finished(&out) {
                    self.changed.insert(ProviderKey::Media);
                }
            }
            Token::MediaWatch => {
                if let Some(Provider::Media(m)) = self.providers.get_mut(&ProviderKey::Media) {
                    m.watch_readable(t);
                }
            }
            Token::Mic(which) => {
                if let Some(Provider::Mic(m, _)) = self.providers.get_mut(&ProviderKey::Mic) {
                    m.readable(which);
                    if std::mem::take(&mut m.changed) {
                        self.changed.insert(ProviderKey::Mic);
                    }
                }
            }
        }
        self.publish();
    }

    /// Run in this process for at most `max_wait` seconds: tick, then wait
    /// for provider output (the in-process preview has no server loop).
    pub fn pump(&mut self, max_wait: f64) {
        let t = crate::now();
        self.tick(t);
        let fds = self.fds();
        let mut pfds: Vec<libc::pollfd> =
            fds.iter().map(|(fd, _)| libc::pollfd { fd: *fd, events: libc::POLLIN, revents: 0 }).collect();
        let wait = (self.deadline().min(t + max_wait) - crate::now()).max(0.0);
        let ms = (wait * 1000.0).ceil() as i32;
        if unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, ms) } <= 0 {
            return;
        }
        for (pfd, (_, token)) in pfds.iter().zip(fds) {
            if pfd.revents != 0 {
                self.readable(token, crate::now());
            }
        }
    }

    /// Whether a command or media script is still running.
    pub fn busy(&self) -> bool {
        self.providers.values().any(|p| match p {
            Provider::Command(c) => c.job.running(),
            Provider::Media(m) => m.status.running(),
            _ => false,
        })
    }

    /// Replace provider data with the fixed data of the golden previews
    /// (tests/golden/fixture.json).
    #[cfg(test)]
    pub fn inject(&mut self, data: &Value) {
        let floats = |v: &Value| -> Vec<f64> { v.as_array().map_or(vec![], |a| a.iter().map(|x| x.as_f64().unwrap()).collect()) };
        let line = |l: &Value| (l[0].as_str().unwrap().to_string(), l[1].as_bool().unwrap());
        for oc in self.outputs.values() {
            for w in &oc.widgets {
                let Some(p) = w.provider.as_ref().and_then(|k| self.providers.get_mut(k)) else { continue };
                match p {
                    Provider::Graph(g) => {
                        let d = &data["graphs"][&w.id];
                        let history = d["history"].as_array().unwrap().iter().map(floats).collect();
                        let mut lines: Vec<(String, bool)> = d["lines"].as_array().unwrap().iter().map(line).collect();
                        if truthy(w.spec.get("temperature")) || text(&w.spec, "detail", "none") != "none" {
                            lines.push(line(&d["detail"]));
                        }
                        let battery = d.get("battery").map(|b| (b["charge"].as_i64().unwrap(), text(b.as_object().unwrap(), "status", "")));
                        g.fixture(history, lines, floats(&d["cores"]), battery);
                    }
                    Provider::Command(c) => {
                        let d = &data["commands"][&w.id];
                        c.fixture(d["text"].as_str().unwrap(), d["urgent"].as_bool().unwrap_or(false));
                    }
                    Provider::Usage(u) => u.fixture(data["usage"].get(text(&w.spec, "agent", "claude")).cloned()),
                    Provider::Media(m) => m.fixture(data["media"].clone()),
                    Provider::Mic(m, _) => {
                        let d = &data["mic"];
                        m.muted = d["muted"].as_bool();
                        m.in_use = d["inUse"].as_bool().unwrap();
                        m.levels = floats(&d["levels"]).into();
                    }
                }
            }
        }
    }

    /// Stop every provider (on exit).
    pub fn shutdown(&mut self) {
        for p in self.providers.values_mut() {
            p.stop();
        }
        self.providers.clear();
    }
}
