//! The Touch Bar client: draws the backend's widget state
//! (docs/backend-protocol.md) on the bar through t1bridge and turns touches
//! into backend requests. Layout, drawing, press feedback, the Fn layer and
//! contact latch, idle dimming and key taps (Esc, F-keys) stay here.

use std::collections::{HashMap, HashSet, VecDeque};
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::fs;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use cairo::{Context, FontSlant, FontWeight, Format, ImageSurface, LinearGradient, Matrix, TextExtents};
use chrono::format::{Item, StrftimeItems};
use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, TimeZone, Utc};
use serde_json::{Value, json};

use crate::backend::{Backend, Paths};
use crate::config::{Config, DEFAULT_CONFIG, Layer, Rgb, SECTIONS, Spec, hex_rgb, home, int, lerp_rgb, num, rgb, text, truthy};
use crate::proto::{self, Buffer, Conn, Packer, le_u32, le_u64};
use crate::protocol::{self, MAX_LINE, WidgetKind, error};
use crate::sources::{self, Source};
use crate::{log, now};

const POLL_SECONDS: f64 = 1.0; // how often to check the btop theme and the minute
const RETRY_MIN: f64 = 0.1; // reconnect delay after losing the backend or t1bridge, doubling ...
const RETRY_MAX: f64 = 2.0; // ... up to this
const OFFLINE_AFTER: f64 = 1.0; // show the disconnected state if no snapshot arrives this soon
const CLIENT: &str = concat!("omarchy-glance ", env!("CARGO_PKG_VERSION"));

// --- hardware facts and widget metrics ---------------------------------------
// The panel reports 2170 px along the bar, but only the first 2060 light up
// (same with the stock renderer; measured 2026-10-04). Lay out within this.
const VISIBLE_WIDTH: i32 = 2060;

const KEY_WIDTH: i64 = 140; // default width of Esc and buttons
const KEY_PAD: i32 = 4; // inset of each key face inside its slot
const DIM: f64 = 0.25; // brightness multiplier while idle
const USAGE_ICON: &str = "\u{F16A3}"; // the Omarchy bar's agents glyph (Nerd Font)
const METER_ALARM: f64 = 0.9; // turn red at this fraction used, like the bar panel
const SHORT_LABELS: [(&str, &str); 3] = [("Session", "5h"), ("5h window", "5h"), ("Weekly", "7d")]; // agents widget: stacked layout, narrow row meters
const STACKED_FONT: f64 = 14.0;
const RESET_COLOR: Rgb = [160.0, 160.0, 160.0]; // agents widget: when each limit resets
const ICON_MIC: &str = "\u{F036C}";
const ICON_MIC_MUTED: &str = "\u{F036D}";
const ICON_PREVIOUS: &str = "\u{F04AE}";
const ICON_PLAY: &str = "\u{F040A}";
const ICON_PAUSE: &str = "\u{F03E4}";
const ICON_NEXT: &str = "\u{F04AD}";
const BTOP_THEME: &str = ".config/btop/themes/current.theme";
const OFFLINE_TEXT: &str = "omarchy-glance: backend unavailable, reconnecting…";

// 5x7 glyphs for the Esc label
const GLYPHS: [(char, [&str; 7]); 3] = [
    ('e', ["     ", "     ", " ### ", "#   #", "#####", "#    ", " ### "]),
    ('s', ["     ", "     ", " ####", "#    ", " ### ", "    #", "#### "]),
    ('c', ["     ", "     ", " ### ", "#    ", "#    ", "#   #", " ### "]),
];

fn mtime(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn cpu_count() -> usize {
    (unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) }).max(1) as usize
}

/// Round half to even, as the original renderer did; the layouts depend on it.
fn round_even(v: f64) -> i64 {
    v.round_ties_even() as i64
}

/// strftime that falls back to the format itself if it's invalid.
fn strftime<Tz: TimeZone>(dt: &DateTime<Tz>, fmt: &str) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let items: Vec<Item> = StrftimeItems::new(fmt).collect();
    if items.iter().any(|i| matches!(i, Item::Error)) {
        return fmt.to_string();
    }
    dt.format_with_items(items.into_iter()).to_string()
}

fn parse_time(s: &str) -> Option<DateTime<FixedOffset>> {
    DateTime::parse_from_rfc3339(s).ok().or_else(|| {
        ["%Y-%m-%dT%H:%M:%S%.f", "%Y-%m-%d %H:%M:%S%.f"]
            .iter()
            .find_map(|f| NaiveDateTime::parse_from_str(s, f).ok())
            .map(|n| n.and_utc().fixed_offset())
    })
}

/// theme[name]="#rrggbb" entries from btop's current (Omarchy) theme.
fn read_btop_theme() -> HashMap<String, String> {
    let mut theme = HashMap::new();
    for line in fs::read_to_string(home().join(BTOP_THEME)).unwrap_or_default().lines() {
        if let Some((name, value)) = line.strip_prefix("theme[").and_then(|l| l.split_once("]=")) {
            theme.insert(name.to_string(), value.trim().trim_matches('"').to_string());
        }
    }
    theme
}

/// Presentation settings from a snapshot, over the built-in defaults.
fn settings_config(settings: &Value) -> Config {
    let mut c = Config::parse(DEFAULT_CONFIG).expect("built-in config");
    c.layers.clear();
    let empty = Spec::new();
    let colors = settings.get("colors").and_then(Value::as_object).unwrap_or(&empty);
    let color = |k: &str, fallback: Rgb| hex_rgb(colors.get(k), fallback);
    (c.background, c.key, c.key_pressed) = (color("background", c.background), color("key", c.key), color("keyPressed", c.key_pressed));
    (c.text, c.urgent) = (color("text", c.text), color("urgent", c.urgent));
    (c.debug_bg, c.debug_bg_fn) = (color("debugBackground", c.debug_bg), color("debugBackgroundFn", c.debug_bg_fn));
    let s = settings.as_object().unwrap_or(&empty);
    c.font = text(s, "font", &c.font);
    c.idle_dim = num(s, "idleDimSeconds", c.idle_dim);
    c.repeat_delay = num(s, "repeatDelay", c.repeat_delay);
    c.repeat_interval = num(s, "repeatInterval", c.repeat_interval);
    c.has_fn = truthy(s.get("hasFn"));
    let debug = s.get("debug").and_then(Value::as_object).unwrap_or(&empty);
    (c.debug_background, c.border, c.test_pattern) =
        (truthy(debug.get("background")), truthy(debug.get("border")), truthy(debug.get("testPattern")));
    c
}

/// What touching a widget does.
#[derive(Clone, PartialEq, Debug)]
enum Action {
    None,
    Press,                                // a backend action (the backend repeats it)
    Key { name: String, repeat: bool },  // a key t1bridge taps (repeated here)
    Zones(Vec<String>),                  // media: a backend action per zone
}

fn parse_action(v: &Value) -> Action {
    if let Some(name) = v.get("key").and_then(Value::as_str) {
        return Action::Key { name: name.into(), repeat: truthy(v.get("repeat")) };
    }
    if let Some(zones) = v.get("zones").and_then(Value::as_array) {
        return Action::Zones(zones.iter().filter_map(Value::as_str).map(String::from).collect());
    }
    if truthy(v.get("press")) { Action::Press } else { Action::None }
}

#[derive(Clone, PartialEq)]
struct Limit {
    label: String,
    frac: Option<f64>,
    resets: Option<DateTime<FixedOffset>>,
}

/// A widget's state from the backend. Graph text, cores and battery go into
/// the widget's `Source` instead, where its label and value helpers read them.
enum Data {
    None,
    Command { text: String, urgent: bool },
    Agents { visible: bool, limits: Vec<Limit> },
    Graph { series: Vec<Vec<f64>>, scale: Vec<f64> },
    Mic { muted: Option<bool>, in_use: bool, levels: Vec<f64> },
    Media(Value),
}

fn floats(v: Option<&Value>) -> Vec<f64> {
    v.and_then(Value::as_array).map_or(vec![], |a| a.iter().filter_map(Value::as_f64).collect())
}

/// How a dot graph is drawn, from a widget's options.
struct DotStyle {
    sp: i32,
    radius: f64,
    bars: bool,
    grid: bool,
}

#[derive(Clone, Copy)]
enum Part {
    Label,
    Graph,
    Cores,
    Value,
}

/// One placed widget from the snapshot.
struct Widget {
    key: String, // "<layer>.<section>.<index>"
    layer: Layer,
    section: String,
    kind: WidgetKind,
    spec: Spec, // the widget's options
    action: Action,
    rect: [i32; 4], // x0, y0, x1, y1
    source: Option<Source>, // graphs
    data: Data,
}

impl Widget {
    fn hit(&self, x: i32, y: i32) -> bool {
        let [x0, y0, x1, y1] = self.rect;
        x0 <= x && x < x1 && y0 <= y && y < y1
    }

    fn visible(&self) -> bool {
        match &self.data {
            Data::Agents { visible, .. } => *visible,
            _ => self.kind != WidgetKind::Agents,
        }
    }

    /// Take a state object from a snapshot or update.
    fn set_state(&mut self, state: &Value) {
        let s = state.as_object();
        self.data = match self.kind {
            WidgetKind::Command => Data::Command {
                text: state.get("text").and_then(Value::as_str).unwrap_or("").into(),
                urgent: truthy(state.get("urgent")),
            },
            WidgetKind::Agents => Data::Agents {
                visible: truthy(state.get("visible")),
                limits: state.get("limits").and_then(Value::as_array).into_iter().flatten().map(|l| Limit {
                    label: l.get("label").and_then(Value::as_str).unwrap_or("Limit").into(),
                    frac: l.get("fraction").and_then(Value::as_f64),
                    resets: l.get("resetsAt").and_then(Value::as_str).and_then(parse_time),
                }).collect(),
            },
            WidgetKind::Graph => {
                if let Some(source) = self.source.as_mut() {
                    source.lines = state.get("lines").and_then(Value::as_array).into_iter().flatten().filter_map(|l| {
                        Some((l.get(0)?.as_str()?.to_string(), truthy(l.get(1))))
                    }).collect();
                    source.cores = floats(state.get("cores"));
                    source.battery = state.get("battery").and_then(|b| {
                        Some((b.get("charge")?.as_i64()?, b.get("status")?.as_str()?.to_string()))
                    });
                }
                let series = state.get("series").and_then(Value::as_array).map_or(vec![], |a| a.iter().map(|s| floats(Some(s))).collect());
                Data::Graph { series, scale: floats(state.get("scale")) }
            }
            WidgetKind::Mic => Data::Mic {
                muted: state.get("muted").and_then(Value::as_bool),
                in_use: truthy(state.get("inUse")),
                levels: floats(state.get("levels")),
            },
            WidgetKind::Media => Data::Media(if s.is_some() { state.clone() } else { json!({}) }),
            _ => Data::None,
        };
    }
}

