//! Backend protocol v1 (docs/backend-protocol.md): names shared by the backend
//! and its clients. Messages are JSON objects, one per line.

use serde_json::Value;

use crate::config::Spec;
use crate::sources;

pub const PROTOCOL: i64 = 1;
pub const MAX_LINE: usize = 1 << 20; // bytes, including the newline

/// Error codes; the fatal ones close the connection.
pub mod error {
    pub const BAD_MESSAGE: &str = "bad_message";
    pub const TOO_LARGE: &str = "too_large";
    pub const UNSUPPORTED_PROTOCOL: &str = "unsupported_protocol";
    pub const UNSUPPORTED_OUTPUT: &str = "unsupported_output";
    pub const NOT_READY: &str = "not_ready";
    pub const UNKNOWN_WIDGET: &str = "unknown_widget";
    pub const STALE_CONFIG: &str = "stale_config";
    pub const NOT_PRESSABLE: &str = "not_pressable";
    pub const HIDDEN: &str = "hidden";
    pub const UNKNOWN_POINTER: &str = "unknown_pointer";
    pub const QUEUE_OVERFLOW: &str = "queue_overflow";

    pub fn fatal(code: &str) -> bool {
        matches!(code, TOO_LARGE | UNSUPPORTED_PROTOCOL | UNSUPPORTED_OUTPUT | QUEUE_OVERFLOW)
    }
}

/// The kind of output a client drives; it picks the config file.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Output {
    Touchbar,
}

impl Output {
    pub fn parse(name: &str) -> Option<Output> {
        match name {
            "touchbar" => Some(Output::Touchbar),
            _ => None, // "desktop" arrives with its config (T08)
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Output::Touchbar => "touchbar",
        }
    }

    pub fn file_name(self) -> &'static str {
        match self {
            Output::Touchbar => "touchbar.json",
        }
    }

    pub fn default_config(self) -> (&'static str, &'static str) {
        match self {
            Output::Touchbar => ("touchbar.default.json", include_str!("../touchbar.default.json")),
        }
    }

    /// Why a widget can't be shown on this output, if it can't.
    pub fn unsupported(self, _kind: WidgetKind, _spec: &Spec) -> Option<&'static str> {
        match self {
            Output::Touchbar => None, // the Touch Bar shows every widget kind
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum WidgetKind {
    Esc,
    Button,
    Command,
    Agents,
    Graph,
    Mic,
    Media,
    Spacer,
}

impl WidgetKind {
    pub fn name(self) -> &'static str {
        match self {
            WidgetKind::Esc => "esc",
            WidgetKind::Button => "button",
            WidgetKind::Command => "command",
            WidgetKind::Agents => "agents",
            WidgetKind::Graph => "graph",
            WidgetKind::Mic => "mic",
            WidgetKind::Media => "media",
            WidgetKind::Spacer => "spacer",
        }
    }
}

/// A widget's kind from its config: `type` for user widgets, else the id.
pub fn kind_of(spec: &Spec) -> Option<WidgetKind> {
    match spec.get("type").and_then(Value::as_str) {
        Some("button") => return Some(WidgetKind::Button),
        Some("command") => return Some(WidgetKind::Command),
        _ => {}
    }
    let id = spec.get("id").and_then(Value::as_str).unwrap_or("");
    if sources::kind_of(id).is_some() {
        return Some(WidgetKind::Graph);
    }
    Some(match id {
        "glance.esc" => WidgetKind::Esc,
        "glance.agents" => WidgetKind::Agents,
        "glance.mic" => WidgetKind::Mic,
        "glance.media" => WidgetKind::Media,
        "glance.spacer" => WidgetKind::Spacer,
        _ => return None,
    })
}
