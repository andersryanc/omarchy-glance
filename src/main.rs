//! omarchy-glance: custom Touch Bar renderer for t1bridge (Touch Bar hardware
//! IPC v1). With no arguments it drives the bar; see cli.rs for the control
//! subcommands.
//!
//! The bar is built from ~/.config/omarchy-glance/touchbar.json (or the
//! built-in touchbar.default.json when there is none) and reloads when that file changes.
//! A config has two layers, "default" and "fn" (shown while Fn is held), each
//! with left/center/right lists of widgets, like the Omarchy bar's shell.json.

mod cli;
mod config;
mod mic;
mod proc;
mod proto;
mod renderer;
mod sources;

use std::sync::OnceLock;
use std::time::Instant;

pub fn log(msg: &str) {
    eprintln!("omarchy-glance: {msg}");
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
        // omarchy-glance --preview out.png [fn]: draw one frame to a PNG, no hardware needed
        Some("--preview") => {
            let path = args.get(1).map_or("preview.png", String::as_str);
            renderer::Renderer::new(None).preview(path, args.get(2).is_some_and(|a| a == "fn"))
        }
        Some(_) => cli::run(&args).unwrap_or_else(|| Err(cli::USAGE.into())),
        None => proto::Conn::connect(proto::SOCK_PATH)
            .map_err(|e| format!("{}: {e}", proto::SOCK_PATH))
            .and_then(|conn| renderer::Renderer::new(Some(conn)).run()),
    };
    if let Err(e) = result {
        log(&e);
        std::process::exit(1);
    }
}