/// The connection to the backend: its socket, or a backend in this process
/// (preview and tests) that gets the same lines.
pub enum Link {
    Socket { stream: UnixStream, rbuf: Vec<u8>, wbuf: Vec<u8> },
    Local { backend: Box<Backend>, session: u64 },
}

impl Link {
    fn connect(path: &Path) -> io::Result<Link> {
        let stream = UnixStream::connect(path)?;
        stream.set_nonblocking(true)?;
        Ok(Link::Socket { stream, rbuf: vec![], wbuf: vec![] })
    }

    /// Queue one message line.
    fn send(&mut self, line: &str) -> Result<(), String> {
        match self {
            Link::Socket { wbuf, .. } => {
                wbuf.extend_from_slice(line.as_bytes());
                wbuf.push(b'\n');
                self.flush()
            }
            Link::Local { backend, session } => {
                backend.handle_line(*session, line, now());
                Ok(())
            }
        }
    }

    /// Write queued lines as far as the socket takes them.
    fn flush(&mut self) -> Result<(), String> {
        let Link::Socket { stream, wbuf, .. } = self else { return Ok(()) };
        while !wbuf.is_empty() {
            match stream.write(wbuf) {
                Ok(0) => return Err("backend closed the connection".into()),
                Ok(n) => {
                    wbuf.drain(..n);
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        Ok(())
    }

    /// Complete lines received so far.
    fn receive(&mut self) -> Result<Vec<String>, String> {
        let (stream, rbuf) = match self {
            Link::Local { backend, session } => return Ok(backend.drain(*session, now())),
            Link::Socket { stream, rbuf, .. } => (stream, rbuf),
        };
        let mut buf = [0u8; 65536];
        let mut eof = false;
        loop {
            match stream.read(&mut buf) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => rbuf.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let mut lines = vec![];
        while let Some(end) = rbuf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = rbuf.drain(..=end).collect();
            lines.push(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned());
        }
        if rbuf.len() >= MAX_LINE {
            return Err("backend sent a line over 1 MiB".into());
        }
        if eof && lines.is_empty() {
            return Err("backend closed the connection".into());
        }
        Ok(lines) // an EOF after lines shows up on the next read
    }

    fn fd(&self) -> Option<RawFd> {
        match self {
            Link::Socket { stream, .. } => Some(stream.as_raw_fd()),
            Link::Local { .. } => None,
        }
    }

    fn wants_write(&self) -> bool {
        matches!(self, Link::Socket { wbuf, .. } if !wbuf.is_empty())
    }
}

pub struct Renderer {
    conn: Option<Conn>, // t1bridge; None in preview mode and while reconnecting
    buffers: HashMap<u32, Buffer>,
    free: VecDeque<u32>, // buffer ids we may draw into
    w: i32,
    h: i32,
    portrait: bool,
    long: i32,
    short: i32,
    visible: i32, // usable length for layout
    dirty: bool,
    damage: HashSet<String>, // widget keys to redraw over the last frame
    last_frame: Option<u32>, // buffer id of the last submitted frame
    frame_id: u64,
    widgets: Vec<Widget>,
    owner: HashMap<u8, String>, // contact id -> key of the widget it first touched
    touch_x: HashMap<u8, i32>,  // contact id -> x where it first touched
    pointers: HashSet<u8>,      // contacts with a press the backend knows about
    ignored: HashSet<u8>,       // contacts that were down when the widgets changed under them
    down: HashSet<u8>,          // contacts touching now
    minute: Option<u64>,        // wall-clock minute, for reset times
    repeat_at: HashMap<u8, f64>, // contact id -> time of the next key repeat
    last_input: f64,
    dimmed: bool,
    fn_held: bool,
    logged_contacts: HashSet<u8>,
    touch_marks: Vec<i32>, // logical x of current touches (test pattern)
    provider_icons: HashMap<String, ImageSurface>,
    theme: HashMap<String, String>, // btop theme colours (default graph gradients)
    theme_mtime: Option<Option<SystemTime>>,
    config: Config, // presentation settings from the last snapshot
    link: Option<Link>,
    next_id: i64,
    rev: u64,
    generation: u64,
    synced: bool,    // drawing from a snapshot
    resyncing: bool, // missed an update; waiting for the snapshot
    view_sent: Option<Layer>,
    offline: bool,   // showing the disconnected state
    offline_at: f64, // when to show it if no snapshot has come
    backend_retry: (f64, f64), // (next attempt, delay after that)
    hw_retry: (f64, f64),
    measure: Context, // scratch context for measuring text outside a frame
    clock: Option<DateTime<FixedOffset>>, // fixed time and zone instead of the system's (golden tests)
}

impl Renderer {
    pub fn new(link: Option<Link>) -> Renderer {
        let surface = ImageSurface::create(Format::Rgb24, 1, 1).expect("cairo surface");
        Renderer {
            conn: None,
            buffers: HashMap::new(),
            free: VecDeque::new(),
            w: 0,
            h: 0,
            portrait: false,
            long: 0,
            short: 0,
            visible: 0,
            dirty: false,
            damage: HashSet::new(),
            last_frame: None,
            frame_id: 0,
            widgets: vec![],
            owner: HashMap::new(),
            touch_x: HashMap::new(),
            pointers: HashSet::new(),
            ignored: HashSet::new(),
            down: HashSet::new(),
            minute: None,
            repeat_at: HashMap::new(),
            last_input: now(),
            dimmed: false,
            fn_held: false,
            logged_contacts: HashSet::new(),
            touch_marks: vec![],
            provider_icons: [
                ("claude", include_bytes!("../assets/agents/claude.png").as_slice()),
                ("codex", include_bytes!("../assets/agents/codex.png").as_slice()),
            ].into_iter().filter_map(|(name, bytes)| {
                ImageSurface::create_from_png(&mut std::io::Cursor::new(bytes)).ok()
                    .map(|surface| (name.to_string(), surface))
            }).collect(),
            theme: HashMap::new(),
            theme_mtime: None,
            config: settings_config(&json!({"hasFn": false})),
            link,
            next_id: 1,
            rev: 0,
            generation: 0,
            synced: false,
            resyncing: false,
            view_sent: None,
            offline: false,
            offline_at: now() + OFFLINE_AFTER,
            backend_retry: (0.0, RETRY_MIN),
            hw_retry: (0.0, RETRY_MIN),
            measure: Context::new(&surface).expect("cairo context"),
            clock: None,
        }
    }

    // --- t1bridge -----------------------------------------------------------
    fn handshake(&mut self) -> Result<(), String> {
        let features = proto::FEAT_MEMFD | proto::FEAT_INPUT | proto::FEAT_KEYS;
        let hello = Packer::default().u16(0).u16(0).u64(features).0;
        let conn = self.conn.as_mut().ok_or("not connected")?;
        conn.send(proto::HELLO, &hello, None, "hello").map_err(|e| e.to_string())?;
        let (mtype, req, payload) = conn.recv().map_err(|e| e.to_string())?;
        conn.pending.remove(&req);
        if mtype == proto::ERROR || mtype != proto::HELLO_ACK || payload.len() < 20 {
            let code = if payload.len() >= 4 { le_u32(&payload, 0) } else { 0 };
            return Err(format!("hello rejected: {}", proto::error_name(code)));
        }
        let minor = proto::le_u16(&payload, 0);
        let (w, h, fmt, max_buffers) =
            (le_u32(&payload, 4), le_u32(&payload, 8), le_u32(&payload, 12), le_u32(&payload, 16));
        log(&format!("connected: minor={minor} {w}x{h} fmt={fmt} max_buffers={max_buffers}"));
        self.set_geometry(w as i32, h as i32);
        let stride = self.w * 4;
        if Format::Rgb24.stride_for_width(w).ok() != Some(stride) {
            return Err(format!("cairo can't draw into a {w} px wide XRGB8888 buffer"));
        }
        for id in 1..=max_buffers.min(2) {
            let buf = Buffer::new(id, self.w, self.h, stride).map_err(|e| format!("buffer {id}: {e}"))?;
            let payload = Packer::default().u32(id).u32(stride as u32).u64(buf.len as u64).0;
            let conn = self.conn.as_mut().unwrap();
            conn.send(proto::REGISTER_BUFFER, &payload, Some(buf.fd.as_raw_fd()), &format!("register buffer {id}"))
                .map_err(|e| e.to_string())?;
            self.buffers.insert(id, buf);
            self.free.push_back(id);
        }
        Ok(())
    }

    fn connect_hw(&mut self, t: f64) {
        let result = Conn::connect(proto::SOCK_PATH).map_err(|e| format!("{}: {e}", proto::SOCK_PATH)).and_then(|conn| {
            self.conn = Some(conn);
            self.handshake()
        });
        match result {
            Ok(()) => {
                self.hw_retry = (0.0, RETRY_MIN);
                self.relayout(); // the geometry may be new
            }
            Err(e) => self.lost_hw(t, &e),
        }
    }

    /// Drop the t1bridge connection and try again soon. Touches end with it.
    fn lost_hw(&mut self, t: f64, why: &str) {
        if self.hw_retry.1 == RETRY_MIN {
            log(&format!("t1bridge: {why}; reconnecting"));
        }
        self.conn = None;
        self.buffers.clear();
        self.free.clear();
        self.last_frame = None;
        self.fn_held = false;
        // The backend outlives this, so it must hear the lifts it will never see.
        let held: Vec<u8> = self.pointers.drain().collect();
        for cid in held {
            self.request(json!({"type": "release", "pointer": cid}));
        }
        self.end_touches();
        let (_, delay) = self.hw_retry;
        self.hw_retry = (t + delay, (delay * 2.0).min(RETRY_MAX));
    }

    fn set_geometry(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
        self.portrait = h > w;
        self.long = w.max(h);
        self.short = w.min(h);
        self.visible = self.long.min(VISIBLE_WIDTH);
    }

    // --- backend ------------------------------------------------------------
    fn connect_backend(&mut self, t: f64) {
        let path = crate::backend::server::socket_path();
        match Link::connect(&path) {
            Ok(link) => {
                self.link = Some(link);
                self.offline_at = self.offline_at.min(t + OFFLINE_AFTER);
                self.request(json!({"type": "hello", "protocol": protocol::PROTOCOL, "output": "touchbar", "client": CLIENT}));
            }
            Err(e) => self.lost_backend(t, &format!("{}: {e}", path.display())),
        }
    }

    /// Show the disconnected state and reconnect soon; the bar stays ours.
    fn lost_backend(&mut self, t: f64, why: &str) {
        if self.backend_retry.1 == RETRY_MIN {
            log(&format!("backend: {why}; reconnecting"));
        }
        self.link = None;
        self.synced = false;
        self.resyncing = false;
        self.view_sent = None;
        self.widgets.clear();
        self.end_touches();
        if !self.offline {
            self.offline = true;
            self.dirty = true;
        }
        let (_, delay) = self.backend_retry;
        self.backend_retry = (t + delay, (delay * 2.0).min(RETRY_MAX));
    }

    /// Forget presses; contacts still down are ignored until they lift.
    fn end_touches(&mut self) {
        self.ignored.extend(self.down.iter().copied());
        self.owner.clear();
        self.touch_x.clear();
        self.repeat_at.clear();
        self.pointers.clear();
        self.dirty = true;
    }

    /// Send a request; the backend answers with an ack or error we only log.
    fn request(&mut self, mut msg: Value) {
        let Some(link) = self.link.as_mut() else { return };
        msg["id"] = self.next_id.into();
        self.next_id += 1;
        if let Err(e) = link.send(&msg.to_string()) {
            self.lost_backend(now(), &e);
        }
    }

    /// Handle everything the backend has sent.
    pub fn pull(&mut self) {
        let Some(link) = self.link.as_mut() else { return };
        match link.receive() {
            Ok(lines) => {
                for line in lines {
                    match serde_json::from_str::<Value>(&line) {
                        Ok(msg) => self.on_message(&msg),
                        Err(e) => log(&format!("backend sent invalid JSON: {e}")),
                    }
                }
            }
            Err(e) => self.lost_backend(now(), &e),
        }
    }

    fn on_message(&mut self, msg: &Value) {
        match msg.get("type").and_then(Value::as_str) {
            Some("snapshot") => self.on_snapshot(msg),
            Some("update") => self.on_update(msg),
            Some("welcome") => log(&format!("backend session {}", msg["session"].as_str().unwrap_or("?"))),
            Some("error") => {
                let code = msg["code"].as_str().unwrap_or("");
                // A release after the backend dropped the press itself (hidden widget, reload).
                if code != error::UNKNOWN_POINTER {
                    log(&format!("backend: {code}: {}", msg["message"].as_str().unwrap_or("")));
                }
            }
            _ => {}
        }
    }

    fn on_snapshot(&mut self, msg: &Value) {
        let generation = msg["config"]["generation"].as_u64().unwrap_or(0);
        if generation != self.generation || !self.synced {
            self.end_touches(); // the backend ended these presses with the old config
        }
        self.config = settings_config(&msg["settings"]);
        let mut widgets = vec![];
        for v in msg["widgets"].as_array().into_iter().flatten() {
            let Some(kind) = v["kind"].as_str().and_then(WidgetKind::parse) else { continue };
            if v["supported"] == false {
                continue;
            }
            let spec = v["options"].as_object().cloned().unwrap_or_default();
            let id = text(&spec, "id", "");
            let mut w = Widget {
                key: v["key"].as_str().unwrap_or("").into(),
                layer: if v["layer"] == "fn" { Layer::Fn } else { Layer::Default },
                section: v["section"].as_str().unwrap_or("").into(),
                kind,
                action: parse_action(&v["action"]),
                rect: [0; 4],
                source: sources::kind_of(&id).filter(|_| kind == WidgetKind::Graph).map(|k| Source::new(k, &spec)),
                data: Data::None,
                spec,
            };
            w.set_state(&v["state"]);
            widgets.push(w);
        }
        self.widgets = widgets;
        (self.generation, self.rev) = (generation, msg["rev"].as_u64().unwrap_or(0));
        (self.synced, self.resyncing, self.offline) = (true, false, false);
        self.backend_retry = (0.0, RETRY_MIN);
        self.relayout();
        self.sync_view();
    }

    fn on_update(&mut self, msg: &Value) {
        if !self.synced || self.resyncing {
            return;
        }
        let rev = msg["rev"].as_u64().unwrap_or(0);
        if rev != self.rev + 1 {
            log(&format!("missed backend updates (rev {rev} after {}); resyncing", self.rev));
            self.resyncing = true;
            self.request(json!({"type": "resync"}));
            return;
        }
        self.rev = rev;
        let mut moved = false;
        for (key, state) in msg["widgets"].as_object().into_iter().flatten() {
            let Some(i) = self.widgets.iter().position(|w| w.key == *key) else { continue };
            let before = self.width_of(&self.widgets[i]);
            self.widgets[i].set_state(state);
            moved |= self.width_of(&self.widgets[i]) != before;
            self.damage.insert(key.clone());
        }
        if moved {
            self.relayout(); // a command's text, the waveform or an agents widget changed size
        }
    }

    /// Tell the backend which layer is on screen (it runs media and mic for that).
    fn sync_view(&mut self) {
        let layer = self.visible_layer();
        if self.synced && self.view_sent != Some(layer) {
            self.view_sent = Some(layer);
            let name = if layer == Layer::Fn { "fn" } else { "default" };
            self.request(json!({"type": "view", "layer": name, "shown": true}));
        }
    }

    // --- layout -------------------------------------------------------------
    /// Place the widgets of both layers (fingers stay attached by key).
    fn relayout(&mut self) {
        for layer in [Layer::Default, Layer::Fn] {
            for section in SECTIONS {
                let idx: Vec<usize> =
                    (0..self.widgets.len()).filter(|&i| self.widgets[i].layer == layer && self.widgets[i].section == section).collect();
                let widths: Vec<i32> = idx.iter().map(|&i| self.width_of(&self.widgets[i])).collect();
                let total: i32 = widths.iter().sum();
                let mut x = match section {
                    "left" => 0,
                    "center" => (self.visible - total).div_euclid(2),
                    _ => self.visible - total,
                };
                for (&i, width) in idx.iter().zip(widths) {
                    self.widgets[i].rect = [x, 0, x + width, self.short];
                    x += width;
                }
            }
        }
        let hidden: HashSet<String> = self.widgets.iter().filter(|w| !w.visible()).map(|w| w.key.clone()).collect();
        self.owner.retain(|_, key| !hidden.contains(key));
        self.repeat_at.retain(|cid, _| self.owner.contains_key(cid));
        self.touch_x.retain(|cid, _| self.owner.contains_key(cid));
        self.pointers.retain(|cid| self.owner.contains_key(cid)); // the backend drops those presses too
        self.dirty = true;
    }

    fn text_ext(&self, text: &str, size: f64) -> TextExtents {
        self.measure.select_font_face(&self.config.font, FontSlant::Normal, FontWeight::Normal);
        self.measure.set_font_size(size);
        extents(&self.measure, text)
    }

    fn width_of(&self, w: &Widget) -> i32 {
        if !w.visible() { return 0; }
        let s = &w.spec;
        match w.kind {
            WidgetKind::Spacer => int(s, "size", 40) as i32,
            WidgetKind::Agents if text(s, "layout", "") == "stacked" => {
                let (label_w, pct_w, reset_w) = self.stacked_columns(w);
                let reset_w = if reset_w != 0 { reset_w + 12 } else { 0 };
                14 + 30 + 14 + label_w + 10 + meter_width(s) + 10 + pct_w + reset_w + 16 + 2 * KEY_PAD
            }
            WidgetKind::Agents => {
                let n = self.limits_for(w).len() as i32;
                14 + 30 + 14 + n * meter_width(s) + (n - 1) * 24 + 16 + 2 * KEY_PAD
            }
            WidgetKind::Graph => self.graph_parts(w).iter().map(|(_, width)| width).sum::<i32>() + 2 * 14 + 2 * KEY_PAD,
            WidgetKind::Mic if self.mic_recording(w) => self.mic_layout(w).2,
            WidgetKind::Media => 3 * int(s, "buttonWidth", 100) as i32 + int(s, "titleWidth", 360) as i32,
            WidgetKind::Command if !truthy(s.get("width")) => {
                let t = match &w.data { Data::Command { text, .. } => text.as_str(), _ => "" };
                let adv = self.text_ext(t, num(s, "fontSize", 18.0)).x_advance() as i32;
                (KEY_WIDTH as i32 / 2).max(adv + 32 + 2 * KEY_PAD)
            }
            _ => int(s, "width", KEY_WIDTH) as i32,
        }
    }

    fn widget(&self, key: &str) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.key == key)
    }

    // --- state --------------------------------------------------------------
    /// The Fn layer stays up while a finger is still on one of its widgets.
    fn fn_layer(&self) -> bool {
        self.config.has_fn
            && (self.fn_held || self.owner.values().any(|k| self.widget(k).is_some_and(|w| w.layer == Layer::Fn)))
    }

    fn visible_layer(&self) -> Layer {
        if self.fn_layer() { Layer::Fn } else { Layer::Default }
    }

    fn pressed(&self, w: &Widget) -> bool {
        self.owner.values().any(|k| *k == w.key)
    }

    // --- actions ------------------------------------------------------------
    /// A contact touched a widget: tap its key here, or ask the backend.
    fn press(&mut self, cid: u8, key: &str, lx: i32, t: f64) {
        let Some(w) = self.widget(key) else { return };
        let zone = match &w.action {
            Action::None => return,
            Action::Key { name, repeat } => {
                let (name, repeat) = (name.clone(), *repeat);
                self.tap_key(&name);
                if repeat {
                    self.repeat_at.insert(cid, t + self.config.repeat_delay);
                }
                return;
            }
            Action::Press => None,
            Action::Zones(zones) => match media_zone(w, Some(lx)) {
                Some(z) if zones.iter().any(|c| c == z) => Some(z),
                _ => return,
            },
        };
        let mut msg = json!({"type": "press", "generation": self.generation, "widget": key, "pointer": cid});
        if let Some(zone) = zone {
            msg["zone"] = zone.into();
        }
        self.pointers.insert(cid);
        self.request(msg);
    }

    fn tap_key(&mut self, name: &str) {
        // Keys the service will tap for us (minor 0): Esc, F1-F12.
        let code: u16 = match name {
            "esc" => 1,
            "f11" => 87,
            "f12" => 88,
            f => match f.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
                Some(n @ 1..=10) => 58 + n,
                _ => {
                    log(&format!("unsupported key {name:?}; the service allows esc and f1-f12"));
                    return;
                }
            },
        };
        let payload = Packer::default().u8(1).pad(3).u16(code).u16(0).u16(0).u16(0).0;
        let Some(conn) = self.conn.as_mut() else { return };
        if let Err(e) = conn.send(proto::TAP_KEYS, &payload, None, &format!("tap {name}")) {
            log(&format!("tap {name}: {e}"));
        }
    }

