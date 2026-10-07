//! Control subcommands: switch the Touch Bar between this client (or the
//! Python renderer with `on python`) and the t1bridge built-in, and manage the
//! user config.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use crate::config::{DEFAULT_CONFIG, user_config};

pub const USAGE: &str =
    "usage: omarchy-glance on [python]|off|restart|status|log|config [edit]|backend|touchbar|preview out.png [fn]";

/// The Python renderer in the checkout this binary was built from; removed in T05.
const PYTHON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/python/renderer.py");

fn renderer_link() -> PathBuf {
    user_config().parent().unwrap().parent().unwrap().join("t1bridge/renderer")
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let status = Command::new("systemctl").arg("--user").args(args).status().map_err(|e| format!("systemctl: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("systemctl --user {} failed", args.join(" "))) }
}

/// The t1bridge launcher runs its renderer with no arguments; this runs ours
/// in Touch Bar client mode.
fn wrapper(exe: &std::path::Path) -> String {
    format!("#!/bin/sh\n# Installed by `omarchy-glance on`.\nexec '{}' touchbar\n", exe.display())
}

fn on(python: bool) -> Result<(), String> {
    let target = if python {
        PathBuf::from(PYTHON)
    } else {
        std::env::current_exe().and_then(fs::canonicalize).map_err(|e| format!("current executable: {e}"))?
    };
    if !target.exists() {
        return Err(format!("{} not found", target.display()));
    }
    let link = renderer_link();
    let err = |e: std::io::Error| format!("{}: {e}", link.display());
    fs::create_dir_all(link.parent().unwrap()).map_err(err)?;
    let _ = fs::remove_file(&link);
    if python {
        std::os::unix::fs::symlink(&target, &link).map_err(err)?;
    } else {
        fs::write(&link, wrapper(&target)).map_err(err)?;
        fs::set_permissions(&link, fs::Permissions::from_mode(0o755)).map_err(err)?;
    }
    systemctl(&["restart", "t1-touchbar"])?;
    println!("custom renderer on: {}", target.display());
    Ok(())
}

fn off() -> Result<(), String> {
    let link = renderer_link();
    if let Err(e) = fs::remove_file(&link) {
        if e.kind() != std::io::ErrorKind::NotFound {
            return Err(format!("{}: {e}", link.display()));
        }
    }
    systemctl(&["restart", "t1-touchbar"])?;
    println!("built-in renderer on");
    Ok(())
}

fn status() -> Result<(), String> {
    let link = renderer_link();
    match (fs::read_link(&link), fs::read_to_string(&link)) {
        (Ok(target), _) => println!("selected: {}", target.display()),
        (_, Ok(script)) => println!("selected: {}", script.lines().last().unwrap_or("").trim_start_matches("exec ")),
        _ => println!("selected: built-in"),
    }
    // Only the process list; the log is `omarchy-glance log`.
    let out = Command::new("systemctl")
        .args(["--user", "--no-pager", "status", "t1-touchbar"])
        .output()
        .map_err(|e| format!("systemctl: {e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    if let Some(start) = text.find("CGroup") {
        print!("{}", &text[start..]);
    }
    Ok(())
}

/// Create the user config from the default if needed; the renderer reloads it on save.
fn config(edit: bool) -> Result<(), String> {
    let path = user_config();
    if !path.exists() {
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| format!("{}: {e}", path.display()))?;
        fs::write(&path, DEFAULT_CONFIG).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("created {} from the default", path.display());
    }
    if edit {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nvim".into());
        return Err(format!("{editor}: {}", Command::new(&editor).arg(&path).exec()));
    }
    println!("{}", path.display());
    Ok(())
}

/// Runs a control subcommand; None if `args` doesn't name one.
pub fn run(args: &[String]) -> Option<Result<(), String>> {
    let second = args.get(1).map(String::as_str);
    Some(match args.first()?.as_str() {
        "on" => on(second == Some("python")),
        "off" => off(),
        "restart" => systemctl(&["restart", "t1-touchbar"]),
        "status" => status(),
        "log" => Err(format!(
            "journalctl: {}",
            Command::new("journalctl").args(["--user", "-u", "t1-touchbar", "-f"]).exec()
        )),
        "config" => config(second == Some("edit")),
        _ => return None,
    })
}
