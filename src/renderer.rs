//! The renderer: widgets, layout, drawing, input and the event loop.

use std::collections::{HashMap, HashSet, VecDeque};
use std::f64::consts::{FRAC_PI_2, PI, TAU};
use std::fs;
use std::io::Read;
use std::os::fd::{AsRawFd, RawFd};
use std::path::PathBuf;
use std::process::Child;
use std::rc::Rc;
use std::time::{SystemTime, UNIX_EPOCH};

use cairo::{Context, FontSlant, FontWeight, Format, ImageSurface, LinearGradient, Matrix, TextExtents};
use chrono::format::{Item, StrftimeItems};
use chrono::{DateTime, FixedOffset, Local, NaiveDateTime, TimeZone, Utc};
use serde_json::Value;

use crate::backend::providers::{COMMAND_TIMEOUT, MEDIA_STATUS, MEDIA_WATCH, provider_has_data, usage_dir};
use crate::config::{Config, Layer, Rgb, Spec, hex_rgb, int, lerp_rgb, num, rgb, text, truthy};
use crate::mic::{Mic, MicFd};
use crate::proc::{kill, popen, reap, shell};
use crate::proto::{self, Buffer, Conn, Packer, le_u32, le_u64};
use crate::sources::{self, Source};
use crate::{log, now};

pub const DEFAULT_CONFIG: &str = include_str!("../touchbar.default.json");
const POLL_SECONDS: f64 = 1.0; // how often to check the config and usage files

// --- hardware facts and widget metrics ---------------------------------------
// The panel reports 2170 px along the bar, but only the first 2060 light up
// (same with the stock renderer; measured 2026-10-04). Lay out within this.
const VISIBLE_WIDTH: i32 = 2060;

const KEY_WIDTH: i64 = 140; // default width of Esc and buttons
const KEY_PAD: i32 = 4; // inset of each key face inside its slot
const DIM: f64 = 0.25; // brightness multiplier while idle
const AGENTS_TOGGLE: &str = "omarchy-shell -q omarchy.agents toggle";
const USAGE_ICON: &str = "\u{F16A3}"; // the Omarchy bar's agents glyph (Nerd Font)
const METER_ALARM: f64 = 0.9; // turn red at this fraction used, like the bar panel
const SHORT_LABELS: [(&str, &str); 3] = [("Session", "5h"), ("5h window", "5h"), ("Weekly", "7d")]; // agents widget, stacked layout
const STACKED_FONT: f64 = 14.0;
const RESET_COLOR: Rgb = [160.0, 160.0, 160.0]; // agents widget: when each limit resets
const ACTIVITY: &str = "omarchy-launch-or-focus-tui btop"; // what Super+Ctrl+T opens
const MIC_TOGGLE: &str = "omarchy-audio-input-mute"; // what the mic-mute key runs (with OSD)
// Players announce changes over MPRIS; re-read the status when they do.
const ICON_MIC: &str = "\u{F036C}";
const ICON_MIC_MUTED: &str = "\u{F036D}";
const ICON_PREVIOUS: &str = "\u{F04AE}";
const ICON_PLAY: &str = "\u{F040A}";
const ICON_PAUSE: &str = "\u{F03E4}";
const ICON_NEXT: &str = "\u{F04AD}";
const BTOP_THEME: &str = ".config/btop/themes/current.theme";

// 5x7 glyphs for the Esc label
const GLYPHS: [(char, [&str; 7]); 3] = [
    ('e', ["     ", "     ", " ### ", "#   #", "#####", "#    ", " ### "]),
    ('s', ["     ", "     ", " ####", "#    ", " ### ", "    #", "#### "]),
    ('c', ["     ", "     ", " ### ", "#    ", "#    ", "#   #", " ### "]),
];

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("/"), PathBuf::from)
}

pub fn user_config() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map_or_else(|| home().join(".config"), PathBuf::from)
        .join("omarchy-glance/touchbar.json")
}

fn mtime(path: &PathBuf) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

fn cpu_count() -> usize {
    (unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) }).max(1) as usize
}

fn pyround(v: f64) -> i64 {
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Esc,
    Button,
    Command,
    Agents,
    Graph,
    Mic,
    Media,
    Spacer,
}

fn kind_of(spec: &Spec) -> Option<Kind> {
    match spec.get("type").and_then(Value::as_str) {
        Some("button") => return Some(Kind::Button),
        Some("command") => return Some(Kind::Command),
        _ => {}
    }
    let id = spec.get("id").and_then(Value::as_str).unwrap_or("");
    if sources::kind_of(id).is_some() {
        return Some(Kind::Graph);
    }
    Some(match id {
        "glance.esc" => Kind::Esc,
        "glance.agents" => Kind::Agents,
        "glance.mic" => Kind::Mic,
        "glance.media" => Kind::Media,
        "glance.spacer" => Kind::Spacer,
        _ => return None,
    })
}

/// One placed widget.
struct Widget {
    key: String, // "<layer>.<section>.<index>"
    layer: Layer,
    section: &'static str,
    kind: Kind,
    spec: Spec,
    rect: [i32; 4], // x0, y0, x1, y1
    repeats: bool,
}

impl Widget {
    fn hit(&self, x: i32, y: i32) -> bool {
        let [x0, y0, x1, y1] = self.rect;
        x0 <= x && x < x1 && y0 <= y && y < y1
    }
    fn id(&self) -> String {
        text(&self.spec, "id", "")
    }
}

/// A command or media widget's script and its last result.
struct CommandState {
    text: String,
    urgent: bool,
    next: f64,
    proc: Option<Child>,
    out: Vec<u8>,
    started: f64,
    media: Option<Value>,
}