    // --- agents widgets -----------------------------------------------------
    fn limits_for(&self, w: &Widget) -> Vec<Limit> {
        let limits = match &w.data { Data::Agents { limits, .. } => limits.clone(), _ => vec![] };
        if !limits.is_empty() {
            return limits;
        }
        ["Session", "Weekly"].iter().map(|l| Limit { label: l.to_string(), frac: None, resets: None }).collect()
    }

    /// Short limit labels: "Session" -> "5h", "Weekly" -> "7d", or `shortLabels`.
    fn short_label(w: &Widget, label: &str) -> String {
        if let Some(v) = w.spec.get("shortLabels").and_then(|m| m.get(label)) {
            return v.as_str().map_or_else(|| v.to_string(), String::from);
        }
        if let Some((_, short)) = SHORT_LABELS.iter().find(|(l, _)| *l == label) {
            return short.to_string();
        }
        label.chars().take(1).collect::<String>().to_lowercase()
    }

    /// How much of the row layout's text fits above every meter: 0 the full
    /// label, 1 the short label, 2 also without the percent (the meter shows
    /// it), 3 also without the reset time. The widget uses one level so its
    /// meters match.
    fn row_fit(cr: &Context, w: &Widget, rows: &[(String, String, String)], width: f64) -> u8 {
        let fits = |label: &str, reset: &str, pct: &str| {
            let mut t = extents(cr, label).x_advance();
            if !reset.is_empty() {
                t += extents(cr, &format!("  {reset}")).x_advance();
            }
            if !pct.is_empty() {
                t += 6.0 + extents(cr, pct).x_advance();
            }
            t <= width
        };
        let level = |(label, reset, pct): &(String, String, String)| {
            let short = Self::short_label(w, label);
            if fits(label, reset, pct) {
                0
            } else if fits(&short, reset, pct) {
                1
            } else if fits(&short, reset, "") {
                2
            } else {
                3
            }
        };
        rows.iter().map(level).max().unwrap_or(0)
    }

