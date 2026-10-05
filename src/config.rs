//! The JSON config (same format as the Python renderer; see README.md) and
//! helpers for reading loosely typed widget options.

use serde_json::{Map, Value};

pub type Spec = Map<String, Value>;
pub type Rgb = [f64; 3]; // 0-255 per channel

pub const SECTIONS: [&str; 3] = ["left", "center", "right"];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Layer {
    Default,
    Fn,
}

pub fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(a)) => !a.is_empty(),
        Some(Value::Object(o)) => !o.is_empty(),
    }
}

/// A numeric option, or `default` when it's missing or not a number.
pub fn num(spec: &Spec, key: &str, default: f64) -> f64 {
    match spec.get(key) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(default),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(default),
        Some(Value::Bool(b)) => f64::from(u8::from(*b)),
        _ => default,
    }
}

/// An integer option (truncated, like Python's int()).
pub fn int(spec: &Spec, key: &str, default: i64) -> i64 {
    num(spec, key, default as f64) as i64
}

/// A string option: missing -> `default`, null -> "", numbers etc. stringified.
pub fn text(spec: &Spec, key: &str, default: &str) -> String {
    match spec.get(key) {
        None => default.to_string(),
        Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
    }
}

pub fn hex_rgb(value: Option<&Value>, fallback: Rgb) -> Rgb {
    let Some(s) = value.and_then(Value::as_str) else { return fallback };
    let v = s.trim_start_matches('#');
    if v.len() != 6 || !v.is_ascii() {
        return fallback;
    }
    let mut out = [0.0; 3];
    for (i, o) in out.iter_mut().enumerate() {
        match u8::from_str_radix(&v[2 * i..2 * i + 2], 16) {
            Ok(c) => *o = f64::from(c),
            Err(_) => return fallback,
        }
    }
    out
}

pub fn rgb(r: u8, g: u8, b: u8) -> Rgb {
    [f64::from(r), f64::from(g), f64::from(b)]
}

/// Colour at t (0-1) along a list of RGB stops.
pub fn lerp_rgb(stops: &[Rgb], t: f64) -> Rgb {
    if stops.len() == 1 {
        return stops[0];
    }
    let pos = t.clamp(0.0, 1.0) * (stops.len() - 1) as f64;
    let i = (pos as usize).min(stops.len() - 2);
    let f = pos - i as f64;
    let (a, b) = (stops[i], stops[i + 1]);
    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
}

pub struct Config {
    pub debug_background: bool,
    pub test_pattern: bool,
    pub border: bool,
    pub background: Rgb,
    pub debug_bg: Rgb,
    pub debug_bg_fn: Rgb,
    pub key: Rgb,
    pub key_pressed: Rgb,
    pub text: Rgb,
    pub urgent: Rgb,
    pub font: String,
    pub idle_dim: f64,
    pub repeat_delay: f64,
    pub repeat_interval: f64,
    /// (layer, section, widget specs) in layer then section order.
    pub layers: Vec<(Layer, &'static str, Vec<Spec>)>,
    pub has_fn: bool,
}

impl Config {
    pub fn parse(source: &str) -> Result<Config, String> {
        let data: Value = serde_json::from_str(source).map_err(|e| e.to_string())?;
        let Some(data) = data.as_object().filter(|d| d.get("version").and_then(Value::as_i64) == Some(1)) else {
            return Err(r#"expected an object with "version": 1"#.into());
        };
        let empty = Spec::new();
        let obj = |k: &str| data.get(k).and_then(Value::as_object).unwrap_or(&empty);
        let (debug, colors) = (obj("debug"), obj("colors"));
        let layers = data.get("layers").and_then(Value::as_object);
        let Some(layers) = layers.filter(|l| l.get("default").is_some_and(Value::is_object)) else {
            return Err(r#""layers" needs at least a "default" layer"#.into());
        };
        let mut out = Vec::new();
        for (layer, name) in [(Layer::Default, "default"), (Layer::Fn, "fn")] {
            let layer_obj = match layers.get(name) {
                None | Some(Value::Null) => &empty,
                Some(Value::Object(o)) => o,
                Some(_) => return Err(format!(r#"layer "{name}" must be an object"#)),
            };
            for section in SECTIONS {
                let items = match layer_obj.get(section) {
                    None | Some(Value::Null) => vec![],
                    Some(Value::Array(a)) if a.iter().all(Value::is_object) => {
                        a.iter().map(|i| i.as_object().unwrap().clone()).collect()
                    }
                    Some(_) => return Err(format!(r#""layers.{name}.{section}" must be a list of objects"#)),
                };
                out.push((layer, section, items));
            }
        }
        let has_fn = out.iter().any(|(l, _, items)| *l == Layer::Fn && !items.is_empty());
        let font = text(data, "font", "");
        Ok(Config {
            debug_background: truthy(debug.get("background")),
            test_pattern: truthy(debug.get("testPattern")),
            border: truthy(debug.get("border")),
            background: hex_rgb(colors.get("background"), rgb(0, 0, 0)),
            debug_bg: hex_rgb(colors.get("debugBackground"), rgb(0x10, 0x60, 0x90)),
            debug_bg_fn: hex_rgb(colors.get("debugBackgroundFn"), rgb(0x60, 0x20, 0x90)),
            key: hex_rgb(colors.get("key"), rgb(0x30, 0x30, 0x30)),
            key_pressed: hex_rgb(colors.get("keyPressed"), rgb(0x80, 0x80, 0x80)),
            text: hex_rgb(colors.get("text"), rgb(0xFF, 0xFF, 0xFF)),
            urgent: hex_rgb(colors.get("urgent"), rgb(0xE0, 0x5A, 0x5A)),
            font: if font.is_empty() { "JetBrainsMono Nerd Font".into() } else { font },
            idle_dim: num(data, "idleDimSeconds", 0.0),
            repeat_delay: num(data, "repeatDelay", 0.4),
            repeat_interval: num(data, "repeatInterval", 0.12).max(0.03),
            layers: out,
            has_fn,
        })
    }
}