struct GraphState {
    source: Source,
    next: f64,
    history: Vec<VecDeque<f64>>,
    columns: usize,
    error: bool,
}

#[derive(Clone, PartialEq)]
struct Limit {
    label: String,
    frac: Option<f64>,
    resets: Option<DateTime<FixedOffset>>,
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

enum Ready {
    Sock,
    Command(String),
    Mic(MicFd),
    Media,
}

pub struct Renderer {
    conn: Option<Conn>, // None in preview mode
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
    mic: Option<Mic>,           // while a glance.mic widget exists
    media_watch: Option<Child>, // dbus-monitor for player changes, while a media widget exists
    media_watch_at: f64,        // earliest (re)start
    minute: Option<u64>,        // wall-clock minute, for reset times
    repeat_at: HashMap<u8, f64>, // contact id -> time of next repeat
    last_input: f64,
    dimmed: bool,
    fn_held: bool,
    logged_contacts: HashSet<u8>,
    touch_marks: Vec<i32>, // logical x of current touches (test pattern)
    provider_data: HashMap<String, bool>,
    provider_icons: HashMap<String, ImageSurface>,
    usage: HashMap<String, (Option<SystemTime>, Vec<Limit>)>,
    usage_refresh: HashMap<String, (f64, Option<Child>)>,
    commands: HashMap<String, CommandState>,
    graphs: HashMap<String, GraphState>,
    theme: HashMap<String, String>, // btop theme colours (default graph gradients)
    theme_mtime: Option<Option<SystemTime>>,
    children: Vec<Child>, // fire-and-forget processes to reap
    config_source: Option<(PathBuf, Option<SystemTime>)>,
    config: Rc<Config>,
    loaded: bool,
    measure: Context, // scratch context for measuring text outside a frame
    clock: Option<DateTime<FixedOffset>>, // fixed time and zone instead of the system's (golden tests)
}

impl Renderer {
    pub fn new(conn: Option<Conn>) -> Renderer {
        let surface = ImageSurface::create(Format::Rgb24, 1, 1).expect("cairo surface");
        Renderer {
            conn,
            buffers: HashMap::new(),
            free: VecDeque::new(),
            w: 0,
            h: 0,
            portrait: false,
            long: 0,
            short: 0,
            visible: 0,
            dirty: true,
            damage: HashSet::new(),
            last_frame: None,
            frame_id: 0,
            widgets: vec![],
            owner: HashMap::new(),
            touch_x: HashMap::new(),
            mic: None,
            media_watch: None,
            media_watch_at: 0.0,
            minute: None,
            repeat_at: HashMap::new(),
            last_input: now(),
            dimmed: false,
            fn_held: false,
            logged_contacts: HashSet::new(),
            touch_marks: vec![],
            provider_data: HashMap::new(),
            provider_icons: [
                ("claude", include_bytes!("../assets/agents/claude.png").as_slice()),
                ("codex", include_bytes!("../assets/agents/codex.png").as_slice()),
            ].into_iter().filter_map(|(name, bytes)| {
                ImageSurface::create_from_png(&mut std::io::Cursor::new(bytes)).ok()
                    .map(|surface| (name.to_string(), surface))
            }).collect(),
            usage: HashMap::new(),
            usage_refresh: HashMap::new(),
            commands: HashMap::new(),
            graphs: HashMap::new(),
            theme: HashMap::new(),
            theme_mtime: None,
            children: vec![],
            config_source: None,
            config: Rc::new(Config::parse(DEFAULT_CONFIG).expect("built-in config")),
            loaded: false,
            measure: Context::new(&surface).expect("cairo context"),
            clock: None,
        }
    }

    // --- setup --------------------------------------------------------------
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

    fn set_geometry(&mut self, w: i32, h: i32) {
        self.w = w;
        self.h = h;
        self.portrait = h > w;
        self.long = w.max(h);
        self.short = w.min(h);
        self.visible = self.long.min(VISIBLE_WIDTH);
    }