    /// Widths of the label, percent and reset columns in the stacked layout.
    fn stacked_columns(&self, w: &Widget) -> (i32, i32, i32) {
        let label_w = self
            .limits_for(w)
            .iter()
            .map(|l| self.text_ext(&Self::short_label(w, &l.label), STACKED_FONT).x_advance())
            .fold(0.0, f64::max);
        let widest = reset_widest(w);
        let reset_w = if widest.is_empty() { 0.0 } else { self.text_ext(&widest, STACKED_FONT).x_advance() };
        (label_w as i32, self.text_ext("100%", STACKED_FONT).x_advance() as i32, reset_w as i32)
    }

    // --- mic widget ---------------------------------------------------------
    /// Show the waveform: another app is recording and the mic is live.
    fn mic_recording(&self, w: &Widget) -> bool {
        let wave = w.spec.get("waveform").is_none_or(|v| truthy(Some(v)));
        matches!(w.data, Data::Mic { muted: Some(false), in_use: true, .. }) && wave
    }

    /// Recording layout, relative to the widget's left edge: (icon centre x,
    /// waveform x, widget width). The icon's ink sits as far from the left of
    /// the key face as the waveform's last dot does from the right.
    fn mic_layout(&self, w: &Widget) -> (f64, f64, i32) {
        let (s, pad, gap) = (&w.spec, 16.0, 14.0);
        let style = dot_style(s);
        let sp = f64::from(style.sp);
        let icon_w = self.text_ext(ICON_MIC, num(s, "iconSize", 30.0)).width();
        let left = f64::from(KEY_PAD) + pad;
        let wave_x = left + icon_w + gap - (sp / 2.0 - style.radius); // first dot's ink at the gap
        let columns = (int(s, "waveformWidth", 120) / i64::from(style.sp)) as f64;
        let right = wave_x + (columns - 1.0) * sp + sp / 2.0 + style.radius; // last dot's ink
        (left + icon_w / 2.0, wave_x, round_even(right + pad + f64::from(KEY_PAD)) as i32)
    }

    // --- graph widgets ------------------------------------------------------
    fn poll_theme(&mut self) {
        let mt = mtime(&home().join(BTOP_THEME));
        if self.theme_mtime != Some(mt) {
            self.theme_mtime = Some(mt);
            self.theme = read_btop_theme();
            self.dirty = true;
        }
    }

    /// One list of RGB stops per series: the config's, else btop's theme.
    fn graph_gradients(&self, w: &Widget, source: &Source) -> Vec<Vec<Rgb>> {
        let custom: Option<Vec<Vec<Value>>> = match w.spec.get("gradient").and_then(Value::as_array) {
            Some(a) if !a.is_empty() && a[0].is_string() => Some(vec![a.clone()]),
            Some(a) if !a.is_empty() => Some(a.iter().map(|g| g.as_array().cloned().unwrap_or_default()).collect()),
            _ => None,
        };
        let white = rgb(0xFF, 0xFF, 0xFF);
        source
            .gradients()
            .iter()
            .enumerate()
            .map(|(i, (name, fallback))| {
                let stops = custom.as_ref().map(|c| c[i.min(c.len() - 1)].clone()).unwrap_or_default();
                if !stops.is_empty() {
                    return stops.iter().map(|s| hex_rgb(Some(s), white)).collect();
                }
                let themed: Vec<&String> =
                    ["start", "mid", "end"].iter().filter_map(|k| self.theme.get(&format!("{name}_{k}"))).collect();
                if themed.is_empty() {
                    fallback.iter().map(|s| hex_rgb(Some(&Value::from(*s)), white)).collect()
                } else {
                    themed.iter().map(|s| hex_rgb(Some(&Value::from(s.as_str())), white)).collect()
                }
            })
            .collect()
    }

    /// [(part, width)] left to right: label, graph, cores, value.
    fn graph_parts(&self, w: &Widget) -> Vec<(Part, i32)> {
        let (s, gap) = (&w.spec, 12);
        let sp = dot_style(s).sp;
        let Some(source) = w.source.as_ref() else { return vec![] };
        let label = source.label_text();
        let mut parts = vec![];
        if !label.is_empty() {
            parts.push((Part::Label, self.text_ext(&label, num(s, "fontSize", 18.0)).x_advance() as i32 + gap));
        }
        parts.push((Part::Graph, graph_columns(s) as i32 * sp));
        if truthy(s.get("cores")) && source.kind == sources::Kind::Cpu {
            let cores = if source.cores.is_empty() { cpu_count() } else { source.cores.len() };
            parts.push((Part::Cores, gap + (3 * cores as i32 - 1) * sp));
        }
        if s.get("showValue").is_none_or(|v| truthy(Some(v))) {
            let widest = source.widest();
            let size = if widest.len() > 1 { 15.0 } else { num(s, "fontSize", 18.0) };
            let max = widest.iter().map(|t| self.text_ext(t, size).x_advance()).fold(0.0, f64::max);
            parts.push((Part::Value, gap + max as i32));
        }
        parts
    }

    // --- drawing primitives -------------------------------------------------
    fn color(&self, c: Rgb, alpha: f64) -> (f64, f64, f64, f64) {
        let k = if self.dimmed { DIM } else { 1.0 };
        (c[0] / 255.0 * k, c[1] / 255.0 * k, c[2] / 255.0 * k, alpha)
    }

    fn set_color(&self, cr: &Context, c: Rgb, alpha: f64) {
        let (r, g, b, a) = self.color(c, alpha);
        cr.set_source_rgba(r, g, b, a);
    }

    /// Fill a rect [x0,x1) x [y0,y1) in a solid colour.
    fn fill(&self, cr: &Context, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgb) {
        cr.rectangle(f64::from(x0), f64::from(y0), f64::from(x1 - x0), f64::from(y1 - y0));
        self.set_color(cr, c, 1.0);
        let _ = cr.fill();
    }

    fn draw_face(&self, cr: &Context, x0: f64, y0: f64, x1: f64, y1: f64, pressed: bool) {
        let p = f64::from(KEY_PAD);
        cr.rectangle(x0 + p, y0 + p, x1 - x0 - 2.0 * p, y1 - y0 - 2.0 * p);
        self.set_color(cr, if pressed { self.config.key_pressed } else { self.config.key }, 1.0);
        let _ = cr.fill();
    }

    fn draw_key_face(&self, cr: &Context, w: &Widget) {
        let [x0, y0, x1, y1] = w.rect.map(f64::from);
        self.draw_face(cr, x0, y0, x1, y1, self.pressed(w));
    }

