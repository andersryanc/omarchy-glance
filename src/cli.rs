//! Control subcommands: switch the Touch Bar between this client (with its
//! backend service) and the t1bridge built-in, and manage the user config.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::user_config;
use crate::protocol::Output;

pub const USAGE: &str =
    "usage: omarchy-glance on|off|restart|status|log|config [desktop] [edit]|backend|touchbar|preview out.png [fn]";

const BACKEND: &str = "omarchy-glance.service";
const SOCKET: &str = "omarchy-glance.socket";
// The backend's user units; `on` installs them with ExecStart pointing at this binary.
const UNITS: [(&str, &str); 2] = [
    (SOCKET, include_str!("../systemd/omarchy-glance.socket")),
    (BACKEND, include_str!("../systemd/omarchy-glance.service")),
];

fn config_home() -> PathBuf {
    user_config().parent().unwrap().parent().unwrap().to_path_buf()
}

fn renderer_link() -> PathBuf {
    config_home().join("t1bridge/renderer")
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let status = Command::new("systemctl").arg("--user").args(args).status().map_err(|e| format!("systemctl: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("systemctl --user {} failed", args.join(" "))) }
}

/// Replace `path` with `content` (a link or an outdated file is removed
/// first, so a link's target is never written through); true if it changed.
fn install(path: &Path, content: &str, mode: u32) -> Result<bool, String> {
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let is_link = fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink());
    if !is_link && fs::read_to_string(path).is_ok_and(|c| c == content) {
        return Ok(false);
    }
    fs::create_dir_all(path.parent().unwrap()).map_err(err)?;
    let _ = fs::remove_file(path);
    fs::write(path, content).map_err(err)?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(err)?;
    Ok(true)
}

/// Install the backend's units and the t1bridge renderer wrapper, then
/// (re)start the backend and the bar so both run this binary.
fn on() -> Result<(), String> {
    let exe = std::env::current_exe().and_then(fs::canonicalize).map_err(|e| format!("current executable: {e}"))?;
    let quoted = format!("'{}'", exe.display());
    let mut changed = false;
    for (name, unit) in UNITS {
        let unit = unit.replace("%h/.local/bin/omarchy-glance", &quoted);
        changed |= install(&config_home().join("systemd/user").join(name), &unit, 0o644)?;
    }
    if changed {
        systemctl(&["daemon-reload"])?;
    }
    systemctl(&["enable", "--now", SOCKET])?;
    systemctl(&["try-restart", BACKEND])?; // a running backend picks up a rebuilt binary
    // The t1bridge launcher runs its renderer with no arguments.
    let wrapper = format!("#!/bin/sh\n# Installed by `omarchy-glance on`.\nexec {quoted} touchbar\n");
    install(&renderer_link(), &wrapper, 0o755)?;
    systemctl(&["restart", "t1-touchbar"])?;
    println!("omarchy-glance on: {}", exe.display());
    Ok(())
}

/// Back to the built-in bar. The backend stays installed; with no clients it
/// runs nothing.
fn off() -> Result<(), String> {
    let link = renderer_link();
    if let Err(e) = fs::remove_file(&link)
        && e.kind() != std::io::ErrorKind::NotFound {
        return Err(format!("{}: {e}", link.display()));
    }
    systemctl(&["restart", "t1-touchbar"])?;
    println!("built-in renderer on");
    Ok(())
}

fn status() -> Result<(), String> {
    match fs::read_to_string(renderer_link()) {
        Ok(script) => println!("selected: {}", script.lines().last().unwrap_or("").trim_start_matches("exec ")),
        Err(_) => println!("selected: built-in"),
    }
    // Only the process lists; the log is `omarchy-glance log`.
    for unit in ["t1-touchbar", BACKEND] {
        let out = Command::new("systemctl")
            .args(["--user", "--no-pager", "status", unit])
            .output()
            .map_err(|e| format!("systemctl: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        let state = text.lines().find_map(|l| l.trim().strip_prefix("Active: ")).unwrap_or("unknown");
        println!("{unit}: {state}");
        if let Some(start) = text.find("CGroup") {
            print!("{}", &text[start..]);
        }
    }
    Ok(())
}

/// Create an output's user config from its default if needed; the backend
/// reloads it on save.
fn config(output: Output, edit: bool) -> Result<(), String> {
    let path = user_config().with_file_name(output.file_name());
    if !path.exists() {
        let default = output.default_config().1;
        fs::create_dir_all(path.parent().unwrap()).map_err(|e| format!("{}: {e}", path.display()))?;
        fs::write(&path, default).map_err(|e| format!("{}: {e}", path.display()))?;
        println!("created {} from the default", path.display());
    }
    if edit {
        // $EDITOR may carry arguments (Omarchy's is "omarchy-launch-editor --inline").
        let editor = std::env::var("EDITOR").ok().filter(|e| !e.trim().is_empty()).unwrap_or_else(|| "nvim".into());
        let err = Command::new("sh").args(["-c", &format!("exec {editor} \"$1\""), "sh"]).arg(&path).exec();
        return Err(format!("{editor}: {err}"));
    }
    println!("{}", path.display());
    Ok(())
}

/// Runs a control subcommand; None if `args` doesn't name one.
pub fn run(args: &[String]) -> Option<Result<(), String>> {
    let second = args.get(1).map(String::as_str);
    Some(match args.first()?.as_str() {
        "on" => on(),
        "off" => off(),
        "restart" => systemctl(&["try-restart", BACKEND]).and_then(|_| systemctl(&["restart", "t1-touchbar"])),
        "status" => status(),
        "log" => Err(format!(
            "journalctl: {}",
            Command::new("journalctl").args(["--user", "-u", "t1-touchbar", "-u", BACKEND, "-f"]).exec()
        )),
        "config" => {
            let desktop = second == Some("desktop");
            let edit = args.get(if desktop { 2 } else { 1 }).map(String::as_str) == Some("edit");
            config(if desktop { Output::Desktop } else { Output::Touchbar }, edit)
        }
        _ => return None,
    })
}
