//! Custom Touch Bar renderer for t1bridge (Touch Bar hardware IPC v1).
//!
//! The bar is built from ~/.config/touchbar/config.json (or the built-in
//! config.default.json when there is none) and reloads when that file changes.
//! A config has two layers, "default" and "fn" (shown while Fn is held), each
//! with left/center/right lists of widgets, like the Omarchy bar's shell.json.

mod config;
mod mic;
mod proc;
mod proto;
mod renderer;
mod sources;

use std::sync::OnceLock;
use std::time::Instant;

pub fn log(msg: &str) {
    eprintln!("touchbar-renderer: {msg}");
}

/// Seconds on a monotonic clock, like Python's time.monotonic().
pub fn now() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

fn main() {
    now();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        // touchbar --preview out.png [fn]: draw one frame to a PNG, no hardware needed
        Some("--preview") => {
            let path = args.get(1).map_or("preview.png", String::as_str);
            renderer::Renderer::new(None).preview(path, args.get(2).is_some_and(|a| a == "fn"))
        }
        _ => proto::Conn::connect(proto::SOCK_PATH)
            .map_err(|e| format!("{}: {e}", proto::SOCK_PATH))
            .and_then(|conn| renderer::Renderer::new(Some(conn)).run()),
    };
    if let Err(e) = result {
        log(&e);
        std::process::exit(1);
    }
}