    /// Draw text centred on (cx, cy) by its ink extents (good for icons).
    fn draw_text(&self, cr: &Context, t: &str, cx: f64, cy: f64, size: f64, c: Rgb) {
        cr.set_font_size(size);
        let e = extents(cr, t);
        cr.move_to(cx - e.width() / 2.0 - e.x_bearing(), cy - e.height() / 2.0 - e.y_bearing());
        self.set_color(cr, c, 1.0);
        let _ = cr.show_text(t);
    }

    /// One line of text vertically centred on cy by the font's metrics.
    #[allow(clippy::too_many_arguments)]
    fn draw_text_at(&self, cr: &Context, t: &str, mut x: f64, cy: f64, size: f64, c: Rgb, right: bool) {
        cr.set_font_size(size);
        let (ascent, descent) = cr.font_extents().map_or((0.0, 0.0), |f| (f.ascent(), f.descent()));
        if right {
            x -= extents(cr, t).x_advance();
        }
        cr.move_to(x, cy + (ascent - descent) / 2.0);
        self.set_color(cr, c, 1.0);
        let _ = cr.show_text(t);
    }

    /// Draw a text line centred in the widget on the font's baseline.
    fn draw_label(&self, cr: &Context, t: &str, w: &Widget, size: f64, c: Rgb) {
        let [x0, y0, x1, y1] = w.rect.map(f64::from);
        cr.set_font_size(size);
        let (ascent, descent) = cr.font_extents().map_or((0.0, 0.0), |f| (f.ascent(), f.descent()));
        cr.move_to((x0 + x1 - extents(cr, t).x_advance()) / 2.0, (y0 + y1) / 2.0 + (ascent - descent) / 2.0);
        self.set_color(cr, c, 1.0);
        let _ = cr.show_text(t);
    }

    fn ellipsize(cr: &Context, t: &str, size: f64, limit: f64) -> String {
        cr.set_font_size(size);
        if extents(cr, t).x_advance() <= limit {
            return t.to_string();
        }
        let mut chars: Vec<char> = t.chars().collect();
        while !chars.is_empty() && extents(cr, &format!("{}…", chars.iter().collect::<String>())).x_advance() > limit {
            chars.pop();
        }
        format!("{}…", chars.iter().collect::<String>().trim_end())
    }

    /// btop-style dot columns, one per value (0-1), newest on the right.
    ///
    /// half=0 fills the full height from the bottom; 1 and -1 use the top half
    /// growing up from the middle, or the bottom half growing down.
    #[allow(clippy::too_many_arguments)]
    fn draw_dots(&self, cr: &Context, style: &DotStyle, x: f64, values: &[f64], columns: usize, stops: &[Rgb], half: i32) {
        let sp = f64::from(style.sp);
        let mut rows = ((self.short - 2 * KEY_PAD - 16) / style.sp + 1) as usize;
        let mid = f64::from(self.short) / 2.0;
        let (base, step) = if half != 0 {
            rows /= 2;
            (mid - f64::from(half) * sp / 2.0, -f64::from(half) * sp)
        } else {
            (mid + (rows as f64 - 1.0) * sp / 2.0, -sp)
        };
        let values = &values[values.len().saturating_sub(columns)..];
        let pad = columns - values.len();
        let mut lit_rows: Vec<Vec<f64>> = vec![vec![]; rows];
        let mut empty = vec![];
        for col in 0..columns {
            let v = if col < pad { None } else { Some(values[col - pad]) };
            let cx = x + col as f64 * sp + sp / 2.0;
            let lit = v.map_or(0, |v| {
                let r = round_even(v * rows as f64);
                let r = if r != 0 { r } else { i64::from(v > 0.01) };
                r.clamp(i64::MIN, rows as i64).max(0) as usize
            });
            if style.bars {
                if lit > 0 {
                    let y0 = base - step / 2.0;
                    let y1 = base + (lit as f64 - 0.5) * step;
                    let grad = LinearGradient::new(0.0, y0, 0.0, base + (rows as f64 - 0.5) * step);
                    for (i, c) in stops.iter().enumerate() {
                        let (r, g, b, a) = self.color(*c, 1.0);
                        grad.add_color_stop_rgba(i as f64 / (stops.len().max(2) - 1) as f64, r, g, b, a);
                    }
                    cr.rectangle(cx - sp / 2.0 + 0.5, y0.min(y1), sp - 1.0, (y1 - y0).abs());
                    let _ = cr.set_source(&grad);
                    let _ = cr.fill();
                }
                continue;
            }
            for (row, lit_row) in lit_rows.iter_mut().enumerate() {
                if row < lit {
                    lit_row.push(cx);
                } else if style.grid {
                    empty.push((cx, base + row as f64 * step));
                }
            }
        }
        // One path per colour: a fill per dot is what makes this slow.
        if !empty.is_empty() {
            for (cx, cy) in empty {
                cr.new_sub_path();
                cr.arc(cx, cy, style.radius, 0.0, TAU);
            }
            self.set_color(cr, self.config.text, 0.12);
            let _ = cr.fill();
        }
        for (row, xs) in lit_rows.iter().enumerate() {
            if xs.is_empty() {
                continue;
            }
            for cx in xs {
                cr.new_sub_path();
                cr.arc(*cx, base + row as f64 * step, style.radius, 0.0, TAU);
            }
            self.set_color(cr, lerp_rgb(stops, row as f64 / (rows.max(2) - 1) as f64), 1.0);
            let _ = cr.fill();
        }
    }