    // --- config -------------------------------------------------------------
    fn poll_config(&mut self) {
        let user = user_config();
        let path = if user.exists() { user } else { PathBuf::from("<built-in touchbar.default.json>") };
        let source = (path.clone(), mtime(&path));
        if self.config_source.as_ref() == Some(&source) {
            return;
        }
        self.config_source = Some(source);
        let parsed = if path.exists() {
            fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| Config::parse(&s))
        } else {
            Config::parse(DEFAULT_CONFIG)
        };
        let config = match parsed {
            Ok(c) => c,
            Err(e) => {
                let what = if self.loaded { "keeping the previous config" } else { "using the default" };
                log(&format!("{}: {e}; {what}", path.display()));
                if self.loaded {
                    return;
                }
                Config::parse(DEFAULT_CONFIG).expect("built-in config")
            }
        };
        log(&format!("loaded {}", path.display()));
        self.config = Rc::new(config);
        self.loaded = true;
        for state in self.commands.values_mut() {
            if let Some(p) = state.proc.take() {
                kill(p);
            }
        }
        self.commands.clear();
        self.graphs.clear();
        self.owner.clear();
        self.repeat_at.clear();
        self.touch_x.clear();
        self.build();
        let fps = self.widgets.iter().find(|w| w.kind == Kind::Mic).map(|w| int(&w.spec, "fps", 20));
        match (fps, self.mic.is_some()) {
            (Some(fps), false) => self.mic = Some(Mic::new(fps)),
            (None, true) => {
                if let Some(mut m) = self.mic.take() {
                    m.close();
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    /// Create the widgets of both layers from the config, then place them.
    fn build(&mut self) {
        self.widgets.clear();
        let config = self.config.clone();
        for (layer, section, items) in &config.layers {
            let lname = if *layer == Layer::Fn { "fn" } else { "default" };
            for (i, spec) in items.iter().enumerate() {
                let Some(kind) = kind_of(spec) else {
                    log(&format!("layers.{lname}.{section}[{i}]: unknown widget {:?}", text(spec, "id", "")));
                    continue;
                };
                self.widgets.push(Widget {
                    key: format!("{lname}.{section}.{i}"),
                    layer: *layer,
                    section,
                    kind,
                    spec: spec.clone(),
                    rect: [0; 4],
                    repeats: kind == Kind::Button && truthy(spec.get("repeat")),
                });
            }
        }
        for w in &self.widgets {
            if matches!(w.kind, Kind::Command | Kind::Media) {
                self.commands.entry(w.key.clone()).or_insert_with(|| CommandState {
                    text: String::new(),
                    urgent: false,
                    next: 0.0,
                    proc: None,
                    out: vec![],
                    started: 0.0,
                    media: None,
                });
            }
            if w.kind == Kind::Graph && !self.graphs.contains_key(&w.key) {
                let kind = sources::kind_of(&w.id()).unwrap();
                let columns = graph_columns(&w.spec);
                self.graphs.insert(w.key.clone(), GraphState {
                    source: Source::new(kind, &w.spec),
                    next: 0.0,
                    history: vec![VecDeque::with_capacity(columns); if kind_is_mirrored(kind) { 2 } else { 1 }],
                    columns,
                    error: false,
                });
            }
        }
        self.relayout();
    }

    /// Re-place widgets after a width change (fingers stay attached by key).
    fn relayout(&mut self) {
        let config = self.config.clone();
        for (layer, section, _) in &config.layers {
            let idx: Vec<usize> =
                (0..self.widgets.len()).filter(|&i| self.widgets[i].layer == *layer && self.widgets[i].section == *section).collect();
            let widths: Vec<i32> = idx.iter().map(|&i| self.width_of(&self.widgets[i])).collect();
            let total: i32 = widths.iter().sum();
            let mut x = match *section {
                "left" => 0,
                "center" => (self.visible - total).div_euclid(2),
                _ => self.visible - total,
            };
            for (&i, width) in idx.iter().zip(widths) {
                self.widgets[i].rect = [x, 0, x + width, self.short];
                x += width;
            }
        }
        let hidden: HashSet<String> = self.widgets.iter().filter(|w| !self.widget_has_data(w)).map(|w| w.key.clone()).collect();
        self.owner.retain(|_, key| !hidden.contains(key));
        self.repeat_at.retain(|cid, _| self.owner.contains_key(cid));
        self.touch_x.retain(|cid, _| self.owner.contains_key(cid));
        self.dirty = true;
    }

    fn text_ext(&self, text: &str, size: f64) -> TextExtents {
        self.measure.select_font_face(&self.config.font, FontSlant::Normal, FontWeight::Normal);
        self.measure.set_font_size(size);
        extents(&self.measure, text)
    }

    fn width_of(&self, w: &Widget) -> i32 {
        if !self.widget_has_data(w) { return 0; }
        let s = &w.spec;
        match w.kind {
            Kind::Spacer => int(s, "size", 40) as i32,
            Kind::Agents if text(s, "layout", "") == "stacked" => {
                let (label_w, pct_w, reset_w) = self.stacked_columns(w);
                let reset_w = if reset_w != 0 { reset_w + 12 } else { 0 };
                14 + 30 + 14 + label_w + 10 + meter_width(s) + 10 + pct_w + reset_w + 16 + 2 * KEY_PAD
            }
            Kind::Agents => {
                let n = self.limits_for(w).len() as i32;
                14 + 30 + 14 + n * meter_width(s) + (n - 1) * 24 + 16 + 2 * KEY_PAD
            }
            Kind::Graph => self.graph_parts(w).iter().map(|(_, width)| width).sum::<i32>() + 2 * 14 + 2 * KEY_PAD,
            Kind::Mic if self.mic_recording(w) => self.mic_layout(w).2,
            Kind::Media => 3 * int(s, "buttonWidth", 100) as i32 + int(s, "titleWidth", 360) as i32,
            Kind::Command if !truthy(s.get("width")) => {
                let t = self.commands.get(&w.key).map_or("", |c| c.text.as_str());
                let adv = self.text_ext(t, num(s, "fontSize", 18.0)).x_advance() as i32;
                (KEY_WIDTH as i32 / 2).max(adv + 32 + 2 * KEY_PAD)
            }
            _ => int(s, "width", KEY_WIDTH) as i32,
        }
    }

    fn widget_has_data(&self, w: &Widget) -> bool {
        w.kind != Kind::Agents || self.provider_data.get(&text(&w.spec, "agent", "claude")).copied().unwrap_or(false)
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
    fn press(&mut self, key: &str, lx: Option<i32>) {
        let Some(w) = self.widget(key) else { return };
        let s = &w.spec;
        let cmd = match w.kind {
            Kind::Esc => {
                self.tap_key("esc");
                return;
            }
            Kind::Button if truthy(s.get("key")) => {
                let name = text(s, "key", "").to_lowercase();
                self.tap_key(&name);
                return;
            }
            Kind::Button => text(s, "exec", ""),
            Kind::Agents => text(s, "onTap", AGENTS_TOGGLE),
            Kind::Graph => text(s, "onTap", ACTIVITY),
            Kind::Mic => text(s, "onTap", MIC_TOGGLE),
            Kind::Media => match media_zone(w, lx) {
                Some("previous") => "omarchy-shell -q media previous".into(),
                Some("playPause") => "omarchy-shell -q media playPause".into(),
                Some("next") => "omarchy-shell -q media next".into(),
                Some(_) => text(s, "onTap", ""),
                None => String::new(),
            },
            Kind::Command => text(s, "onTap", ""),
            Kind::Spacer => String::new(),
        };
        if cmd.is_empty() {
            return;
        }
        let is_media = w.kind == Kind::Media;
        self.spawn(&cmd);
        if is_media && let Some(state) = self.commands.get_mut(key).filter(|s| s.proc.is_none()) {
            state.next = now() + 0.3; // show the new state soon
        }
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

    /// Run a shell command without blocking the render loop.
    fn spawn(&mut self, cmd: &str) {
        match shell(cmd, false) {
            Ok(p) => self.children.push(p),
            Err(e) => log(&format!("could not run {cmd:?}: {e}")),
        }
    }

    // --- command widgets ----------------------------------------------------
    fn run_commands(&mut self, t: f64) {
        self.children.retain_mut(|p| matches!(p.try_wait(), Ok(None)));
        let layer = self.visible_layer();
        let mut timed_out = vec![];
        for w in &self.widgets {
            let cmd = match w.kind {
                Kind::Media => MEDIA_STATUS.to_string(),
                Kind::Command => text(&w.spec, "exec", ""),
                _ => continue,
            };
            if cmd.is_empty() {
                continue;
            }
            let state = self.commands.get_mut(&w.key).unwrap();
            if w.kind == Kind::Media && state.proc.is_none() && w.layer != layer {
                continue; // only ask for player state while it's on screen
            }
            if state.proc.is_some() && t - state.started > COMMAND_TIMEOUT {
                log(&format!("{}: command timed out", w.id()));
                timed_out.push(w.key.clone());
            } else if state.proc.is_none() && t >= state.next {
                match shell(&cmd, true) {
                    Ok(p) => {
                        state.proc = Some(p);
                        state.out.clear();
                        state.started = t;
                    }
                    Err(e) => log(&format!("{}: could not run: {e}", w.id())),
                }
                let interval = num(&w.spec, "interval", if w.kind == Kind::Media { 30.0 } else { 0.0 });
                state.next = if interval > 0.0 { t + interval } else { f64::INFINITY };
            }
        }
        for key in timed_out {
            if let Some(p) = self.commands.get_mut(&key).and_then(|s| s.proc.as_mut()) {
                let _ = p.kill();
            }
            self.finish_command(&key);
        }
    }

    /// Next runs of command and media widgets (media only while shown).
    fn command_deadlines(&self) -> Vec<f64> {
        let layer = self.visible_layer();
        self.widgets
            .iter()
            .filter_map(|w| self.commands.get(&w.key).map(|s| (w, s)))
            .filter(|(w, s)| s.proc.is_none() && (w.kind != Kind::Media || w.layer == layer))
            .map(|(_, s)| s.next)
            .collect()
    }

    fn read_command(&mut self, key: &str) {
        let state = self.commands.get_mut(key).unwrap();
        let mut buf = vec![0u8; 65536];
        let n = state.proc.as_mut().and_then(|p| p.stdout.as_mut()).map_or(Ok(0), |s| s.read(&mut buf)).unwrap_or(0);
        if n > 0 && state.out.len() < 65536 {
            state.out.extend_from_slice(&buf[..n]);
            return;
        }
        self.finish_command(key);
    }

    fn finish_command(&mut self, key: &str) {
        let Some(state) = self.commands.get_mut(key) else { return };
        if let Some(mut p) = state.proc.take() {
            drop(p.stdout.take());
            reap(p, 1.0);
        }
        let out = String::from_utf8_lossy(&state.out).trim().to_string();
        let Some(w) = self.widgets.iter().find(|w| w.key == key) else { return };
        if w.kind == Kind::Media {
            let media = serde_json::from_str::<Value>(&out).ok().filter(Value::is_object).unwrap_or(Value::Object(Spec::new()));
            if state.media.as_ref() != Some(&media) {
                state.media = Some(media);
                self.damage.insert(key.to_string());
            }
            return;
        }
        // Plain text, or Waybar-style JSON: {"text": ..., "class": ...}
        let mut text_out = out.lines().next().unwrap_or("").to_string();
        let mut urgent = false;
        if out.starts_with('{') && let Ok(Value::Object(data)) = serde_json::from_str::<Value>(&out) {
            text_out = text(&data, "text", "");
            urgent = match data.get("class") {
                Some(Value::String(c)) => c == "urgent" || c == "critical",
                Some(Value::Array(a)) => a.iter().any(|c| matches!(c.as_str(), Some("urgent" | "critical"))),
                _ => false,
            };
        }
        if (text_out.as_str(), urgent) != (state.text.as_str(), state.urgent) {
            state.text = text_out;
            state.urgent = urgent;
            let fixed = truthy(w.spec.get("width"));
            if !fixed {
                self.relayout();
            }
            self.damage.insert(key.to_string());
        }
    }

    // --- agent usage records -----------------------------------------------
    /// Read limits for every agents widget from its Omarchy usage record.
    fn poll_usage(&mut self) {
        self.poll_usage_at(&usage_dir());
    }

    // Refresh configured providers even when their records are missing or their
    // widgets are hidden. Each provider has its own nonblocking job and cadence.
    fn refresh_usage(&mut self, t: f64) {
        let agents: HashSet<String> = self.widgets.iter()
            .filter(|w| w.kind == Kind::Agents)
            .map(|w| text(&w.spec, "agent", "claude")).collect();
        for agent in agents {
            // The updater interprets leading dashes as options.
            if agent.is_empty() || !agent.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
                || agent.starts_with('-') {
                continue;
            }
            let (next, job) = self.usage_refresh.entry(agent.clone()).or_insert((0.0, None));
            if let Some(child) = job.as_mut() {
                match child.try_wait() {
                    Ok(None) => continue,
                    Ok(Some(status)) => {
                        if !status.success() { log(&format!("usage refresh for {agent} exited with {status}")); }
                    }
                    Err(e) => log(&format!("usage refresh for {agent}: {e}")),
                }
                *job = None;
            }
            if t < *next { continue; }
            *next = t + 60.0;
            match popen(&["timeout", "--kill-after=5s", "120s", "omarchy-agent-usage-update", &agent], false) {
                Ok(child) => *job = Some(child),
                Err(e) => log(&format!("could not refresh usage for {agent}: {e}")),
            }
        }
    }

    fn poll_usage_at(&mut self, directory: &std::path::Path) {
        let agents: HashSet<String> =
            self.widgets.iter().filter(|w| w.kind == Kind::Agents).map(|w| text(&w.spec, "agent", "claude")).collect();
        for agent in agents {
            let path = directory.join(format!("{agent}.json"));
            let mt = mtime(&path);
            if self.usage.get(&agent).is_some_and(|o| o.0 == mt) {
                continue;
            }
            let record = fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|s| {
                serde_json::from_str::<Value>(&s).map_err(|e| e.to_string())
            });
            if let Err(e) = &record && mt.is_some() {
                log(&format!("unreadable usage record {}: {e}", path.display()));
            }
            self.set_usage(agent, mt, record.ok());
        }
    }

    /// Take an agent's usage record (None when missing or unreadable).
    fn set_usage(&mut self, agent: String, mt: Option<SystemTime>, record: Option<Value>) {
        let mut limits = vec![];
        let has_data = record.as_ref().is_some_and(provider_has_data);
        for entry in record.iter().filter_map(|r| r.get("limits")?.as_array()).flatten() {
            let Some(entry) = entry.as_object() else { continue };
            let frac = num(entry, "percent", -1.0);
            if frac >= 0.0 {
                let label = text(entry, "label", "");
                let label = label.split(" (").next().unwrap_or("").to_string();
                limits.push(Limit {
                    label: if label.is_empty() { "Limit".into() } else { label },
                    frac: Some(frac),
                    resets: entry.get("resetsAt").and_then(Value::as_str).and_then(parse_time),
                });
            }
        }
        let changed = self.usage.get(&agent).is_none_or(|o| o.1 != limits)
            || self.provider_data.get(&agent).copied() != Some(has_data);
        self.provider_data.insert(agent.clone(), has_data);
        self.usage.insert(agent, (mt, limits));
        if changed {
            self.relayout();
        }
    }

    fn limits_for(&self, w: &Widget) -> Vec<Limit> {
        let limits = self.usage.get(&text(&w.spec, "agent", "claude")).map(|u| u.1.clone()).unwrap_or_default();
        if !limits.is_empty() {
            return limits;
        }
        ["Session", "Weekly"].iter().map(|l| Limit { label: l.to_string(), frac: None, resets: None }).collect()
    }

    /// Stacked layout labels: "Session" -> "5h", "Weekly" -> "7d", or `shortLabels`.
    fn short_label(w: &Widget, label: &str) -> String {
        if let Some(v) = w.spec.get("shortLabels").and_then(|m| m.get(label)) {
            return v.as_str().map_or_else(|| v.to_string(), String::from);
        }
        if let Some((_, short)) = SHORT_LABELS.iter().find(|(l, _)| *l == label) {
            return short.to_string();
        }
        label.chars().take(1).collect::<String>().to_lowercase()
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

    // --- mic and media widgets ---------------------------------------------
    fn update_mic(&mut self, t: f64) {
        let wanted = self.widgets.iter().any(|w| w.kind == Kind::Mic && w.layer == self.visible_layer());
        let Some(mic) = self.mic.as_mut() else { return };
        mic.set_wanted(wanted);
        mic.tick(t);
        if !mic.changed {
            return;
        }
        mic.changed = false;
        let mics: Vec<usize> = (0..self.widgets.len()).filter(|&i| self.widgets[i].kind == Kind::Mic).collect();
        if mics.iter().any(|&i| self.width_of(&self.widgets[i]) != self.widgets[i].rect[2] - self.widgets[i].rect[0]) {
            self.relayout(); // the waveform appeared or went away
        }
        for i in mics {
            self.damage.insert(self.widgets[i].key.clone());
        }
    }

    /// Show the waveform: another app is recording and the mic is live.
    fn mic_recording(&self, w: &Widget) -> bool {
        let wave = w.spec.get("waveform").is_none_or(|v| truthy(Some(v)));
        self.mic.as_ref().is_some_and(|m| m.in_use && m.muted == Some(false)) && wave
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
        (left + icon_w / 2.0, wave_x, pyround(right + pad + f64::from(KEY_PAD)) as i32)
    }

    fn update_media_watch(&mut self, t: f64) {
        let want = self.widgets.iter().any(|w| w.kind == Kind::Media);
        if let Some(p) = self.media_watch.as_mut() {
            let exited = !matches!(p.try_wait(), Ok(None));
            if !want || exited {
                kill(self.media_watch.take().unwrap());
            }
        }
        if want && self.media_watch.is_none() && t >= self.media_watch_at {
            self.media_watch_at = t + 5.0;
            match crate::proc::popen(&MEDIA_WATCH, true) {
                Ok(p) => self.media_watch = Some(p),
                Err(e) => log(&format!("media: could not run dbus-monitor: {e}")),
            }
        }
    }

    fn read_media_watch(&mut self) {
        let mut buf = vec![0u8; 65536];
        let n = self.media_watch.as_mut().and_then(|p| p.stdout.as_mut()).map_or(Ok(0), |s| s.read(&mut buf)).unwrap_or(0);
        if n == 0 {
            if let Some(p) = self.media_watch.take() {
                kill(p);
            }
            return;
        }
        let chunk = &buf[..n];
        let has = |needle: &[u8]| chunk.windows(needle.len()).any(|w| w == needle);
        if has(b"PropertiesChanged") || has(b"NameOwnerChanged") {
            let soon = now() + 0.15;
            for w in self.widgets.iter().filter(|w| w.kind == Kind::Media) {
                let state = self.commands.get_mut(&w.key).unwrap();
                state.next = state.next.min(soon);
            }
        }
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

    fn sample_graphs(&mut self, t: f64) {
        for w in &self.widgets {
            let Some(g) = self.graphs.get_mut(&w.key) else { continue };
            if t < g.next {
                continue;
            }
            g.next = t + num(&w.spec, "interval", 1.0).max(0.25);
            match g.source.sample() {
                Ok(values) => {
                    for (history, v) in g.history.iter_mut().zip(values.unwrap_or_default()) {
                        if history.len() == g.columns {
                            history.pop_front();
                        }
                        history.push_back(v);
                    }
                    g.error = false;
                }
                Err(e) => {
                    if !g.error {
                        log(&format!("{}: {e}", w.id()));
                    }
                    g.error = true;
                }
            }
            self.damage.insert(w.key.clone());
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
        let Some(g) = self.graphs.get(&w.key) else { return vec![] };
        let source = &g.source;
        let label = source.label_text();
        let mut parts = vec![];
        if !label.is_empty() {
            parts.push((Part::Label, self.text_ext(&label, num(s, "fontSize", 18.0)).x_advance() as i32 + gap));
        }
        parts.push((Part::Graph, g.columns as i32 * sp));
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
                let r = pyround(v * rows as f64);
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
        let state = &self.commands[&w.key];
        let c = if state.urgent { self.config.urgent } else { self.config.text };
        self.draw_label(cr, &state.text, w, num(&w.spec, "fontSize", 18.0), c);
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
            let pct = l.frac.map_or("—".to_string(), |f| format!("{}%", pyround(f * 100.0)));
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
        for l in self.limits_for(w) {
            let alarm = l.frac.is_some_and(|f| f >= METER_ALARM);
            self.set_color(cr, white, 1.0);
            cr.move_to(x, 25.0);
            let _ = cr.show_text(&l.label);
            let reset = reset_text(w, l.resets, self.clock);
            if !reset.is_empty() {
                self.set_color(cr, RESET_COLOR, 1.0);
                let _ = cr.show_text(&format!("  {reset}"));
            }
            let pct = l.frac.map_or("—".to_string(), |f| format!("{}%", pyround(f * 100.0)));
            self.set_color(cr, if alarm { urgent } else { white }, 1.0);
            cr.move_to(x + meter_w - extents(cr, &pct).x_advance(), 25.0);
            let _ = cr.show_text(&pct);
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
        let lit = match pyround(level * columns as f64) {
            0 => usize::from(level > 0.0),
            n => n as usize,
        };
        let values: Vec<f64> = (0..columns).map(|i| if i < lit { 1.0 } else { 0.0 }).collect();
        self.draw_dots(cr, &style, x, &values, columns, &[c], 0);
    }

    /// Key-style container: label, history graph, per-core meters, current value.
    fn draw_graph(&self, cr: &Context, w: &Widget) {
        let s = &w.spec;
        let g = &self.graphs[&w.key];
        let style = dot_style(s);
        let source = &g.source;
        let gradients = self.graph_gradients(w, source);
        self.draw_key_face(cr, w);
        let (mut x, cy) = (f64::from(w.rect[0] + KEY_PAD + 14), f64::from(self.short) / 2.0);
        for (part, width) in self.graph_parts(w) {
            match part {
                Part::Label => {
                    self.draw_text_at(cr, &source.label_text(), x, cy, num(s, "fontSize", 18.0), self.config.text, false);
                }
                Part::Graph if source.meter() => {
                    self.draw_meter(cr, w, g.columns, x, g.history[0].back().copied(), &gradients[0]);
                }
                Part::Graph => {
                    for (i, history) in g.history.iter().enumerate() {
                        let scale = match source.scale(history) {
                            s if s != 0.0 => s,
                            _ => 1.0,
                        };
                        let values: Vec<f64> = history.iter().map(|v| v / scale).collect();
                        let half = if source.mirrored() { if i == 0 { 1 } else { -1 } } else { 0 };
                        self.draw_dots(cr, &style, x, &values, g.columns, &gradients[i], half);
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
        let m = self.mic.as_ref();
        let (icon, c) = match m {
            Some(m) if m.muted == Some(false) => (ICON_MIC, if m.in_use { active } else { self.config.text }),
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
        let levels: Vec<f64> = m.map_or(vec![], |m| m.levels.iter().copied().collect());
        for half in [1, -1] {
            self.draw_dots(cr, &style, x0 + wave_x, &levels, columns, &[active], half);
        }
    }

    /// Previous / play-pause / next keys and the track, from Omarchy's media service.
    fn draw_media(&self, cr: &Context, w: &Widget) {
        let s = &w.spec;
        let empty = Value::Object(Spec::new());
        let media = self.commands[&w.key].media.as_ref().unwrap_or(&empty);
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
        self.draw_face(cr, tx, y0, x1, y1, pressed.contains("title") && truthy(s.get("onTap")));
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
            self.widgets.iter().filter(|w| w.layer == layer && self.widget_has_data(w) && only.is_none_or(|o| o.contains(&w.key))).collect();
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
        for w in &widgets {
            let [x0, y0, x1, y1] = w.rect;
            cr.save().ok();
            cr.rectangle(f64::from(x0), f64::from(y0), f64::from(x1 - x0), f64::from(y1 - y0));
            cr.clip();
            match w.kind {
                Kind::Esc => self.draw_esc(&cr, w),
                Kind::Button => self.draw_button(&cr, w),
                Kind::Command => self.draw_command(&cr, w),
                Kind::Agents => self.draw_agents(&cr, w),
                Kind::Graph => self.draw_graph(&cr, w),
                Kind::Mic => self.draw_mic(&cr, w),
                Kind::Media => self.draw_media(&cr, w),
                Kind::Spacer => {}
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

    /// Submit a frame: in full, or just the widgets in self.damage.
    fn present(&mut self) -> Result<(), String> {
        if !(self.dirty || !self.damage.is_empty()) || self.free.is_empty() {
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
        let down: HashSet<u8> = touching.iter().map(|t| t.0).collect();
        self.owner.retain(|cid, _| down.contains(cid));
        self.touch_x.retain(|cid, _| down.contains(cid));
        self.repeat_at.retain(|cid, _| down.contains(cid));
        let t = now();
        for &(cid, lx, ly) in &touching {
            if self.config.test_pattern && !self.logged_contacts.contains(&cid) {
                log(&format!("touch down id={cid} x={lx} y={ly}"));
            }
            if self.owner.contains_key(&cid) {
                continue;
            }
            let layer = self.visible_layer();
            let hit = self.widgets.iter().find(|w| w.layer == layer && self.widget_has_data(w) && w.kind != Kind::Spacer && w.hit(lx, ly));
            if let Some(w) = hit {
                let (key, repeats) = (w.key.clone(), w.repeats);
                self.owner.insert(cid, key.clone());
                self.touch_x.insert(cid, lx);
                self.press(&key, Some(lx));
                if repeats {
                    self.repeat_at.insert(cid, t + self.config.repeat_delay);
                }
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
        if count > 0 {
            self.last_input = t;
            if self.dimmed {
                self.dimmed = false;
                self.dirty = true;
            }
        }
    }

    fn run_repeats(&mut self, t: f64) {
        let due: Vec<(u8, f64)> = self.repeat_at.iter().filter(|(_, d)| t >= **d).map(|(c, d)| (*c, *d)).collect();
        for (cid, d) in due {
            if let Some(key) = self.owner.get(&cid).cloned() {
                self.press(&key, None);
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

    pub fn run(mut self) -> Result<(), String> {
        self.handshake()?;
        let mut next_poll = 0.0;
        loop {
            let t = now();
            if t >= next_poll {
                self.poll_config();
                self.refresh_usage(t);
                self.poll_usage();
                self.poll_theme();
                let minute = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs() / 60);
                if self.minute != Some(minute) {
                    // reset times and countdowns
                    self.minute = Some(minute);
                    for w in self.widgets.iter().filter(|w| w.kind == Kind::Agents) {
                        self.damage.insert(w.key.clone());
                    }
                }
                next_poll = t + POLL_SECONDS;
            }
            self.run_commands(t);
            self.sample_graphs(t);
            self.update_mic(t);
            self.update_media_watch(t);
            self.run_repeats(t);
            let idle = self.config.idle_dim;
            if idle > 0.0 && !self.dimmed && t >= self.last_input + idle {
                self.dimmed = true;
                self.dirty = true;
            }
            self.present()?;

            let mut deadlines = vec![next_poll];
            deadlines.extend(self.repeat_at.values());
            deadlines.extend(self.command_deadlines());
            deadlines.extend(self.graphs.values().map(|g| g.next));
            if let Some(m) = &self.mic {
                deadlines.push(m.deadline());
            }
            if self.media_watch.is_none() && self.widgets.iter().any(|w| w.kind == Kind::Media) {
                deadlines.push(self.media_watch_at);
            }
            if idle > 0.0 && !self.dimmed {
                deadlines.push(self.last_input + idle);
            }
            let timeout = (deadlines.iter().copied().fold(f64::INFINITY, f64::min) - now()).max(0.0);

            let sock = self.conn.as_ref().ok_or("not connected")?.raw_fd();
            let mut fds: Vec<(RawFd, Ready)> = vec![(sock, Ready::Sock)];
            for (key, s) in &self.commands {
                if let Some(out) = s.proc.as_ref().and_then(|p| p.stdout.as_ref()) {
                    fds.push((out.as_raw_fd(), Ready::Command(key.clone())));
                }
            }
            if let Some(m) = &self.mic {
                fds.extend(m.fds().into_iter().map(|(fd, which)| (fd, Ready::Mic(which))));
            }
            if let Some(out) = self.media_watch.as_ref().and_then(|p| p.stdout.as_ref()) {
                fds.push((out.as_raw_fd(), Ready::Media));
            }
            let mut pfds: Vec<libc::pollfd> =
                fds.iter().map(|(fd, _)| libc::pollfd { fd: *fd, events: libc::POLLIN, revents: 0 }).collect();
            let ms = (timeout * 1000.0).ceil().min(f64::from(i32::MAX)) as i32;
            let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, ms) };
            if n < 0 {
                let err = std::io::Error::last_os_error();
                if err.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(format!("poll: {err}"));
            }
            for (pfd, (_, ready)) in pfds.iter().zip(fds) {
                if pfd.revents == 0 {
                    continue;
                }
                match ready {
                    Ready::Sock => {
                        let conn = self.conn.as_mut().ok_or("not connected")?;
                        let (mtype, req, payload) = conn.recv().map_err(|e| e.to_string())?;
                        self.handle(mtype, req, &payload);
                    }
                    Ready::Command(key) => self.read_command(&key),
                    Ready::Mic(which) => {
                        if let Some(m) = self.mic.as_mut() {
                            m.readable(which);
                        }
                    }
                    Ready::Media => self.read_media_watch(),
                }
            }
        }
    }
}

impl Renderer {
    /// Draw one frame of the configured bar to a PNG without the hardware:
    /// samples the graphs twice a second apart and waits for command widgets.
    pub fn preview(mut self, path: &str, fn_layer: bool) -> Result<(), String> {
        self.set_geometry(2170, 60);
        self.fn_held = fn_layer;
        self.poll_config();
        self.poll_usage();
        self.poll_theme();
        for _ in 0..2 {
            let t = now();
            for g in self.graphs.values_mut() {
                g.next = 0.0;
            }
            self.sample_graphs(t);
            self.update_mic(t);
            self.run_commands(t);
            let keys: Vec<String> = self.commands.iter().filter(|(_, s)| s.proc.is_some()).map(|(k, _)| k.clone()).collect();
            for key in keys {
                while self.commands[&key].proc.is_some() {
                    self.read_command(&key);
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
        if let Some(mut m) = self.mic.take() {
            m.close();
            self.mic = Some(m);
        }
        let surface = ImageSurface::create(Format::Rgb24, self.w, self.h).map_err(|e| e.to_string())?;
        self.draw(&surface, None);
        let mut file = fs::File::create(path).map_err(|e| e.to_string())?;
        surface.write_to_png(&mut file).map_err(|e| e.to_string())
    }
}

// --- free helpers ------------------------------------------------------------
fn extents(cr: &Context, t: &str) -> TextExtents {
    cr.text_extents(t).unwrap_or_else(|_| TextExtents::new(0.0, 0.0, 0.0, 0.0, 0.0, 0.0))
}

fn kind_is_mirrored(kind: sources::Kind) -> bool {
    matches!(kind, sources::Kind::Network | sources::Kind::Disk)
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
mod agent_visibility_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matches_panel_data_rules() {
        assert!(!provider_has_data(&json!({"id":"codex", "ready":true})));
        assert!(!provider_has_data(&json!({"id":"codex", "recentDays":[{}], "totalPrompts":0})));
        for key in ["totalPrompts", "totalSessions", "activeDays", "todayPrompts", "todaySessions"] {
            let mut record = json!({"id":"codex", "ready":false});
            record[key] = json!(1);
            assert!(provider_has_data(&record), "{key}");
        }
        assert!(provider_has_data(&json!({"id":"codex", "limits":[{"percent":0}]})));
        assert!(provider_has_data(&json!({"id":"codex", "balance":{"remaining":0}})));
        assert!(!provider_has_data(&json!({"id":"codex", "balance":{"remaining":-1}})));
        assert!(!provider_has_data(&json!({"limits":[{}]})));
    }

    #[test]
    fn records_appear_disappear_and_reclaim_space() {
        let directory = std::env::temp_dir().join(format!("omarchy-glance-visibility-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("codex.json");
        let mut renderer = Renderer::new(None);
        renderer.config = Rc::new(Config::parse(r#"{"version":1,"layers":{"default":{"right":[
            {"id":"glance.agents","agent":"codex","layout":"stacked"},
            {"id":"glance.esc"}
        ]}}}"#).unwrap());
        renderer.set_geometry(2170, 60);
        renderer.build();
        renderer.poll_usage_at(&directory);
        assert_eq!(renderer.width_of(&renderer.widgets[0]), 0);
        let esc_rect = renderer.widgets[1].rect;
        for record in [json!({"id":"codex","limits":[{"label":"5h window","percent":0}]}),
                       json!({"id":"codex","totalSessions":1})] {
            fs::write(&path, record.to_string()).unwrap();
            renderer.poll_usage_at(&directory);
            assert!(renderer.width_of(&renderer.widgets[0]) > 0);
            assert_eq!(renderer.widgets[1].rect, esc_rect);
            renderer.owner.insert(1, renderer.widgets[0].key.clone());
            fs::remove_file(&path).unwrap();
            renderer.poll_usage_at(&directory);
            assert_eq!(renderer.width_of(&renderer.widgets[0]), 0);
            assert!(renderer.owner.is_empty());
        }
        fs::write(&path, "broken JSON").unwrap();
        renderer.poll_usage_at(&directory);
        assert_eq!(renderer.width_of(&renderer.widgets[0]), 0);
        fs::remove_file(&path).unwrap();
        renderer.poll_usage_at(&directory);
        fs::write(&path, r#"{"id":"codex","ready":true,"limits":[]}"#).unwrap();
        renderer.poll_usage_at(&directory);
        assert_eq!(renderer.width_of(&renderer.widgets[0]), 0);
        fs::remove_dir_all(directory).unwrap();
    }
}