    fn rounded_rect(cr: &Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
        cr.new_sub_path();
        cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.0);
        cr.arc(x + w - r, y + h - r, r, 0.0, FRAC_PI_2);
        cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
        cr.arc(x + r, y + r, r, PI, 1.5 * PI);
        cr.close_path();
    }

    // --- widgets ------------------------------------------------------------
    fn draw_esc(&self, cr: &Context, w: &Widget) {
        let [x0, y0, x1, y1] = w.rect;
        let pad = KEY_PAD;
        let face = if self.pressed(w) { self.config.key_pressed } else { self.config.key };
        self.fill(cr, x0 + pad, y0 + pad, x1 - pad, y1 - pad, face);
        // "esc" label, 5x7 glyphs scaled
        let scale = ((self.short - 2 * pad) / 14).max(1);
        let label = "esc";
        let tw = (label.len() as i32 * 6 - 1) * scale;
        let tx = x0 + (x1 - x0 - tw).div_euclid(2);
        let ty = (self.short - 7 * scale).div_euclid(2);
        for (i, ch) in label.chars().enumerate() {
            let rows = GLYPHS.iter().find(|(c, _)| *c == ch).unwrap().1;
            for (gy, line) in rows.iter().enumerate() {
                for (gx, px) in line.chars().enumerate() {
                    if px == '#' {
                        let lx = tx + (i as i32 * 6 + gx as i32) * scale;
                        let ly = ty + gy as i32 * scale;
                        self.fill(cr, lx, ly, lx + scale, ly + scale, self.config.text);
                    }
                }
            }
        }
    }

    fn draw_button(&self, cr: &Context, w: &Widget) {
        self.draw_key_face(cr, w);
        let [x0, y0, x1, y1] = w.rect.map(f64::from);
        let s = &w.spec;
        if truthy(s.get("icon")) {
            self.draw_text(cr, &text(s, "icon", ""), (x0 + x1) / 2.0, (y0 + y1) / 2.0, num(s, "iconSize", 30.0), self.config.text);
        } else if truthy(s.get("label")) {
            self.draw_label(cr, &text(s, "label", ""), w, num(s, "fontSize", 18.0), self.config.text);
        }
    }

    fn draw_command(&self, cr: &Context, w: &Widget) {
        self.draw_key_face(cr, w);
        let (t, urgent) = match &w.data { Data::Command { text, urgent } => (text.as_str(), *urgent), _ => ("", false) };
        let c = if urgent { self.config.urgent } else { self.config.text };
        self.draw_label(cr, t, w, num(&w.spec, "fontSize", 18.0), c);
    }

    /// Provider marks from the Omarchy agents panel, with a glyph fallback.
    fn draw_provider_icon(&self, cr: &Context, w: &Widget, cx: f64, cy: f64) {
        if let Some(icon) = self.provider_icons.get(&text(&w.spec, "agent", "claude")) {
            let _ = cr.save();
            cr.translate(cx - 15.0, cy - 15.0);
            cr.scale(30.0 / f64::from(icon.width()), 30.0 / f64::from(icon.height()));
            let _ = cr.set_source_surface(icon, 0.0, 0.0);
            let _ = cr.paint();
            let _ = cr.restore();
        } else {
            self.draw_text(cr, USAGE_ICON, cx, cy, 30.0, self.config.text);
        }
    }

    /// Agents icon, then one row per limit: short label, meter, percent.
    fn draw_agents_stacked(&self, cr: &Context, w: &Widget) {
        let (white, urgent) = (self.config.text, self.config.urgent);
        let meter_w = f64::from(meter_width(&w.spec));
        let (label_w, pct_w, reset_w) = self.stacked_columns(w);
        let (label_w, pct_w) = (f64::from(label_w), f64::from(pct_w));
        let x0 = f64::from(w.rect[0] + KEY_PAD);
        let mid = f64::from(self.short) / 2.0;
        self.draw_provider_icon(cr, w, x0 + 14.0 + 15.0, mid);
        let limits = self.limits_for(w);
        let x = x0 + 14.0 + 30.0 + 14.0;
        for (i, l) in limits.iter().enumerate() {
            let cy = mid + (i as f64 - (limits.len() as f64 - 1.0) / 2.0) * 22.0;
            let alarm = l.frac.is_some_and(|f| f >= METER_ALARM);
            self.draw_text_at(cr, &Self::short_label(w, &l.label), x, cy, STACKED_FONT, white, false);
            let mx = x + label_w + 10.0;
            Self::rounded_rect(cr, mx, cy - 4.0, meter_w, 8.0, 4.0);
            self.set_color(cr, white, 0.2);
            let _ = cr.fill();
            if let Some(f) = l.frac.filter(|f| *f != 0.0) {
                Self::rounded_rect(cr, mx, cy - 4.0, (meter_w * f.min(1.0)).max(8.0), 8.0, 4.0);
                self.set_color(cr, if alarm { urgent } else { white }, 1.0);
                let _ = cr.fill();
            }
            let pct = l.frac.map_or("—".to_string(), |f| format!("{}%", round_even(f * 100.0)));
            self.draw_text_at(cr, &pct, mx + meter_w + 10.0 + pct_w, cy, STACKED_FONT, if alarm { urgent } else { white }, true);
            if reset_w != 0 {
                let reset = reset_text(w, l.resets, self.clock);
                self.draw_text_at(cr, &reset, mx + meter_w + 10.0 + pct_w + 12.0, cy, STACKED_FONT, RESET_COLOR, false);
            }
        }
    }

    /// Key-style container: agents icon, then one meter per limit.
    fn draw_agents(&self, cr: &Context, w: &Widget) {
        self.draw_key_face(cr, w);
        if text(&w.spec, "layout", "") == "stacked" {
            self.draw_agents_stacked(cr, w);
            return;
        }
        let (gap, icon_w, meter_w) = (24.0, 30.0, f64::from(meter_width(&w.spec)));
        let (white, urgent) = (self.config.text, self.config.urgent);
        let x0 = f64::from(w.rect[0] + KEY_PAD);
        self.draw_provider_icon(cr, w, x0 + 14.0 + icon_w / 2.0, f64::from(self.short) / 2.0);
        cr.set_font_size(15.0);
        let mut x = x0 + 14.0 + icon_w + 14.0;
        let limits = self.limits_for(w);
        let rows: Vec<(String, String, String)> = limits
            .iter()
            .map(|l| {
                let pct = l.frac.map_or("—".to_string(), |f| format!("{}%", round_even(f * 100.0)));
                (l.label.clone(), reset_text(w, l.resets, self.clock), pct)
            })
            .collect();
        let fit = Self::row_fit(cr, w, &rows, meter_w);
        for (l, (label, reset, pct)) in limits.iter().zip(&rows) {
            let alarm = l.frac.is_some_and(|f| f >= METER_ALARM);
            self.set_color(cr, white, 1.0);
            cr.move_to(x, 25.0);
            let _ = cr.show_text(&if fit == 0 { label.clone() } else { Self::short_label(w, label) });
            // The reset time and percent at the right end.
            let mut right = x + meter_w;
            if fit < 2 {
                right -= extents(cr, pct).x_advance();
                self.set_color(cr, if alarm { urgent } else { white }, 1.0);
                cr.move_to(right, 25.0);
                let _ = cr.show_text(pct);
                right -= 6.0;
            }
            if fit < 3 && !reset.is_empty() {
                self.set_color(cr, RESET_COLOR, 1.0);
                cr.move_to(right - extents(cr, reset).x_advance(), 25.0);
                let _ = cr.show_text(reset);
            }
            Self::rounded_rect(cr, x, 33.0, meter_w, 10.0, 5.0);
            self.set_color(cr, white, 0.2);
            let _ = cr.fill();
            if let Some(f) = l.frac.filter(|f| *f != 0.0) {
                Self::rounded_rect(cr, x, 33.0, (meter_w * f.min(1.0)).max(10.0), 10.0, 5.0);
                self.set_color(cr, if alarm { urgent } else { white }, 1.0);
                let _ = cr.fill();
            }
            x += meter_w + gap;
        }
    }

    /// Level meter: filled left to right, all in the gradient's colour for the level.
    fn draw_meter(&self, cr: &Context, w: &Widget, columns: usize, x: f64, level: Option<f64>, stops: &[Rgb]) {
        let style = dot_style(&w.spec);
        let Some(level) = level else {
            self.draw_dots(cr, &style, x, &[], columns, stops, 0);
            return;
        };
        let level = level.clamp(0.0, 1.0);
        let c = lerp_rgb(stops, level);
        let sp = f64::from(style.sp);
        if style.bars {
            let rows = f64::from((self.short - 2 * KEY_PAD - 16) / style.sp + 1);
            let h = (rows - 1.0) * sp + sp * 0.64;
            let top = f64::from(self.short) / 2.0 - h / 2.0;
            Self::rounded_rect(cr, x, top, columns as f64 * sp, h, 4.0);
            self.set_color(cr, self.config.text, 0.12);
            let _ = cr.fill();
            if level > 0.0 {
                Self::rounded_rect(cr, x, top, (columns as f64 * sp * level).max(8.0), h, 4.0);
                self.set_color(cr, c, 1.0);
                let _ = cr.fill();
            }
            return;
        }
        let lit = match round_even(level * columns as f64) {
            0 => usize::from(level > 0.0),
            n => n as usize,
        };
        let values: Vec<f64> = (0..columns).map(|i| if i < lit { 1.0 } else { 0.0 }).collect();
        self.draw_dots(cr, &style, x, &values, columns, &[c], 0);
    }

    /// Key-style container: label, history graph, per-core meters, current value.
    fn draw_graph(&self, cr: &Context, w: &Widget) {
        let s = &w.spec;
        let (Some(source), Data::Graph { series, scale }) = (w.source.as_ref(), &w.data) else { return };
        let columns = graph_columns(s);
        let style = dot_style(s);
        let gradients = self.graph_gradients(w, source);
        self.draw_key_face(cr, w);
        let (mut x, cy) = (f64::from(w.rect[0] + KEY_PAD + 14), f64::from(self.short) / 2.0);
        for (part, width) in self.graph_parts(w) {
            match part {
                Part::Label => {
                    self.draw_text_at(cr, &source.label_text(), x, cy, num(s, "fontSize", 18.0), self.config.text, false);
                }
                Part::Graph if source.meter() => {
                    let level = series.first().and_then(|s| s.last()).copied();
                    self.draw_meter(cr, w, columns, x, level, &gradients[0]);
                }
                Part::Graph => {
                    for (i, history) in series.iter().enumerate() {
                        let scale = scale.get(i).copied().filter(|s| *s != 0.0).unwrap_or(1.0);
                        let values: Vec<f64> = history.iter().map(|v| v / scale).collect();
                        let half = if source.mirrored() { if i == 0 { 1 } else { -1 } } else { 0 };
                        self.draw_dots(cr, &style, x, &values, columns, &gradients[i.min(gradients.len() - 1)], half);
                    }
                }
                Part::Cores => {
                    let mut cx = x + 12.0;
                    let cores: Vec<Option<f64>> = if source.cores.is_empty() {
                        vec![None; cpu_count()]
                    } else {
                        source.cores.iter().copied().map(Some).collect()
                    };
                    for frac in cores {
                        let values: Vec<f64> = frac.map_or(vec![], |f| vec![f, f]);
                        self.draw_dots(cr, &style, cx, &values, 2, &gradients[0], 0);
                        cx += f64::from(3 * style.sp);
                    }
                }
                Part::Value => self.draw_values(cr, w, &source.lines, x + f64::from(width)),
            }
            x += f64::from(width);
        }
    }

    /// Value lines, right-aligned so the units stay put.
    fn draw_values(&self, cr: &Context, w: &Widget, lines: &[(String, bool)], right: f64) {
        let dash = [("—".to_string(), false)];
        let lines = if lines.is_empty() { &dash[..] } else { lines };
        let size = if lines.len() > 1 { 15.0 } else { num(&w.spec, "fontSize", 18.0) };
        for (i, (t, urgent)) in lines.iter().enumerate() {
            let y = f64::from(self.short) / 2.0 + (i as f64 - (lines.len() as f64 - 1.0) / 2.0) * 18.0;
            let c = if *urgent { self.config.urgent } else { self.config.text };
            self.draw_text_at(cr, t, right, y, size, c, true);
        }
    }

    /// Mic key: muted/live icon, plus a mirrored live waveform while recording.
    fn draw_mic(&self, cr: &Context, w: &Widget) {
        let s = &w.spec;
        let [x0, y0, _, y1] = w.rect.map(f64::from);
        self.draw_key_face(cr, w);
        let active = hex_rgb(s.get("activeColor"), self.config.urgent);
        let (icon, c) = match &w.data {
            Data::Mic { muted: Some(false), in_use, .. } => (ICON_MIC, if *in_use { active } else { self.config.text }),
            _ => (ICON_MIC_MUTED, hex_rgb(s.get("mutedColor"), rgb(0x80, 0x80, 0x80))),
        };
        let size = num(s, "iconSize", 30.0);
        if !self.mic_recording(w) {
            self.draw_text(cr, icon, x0 + int(s, "width", KEY_WIDTH) as f64 / 2.0, (y0 + y1) / 2.0, size, c);
            return;
        }
        let (icon_cx, wave_x, _) = self.mic_layout(w);
        self.draw_text(cr, icon, x0 + icon_cx, (y0 + y1) / 2.0, size, c);
        let mut style = dot_style(s);
        style.grid = false; // grid-free dots in the active colour, growing out from the middle
        let columns = (int(s, "waveformWidth", 120) / i64::from(style.sp)) as usize;
        let levels: &[f64] = match &w.data { Data::Mic { levels, .. } => levels, _ => &[] };
        for half in [1, -1] {
            self.draw_dots(cr, &style, x0 + wave_x, levels, columns, &[active], half);
        }
    }

    /// Previous / play-pause / next keys and the track, from Omarchy's media service.
    fn draw_media(&self, cr: &Context, w: &Widget) {
        let s = &w.spec;
        let empty = json!({});
        let media = match &w.data { Data::Media(m) => m, _ => &empty };
        let flag = |k: &str| truthy(media.get(k));
        let field = |k: &str| media.get(k).and_then(Value::as_str).filter(|v| !v.is_empty()).map(String::from);
        let [x0, y0, x1, y1] = w.rect.map(f64::from);
        let bw = int(s, "buttonWidth", 100) as f64;
        let pressed: HashSet<&str> = self
            .owner
            .iter()
            .filter(|(_, k)| **k == w.key)
            .filter_map(|(cid, _)| media_zone(w, self.touch_x.get(cid).copied()))
            .collect();
        let zones = [
            ("previous", ICON_PREVIOUS, flag("canGoPrevious")),
            ("playPause", if flag("playing") { ICON_PAUSE } else { ICON_PLAY }, flag("canTogglePlaying")),
            ("next", ICON_NEXT, flag("canGoNext")),
        ];
        for (i, (zone, icon, enabled)) in zones.iter().enumerate() {
            let zx = x0 + i as f64 * bw;
            self.draw_face(cr, zx, y0, zx + bw, y1, pressed.contains(zone));
            let c = if *enabled { self.config.text } else { rgb(0x60, 0x60, 0x60) };
            self.draw_text(cr, icon, zx + bw / 2.0, (y0 + y1) / 2.0, num(s, "iconSize", 28.0), c);
        }
        let tx = x0 + 3.0 * bw;
        let title_tap = matches!(&w.action, Action::Zones(z) if z.iter().any(|z| z == "title"));
        self.draw_face(cr, tx, y0, x1, y1, pressed.contains("title") && title_tap);
        let pad = f64::from(KEY_PAD);
        cr.save().ok();
        cr.rectangle(tx + pad + 12.0, y0, x1 - tx - 2.0 * pad - 24.0, y1 - y0);
        cr.clip();
        let mid = f64::from(self.short) / 2.0;
        if flag("hasMedia") {
            let title = field("title").or_else(|| field("identity")).unwrap_or_else(|| "Unknown".into());
            let artist = field("artist").or_else(|| field("identity")).unwrap_or_default();
            let limit = x1 - tx - 2.0 * pad - 24.0;
            let ty = if artist.is_empty() { mid } else { y0 + 22.0 };
            self.draw_text_at(cr, &Self::ellipsize(cr, &title, 17.0, limit), tx + pad + 12.0, ty, 17.0, self.config.text, false);
            if !artist.is_empty() {
                let a = Self::ellipsize(cr, &artist, 14.0, limit);
                self.draw_text_at(cr, &a, tx + pad + 12.0, y0 + 41.0, 14.0, rgb(0xA0, 0xA0, 0xA0), false);
            }
        } else {
            self.draw_text_at(cr, "No media", tx + pad + 12.0, mid, 17.0, rgb(0x80, 0x80, 0x80), false);
        }
        cr.restore().ok();
    }

    fn draw_border(&self, cr: &Context) {
        let (v, s, b, red) = (self.visible, self.short, 2, rgb(0xFF, 0, 0));
        self.fill(cr, 0, 0, v, b, red);
        self.fill(cr, 0, s - b, v, s, red);
        self.fill(cr, 0, 0, b, s, red);
        self.fill(cr, v - b, 0, v, s, red);
    }

    fn draw_test_pattern(&self, cr: &Context) {
        let (white, yellow) = (rgb(0xFF, 0xFF, 0xFF), rgb(0xFF, 0xFF, 0));
        for x in (100..self.long).step_by(100) {
            if x % 500 == 0 {
                self.fill(cr, x - 1, 0, x + 1, self.short, yellow);
            } else {
                self.fill(cr, x - 1, 0, x + 1, self.short / 2, white);
            }
        }
        for &x in &self.touch_marks {
            self.fill(cr, x - 3, 0, x + 3, self.short, white); // white bar under each finger
        }
    }

    /// The backend is gone: say so, so a blank bar isn't mistaken for a hang.
    fn draw_offline(&self, cr: &Context) {
        cr.set_font_size(17.0);
        let x = (f64::from(self.visible) - extents(cr, OFFLINE_TEXT).x_advance()) / 2.0;
        self.draw_text_at(cr, OFFLINE_TEXT, x, f64::from(self.short) / 2.0, 17.0, rgb(0x80, 0x80, 0x80), false);
    }

    // --- frames -------------------------------------------------------------
    fn background(&self) -> Rgb {
        let c = &self.config;
        match (c.debug_background, self.fn_held) {
            (true, true) => c.debug_bg_fn,
            (true, false) => c.debug_bg,
            _ => c.background,
        }
    }

    /// Draw the frame, or with `only` just those widgets over the previous
    /// frame. Returns the drawn widgets' rects.
    fn draw(&self, surface: &ImageSurface, only: Option<&HashSet<String>>) -> Vec<[i32; 4]> {
        let Ok(cr) = Context::new(surface) else { return vec![] };
        if self.portrait {
            cr.set_matrix(Matrix::new(0.0, 1.0, 1.0, 0.0, 0.0, 0.0)); // logical x runs down the buffer
        }
        cr.select_font_face(&self.config.font, FontSlant::Normal, FontWeight::Normal);
        let layer = self.visible_layer();
        let widgets: Vec<&Widget> =
            self.widgets.iter().filter(|w| w.layer == layer && w.visible() && only.is_none_or(|o| o.contains(&w.key))).collect();
        let bg = self.background();
        match only {
            None => self.fill(&cr, 0, 0, self.long, self.short, bg),
            Some(_) => {
                for w in &widgets {
                    let [x0, y0, x1, y1] = w.rect;
                    self.fill(&cr, x0, y0, x1, y1, bg);
                }
            }
        }
        if !self.synced {
            self.draw_offline(&cr);
        }
        for w in &widgets {
            let [x0, y0, x1, y1] = w.rect;
            cr.save().ok();
            cr.rectangle(f64::from(x0), f64::from(y0), f64::from(x1 - x0), f64::from(y1 - y0));
            cr.clip();
            match w.kind {
                WidgetKind::Esc => self.draw_esc(&cr, w),
                WidgetKind::Button => self.draw_button(&cr, w),
                WidgetKind::Command => self.draw_command(&cr, w),
                WidgetKind::Agents => self.draw_agents(&cr, w),
                WidgetKind::Graph => self.draw_graph(&cr, w),
                WidgetKind::Mic => self.draw_mic(&cr, w),
                WidgetKind::Media => self.draw_media(&cr, w),
                WidgetKind::Spacer => {}
            }
            cr.restore().ok();
        }
        if only.is_none() {
            if self.config.test_pattern {
                self.draw_test_pattern(&cr);
            }
            if self.config.border || self.config.test_pattern {
                self.draw_border(&cr);
            }
        }
        drop(cr);
        surface.flush();
        widgets.iter().map(|w| w.rect).collect()
    }

    /// Submit a frame: in full, or just the widgets in self.damage. Nothing
    /// is drawn before the first snapshot until the disconnected state shows.
    fn present(&mut self) -> Result<(), String> {
        if !(self.dirty || !self.damage.is_empty()) || self.free.is_empty() || !(self.synced || self.offline) {
            return Ok(());
        }
        let buf_id = self.free.pop_front().unwrap();
        let c = &self.config;
        let partial = !self.dirty && !self.portrait && !c.test_pattern && !c.border;
        let mut rects = vec![];
        let buf = &self.buffers[&buf_id];
        match self.last_frame.filter(|_| partial) {
            Some(last) => {
                buf.copy_from(&self.buffers[&last]); // buffers hold complete frames
                for [x0, _, x1, _] in self.draw(&buf.surface, Some(&self.damage)) {
                    let (x0, x1) = (x0.max(0), x1.min(self.w));
                    if x0 < x1 {
                        rects.push([x0 as u32, 0, (x1 - x0) as u32, self.h as u32]);
                    }
                }
            }
            None => {
                self.draw(&buf.surface, None);
            }
        }
        self.frame_id += 1;
        if rects.len() > 64 {
            rects.clear();
        }
        let mut p = Packer::default().u32(buf_id).u64(self.frame_id).u32(rects.len() as u32);
        for r in &rects {
            p = p.u32(r[0]).u32(r[1]).u32(r[2]).u32(r[3]);
        }
        let what = format!("submit frame {}", self.frame_id);
        self.conn.as_mut().ok_or("not connected")?.send(proto::SUBMIT_FRAME, &p.0, None, &what).map_err(|e| e.to_string())?;
        self.last_frame = Some(buf_id);
        self.dirty = false;
        self.damage.clear();
        Ok(())
    }

    // --- events -------------------------------------------------------------
    fn owners(&self) -> HashSet<String> {
        self.owner.values().cloned().collect()
    }

    fn on_input(&mut self, payload: &[u8]) {
        if payload.len() < 12 {
            return;
        }
        let (fn_flag, count) = (payload[8], payload[9] as usize);
        let before = (self.fn_held, self.fn_layer(), self.owners());
        self.fn_held = fn_flag != 0;

        let mut touching: Vec<(u8, i32, i32)> = vec![];
        for i in 0..count {
            let at = 12 + i * 12;
            if payload.len() < at + 12 {
                break;
            }
            let (cid, tip) = (payload[at], payload[at + 1]);
            let (x, y) = (le_u32(payload, at + 4) as i32, le_u32(payload, at + 8) as i32);
            if tip != 0 {
                let (lx, ly) = if self.portrait { (y, x) } else { (x, y) };
                touching.push((cid, lx, ly));
            }
        }

        // A contact belongs to whatever it first touched until it lifts.
        self.down = touching.iter().map(|t| t.0).collect();
        let lifted: Vec<u8> = self.pointers.iter().filter(|c| !self.down.contains(c)).copied().collect();
        for cid in lifted {
            self.pointers.remove(&cid);
            self.request(json!({"type": "release", "pointer": cid}));
        }
        let down = self.down.clone();
        self.owner.retain(|cid, _| down.contains(cid));
        self.touch_x.retain(|cid, _| down.contains(cid));
        self.repeat_at.retain(|cid, _| down.contains(cid));
        self.ignored.retain(|cid| down.contains(cid));
        let t = now();
        for &(cid, lx, ly) in &touching {
            if self.config.test_pattern && !self.logged_contacts.contains(&cid) {
                log(&format!("touch down id={cid} x={lx} y={ly}"));
            }
            if self.owner.contains_key(&cid) || self.ignored.contains(&cid) {
                continue;
            }
            let layer = self.visible_layer();
            let hit = self.widgets.iter().find(|w| w.layer == layer && w.visible() && w.kind != WidgetKind::Spacer && w.hit(lx, ly));
            if let Some(key) = hit.map(|w| w.key.clone()) {
                self.owner.insert(cid, key.clone());
                self.touch_x.insert(cid, lx);
                self.press(cid, &key, lx, t);
            }
        }

        if self.config.test_pattern {
            self.logged_contacts = down;
            let marks: Vec<i32> = touching.iter().map(|t| t.1).collect();
            if marks != self.touch_marks {
                self.touch_marks = marks;
                self.dirty = true;
            }
        }
        let after = (self.fn_held, self.fn_layer(), self.owners());
        if (after.1, &after.2) != (before.1, &before.2) || (self.config.debug_background && after.0 != before.0) {
            self.dirty = true;
        }
        self.sync_view();
        if count > 0 {
            self.last_input = t;
            if self.dimmed {
                self.dimmed = false;
                self.dirty = true;
            }
        }
    }

    /// Repeat held keys (backend actions repeat in the backend).
    fn run_repeats(&mut self, t: f64) {
        let due: Vec<(u8, f64)> = self.repeat_at.iter().filter(|(_, d)| t >= **d).map(|(c, d)| (*c, *d)).collect();
        for (cid, d) in due {
            let key = self.owner.get(&cid).and_then(|k| self.widget(k)).map(|w| w.action.clone());
            if let Some(Action::Key { name, .. }) = key {
                self.tap_key(&name);
            }
            self.repeat_at.insert(cid, (d + self.config.repeat_interval).max(t));
        }
    }

    fn handle(&mut self, mtype: u16, req: u32, payload: &[u8]) {
        match mtype {
            proto::ACK => {
                self.conn.as_mut().map(|c| c.pending.remove(&req));
            }
            proto::ERROR => {
                let code = if payload.len() >= 4 { le_u32(payload, 0) } else { 0 };
                let what = self.conn.as_mut().and_then(|c| c.pending.remove(&req)).unwrap_or_else(|| "?".into());
                log(&format!("{what}: error {code} ({})", proto::error_name(code)));
            }
            proto::FRAME_RELEASED if payload.len() >= 12 => {
                let _frame = le_u64(payload, 4);
                self.free.push_back(le_u32(payload, 0));
            }
            proto::INPUT_FRAME => self.on_input(payload),
            _ => log(&format!("ignoring message type {mtype:#06x}")),
        }
    }

    /// `omarchy-glance touchbar`: drive the bar until killed. Losing the
    /// backend or t1bridge reconnects instead of exiting, so t1bridge never
    /// falls back to its built-in renderer.
    pub fn run(mut self) -> Result<(), String> {
        let mut next_poll = 0.0;
        loop {
            let t = now();
            if self.conn.is_none() && t >= self.hw_retry.0 {
                self.connect_hw(t);
            }
            if self.link.is_none() && t >= self.backend_retry.0 {
                self.connect_backend(t);
            }
            if t >= next_poll {
                self.poll_theme();
                let minute = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 60);
                if self.minute != Some(minute) {
                    // reset times and countdowns
                    self.minute = Some(minute);
                    for w in self.widgets.iter().filter(|w| w.kind == WidgetKind::Agents) {
                        self.damage.insert(w.key.clone());
                    }
                }
                next_poll = t + POLL_SECONDS;
            }
            if !self.synced && !self.offline && t >= self.offline_at {
                self.offline = true;
                self.dirty = true;
            }
            self.run_repeats(t);
            let idle = self.config.idle_dim;
            if idle > 0.0 && !self.dimmed && t >= self.last_input + idle {
                self.dimmed = true;
                self.dirty = true;
            }
            if self.conn.is_some() && let Err(e) = self.present() {
                self.lost_hw(now(), &e);
            }

            let mut deadlines = vec![next_poll];
            deadlines.extend(self.repeat_at.values());
            if idle > 0.0 && !self.dimmed {
                deadlines.push(self.last_input + idle);
            }
            if self.conn.is_none() {
                deadlines.push(self.hw_retry.0);
            }
            if self.link.is_none() {
                deadlines.push(self.backend_retry.0);
            }
            if !self.synced && !self.offline {
                deadlines.push(self.offline_at);
            }
            let timeout = (deadlines.iter().copied().fold(f64::INFINITY, f64::min) - now()).max(0.0);

            let hw = self.conn.as_ref().map(Conn::raw_fd);
            let backend = self.link.as_ref().and_then(Link::fd);
            let mut pfds: Vec<libc::pollfd> = vec![];
            for (fd, write) in [(hw, false), (backend, self.link.as_ref().is_some_and(Link::wants_write))] {
                let events = if write { libc::POLLIN | libc::POLLOUT } else { libc::POLLIN };
                pfds.push(libc::pollfd { fd: fd.unwrap_or(-1), events, revents: 0 }); // poll skips -1
            }
            let ms = (timeout * 1000.0).ceil().min(f64::from(i32::MAX)) as i32;
            let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, ms) };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(format!("poll: {err}"));
            }
            if pfds[0].revents != 0 && let Some(conn) = self.conn.as_mut() {
                match conn.recv() {
                    Ok((mtype, req, payload)) => self.handle(mtype, req, &payload),
                    Err(e) => self.lost_hw(now(), &e.to_string()),
                }
            }
            if pfds[1].revents & libc::POLLOUT != 0 && let Some(link) = self.link.as_mut() && let Err(e) = link.flush() {
                self.lost_backend(now(), &e);
            }
            if pfds[1].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                self.pull();
            }
        }
    }
}

/// `omarchy-glance preview out.png [fn]`: draw one frame of the configured bar
/// to a PNG without the hardware, from a backend in this process that samples
/// the graphs twice a second apart and waits for command widgets.
pub fn preview(path: &str, fn_layer: bool) -> Result<(), String> {
    let mut backend = Box::new(Backend::new(Paths::user()));
    let session = backend.connect();
    let mut r = Renderer::new(Some(Link::Local { backend, session }));
    r.set_geometry(2170, 60);
    r.fn_held = fn_layer;
    r.poll_theme();
    r.request(json!({"type": "hello", "protocol": protocol::PROTOCOL, "output": "touchbar", "client": CLIENT}));
    let start = now();
    loop {
        r.pull();
        let Some(Link::Local { backend, .. }) = r.link.as_mut() else { unreachable!() };
        let t = now() - start;
        if (t >= 1.05 && !backend.busy()) || t >= 12.0 {
            break;
        }
        backend.pump(0.05);
    }
    std::thread::sleep(std::time::Duration::from_millis(20)); // past the update coalescing
    r.pull();
    if let Some(Link::Local { backend, .. }) = r.link.as_mut() {
        backend.shutdown();
    }
    if !r.synced {
        return Err("the backend sent no snapshot".into());
    }
    let surface = ImageSurface::create(Format::Rgb24, r.w, r.h).map_err(|e| e.to_string())?;
    r.draw(&surface, None);
    let mut file = fs::File::create(path).map_err(|e| e.to_string())?;
    surface.write_to_png(&mut file).map_err(|e| e.to_string())
}

// --- free helpers ------------------------------------------------------------
fn extents(cr: &Context, t: &str) -> TextExtents {
    cr.text_extents(t).unwrap_or_else(|_| TextExtents::new(0.0, 0.0, 0.0, 0.0, 0.0, 0.0))
}

fn dot_style(s: &Spec) -> DotStyle {
    let sp = int(s, "dotSpacing", 4).max(2) as i32;
    DotStyle {
        sp,
        radius: num(s, "dotSize", f64::from(sp) * 0.32),
        bars: text(s, "style", "") == "bars",
        grid: s.get("grid").is_none_or(|v| truthy(Some(v))),
    }
}

fn graph_columns(s: &Spec) -> usize {
    (int(s, "graphWidth", 100) / i64::from(dot_style(s).sp)).max(1) as usize
}

fn meter_width(s: &Spec) -> i32 {
    let stacked = text(s, "layout", "") == "stacked";
    int(s, "meterWidth", if stacked { 120 } else { 200 }) as i32
}

/// Which part of a media widget x falls in: previous, playPause, next, title.
fn media_zone(w: &Widget, lx: Option<i32>) -> Option<&'static str> {
    let bw = int(&w.spec, "buttonWidth", 100).max(1) as i32;
    let i = (lx? - w.rect[0]).div_euclid(bw);
    Some(match i {
        0 => "previous",
        1 => "playPause",
        2 => "next",
        _ => "title",
    })
}

/// When a limit resets: a clock time (with the weekday if it's more than a day
/// away), or with "resets": "countdown" the time left. `clock` replaces the
/// current time and local zone.
fn reset_text(w: &Widget, resets: Option<DateTime<FixedOffset>>, clock: Option<DateTime<FixedOffset>>) -> String {
    let mode = text(&w.spec, "resets", "time");
    if mode == "none" {
        return String::new();
    }
    let Some(resets) = resets else { return "—".into() };
    let now = clock.map_or_else(Utc::now, |c| c.with_timezone(&Utc));
    let left = (resets.with_timezone(&Utc) - now).num_milliseconds() as f64 / 1000.0;
    if left <= 0.0 {
        return "now".into();
    }
    if mode == "countdown" {
        let minutes = (left / 60.0) as i64;
        let (days, hours, mins) = (minutes / 1440, minutes / 60 % 24, minutes % 60);
        return if days > 0 {
            format!("{days}d {hours}h")
        } else if hours > 0 {
            format!("{hours}h {mins:02}m")
        } else {
            format!("{mins}m")
        };
    }
    let fmt = if left < 86400.0 { text(&w.spec, "timeFormat", "%H:%M") } else { text(&w.spec, "dayTimeFormat", "%a %H:%M") };
    match clock {
        Some(c) => strftime(&resets.with_timezone(c.offset()), &fmt),
        None => strftime(&resets.with_timezone(&Local), &fmt),
    }
}

/// A sample of the longest reset text, so the widget doesn't change width.
fn reset_widest(w: &Widget) -> String {
    match text(&w.spec, "resets", "time").as_str() {
        "none" => String::new(),
        "countdown" => "6d 23h".into(),
        _ => {
            let sample = Utc.with_ymd_and_hms(2026, 9, 30, 23, 59, 0).unwrap(); // a Wednesday
            let day = strftime(&sample, &text(&w.spec, "dayTimeFormat", "%a %H:%M"));
            let time = strftime(&sample, &text(&w.spec, "timeFormat", "%H:%M"));
            if time.chars().count() > day.chars().count() { time } else { day }
        }
    }
}

#[cfg(test)]
mod golden;

#[cfg(test)]
mod tests;
