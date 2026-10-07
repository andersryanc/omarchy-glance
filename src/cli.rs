//! Control subcommands: switch the Touch Bar between this client (with its
//! backend service) and the t1bridge built-in, install the desktop row's
//! Omarchy panel plugin, and manage the user configs.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::config::user_config;
use crate::protocol::Output;

pub const USAGE: &str =
    "usage: omarchy-glance on|off|desktop on|off|restart|status|log|config [desktop] [edit]|backend|touchbar|preview out.png [fn]";

const BACKEND: &str = "omarchy-glance.service";
const SOCKET: &str = "omarchy-glance.socket";
// The backend's user units; `on` installs them with ExecStart pointing at this binary.
const UNITS: [(&str, &str); 2] = [
    (SOCKET, include_str!("../systemd/omarchy-glance.socket")),
    (BACKEND, include_str!("../systemd/omarchy-glance.service")),
];

// The desktop row's panel plugin, compiled in so an installed binary needs no
// checkout. The development hosts (shell.qml, check.qml, shot.qml) stay out.
const PLUGIN_ID: &str = "glance.row";
const PLUGIN_FILES: [(&str, &str); 18] = [
    ("manifest.json", include_str!("../desktop/manifest.json")),
    ("Panel.qml", include_str!("../desktop/Panel.qml")),
    ("glance/AgentsWidget.qml", include_str!("../desktop/glance/AgentsWidget.qml")),
    ("glance/BtopTheme.qml", include_str!("../desktop/glance/BtopTheme.qml")),
    ("glance/ButtonWidget.qml", include_str!("../desktop/glance/ButtonWidget.qml")),
    ("glance/CommandWidget.qml", include_str!("../desktop/glance/CommandWidget.qml")),
    ("glance/Dots.qml", include_str!("../desktop/glance/Dots.qml")),
    ("glance/Face.qml", include_str!("../desktop/glance/Face.qml")),
    ("glance/GlanceClient.qml", include_str!("../desktop/glance/GlanceClient.qml")),
    ("glance/GlanceHost.qml", include_str!("../desktop/glance/GlanceHost.qml")),
    ("glance/GlanceRow.qml", include_str!("../desktop/glance/GlanceRow.qml")),
    ("glance/GraphWidget.qml", include_str!("../desktop/glance/GraphWidget.qml")),
    ("glance/MediaWidget.qml", include_str!("../desktop/glance/MediaWidget.qml")),
    ("glance/Meter.qml", include_str!("../desktop/glance/Meter.qml")),
    ("glance/MicWidget.qml", include_str!("../desktop/glance/MicWidget.qml")),
    ("glance/WidgetView.qml", include_str!("../desktop/glance/WidgetView.qml")),
    ("glance/util.js", include_str!("../desktop/glance/util.js")),
    ("README.md", "Installed by `omarchy-glance desktop on`; `omarchy-glance desktop off` removes it.\n"),
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

fn exe() -> Result<PathBuf, String> {
    std::env::current_exe().and_then(fs::canonicalize).map_err(|e| format!("current executable: {e}"))
}

/// Install the backend's socket and service units for this binary, enable
/// the socket, and restart a running backend so it runs this binary.
fn install_backend(exe: &Path) -> Result<(), String> {
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
    systemctl(&["try-restart", BACKEND]) // a running backend picks up a rebuilt binary
}

/// Install the backend and the t1bridge renderer wrapper, then (re)start the
/// backend and the bar so both run this binary.
fn on() -> Result<(), String> {
    let exe = exe()?;
    install_backend(&exe)?;
    let quoted = format!("'{}'", exe.display());
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

fn plugin_dir() -> PathBuf {
    config_home().join("omarchy/plugins").join(PLUGIN_ID)
}

/// Run an omarchy-shell IPC call; returns what it printed.
fn omarchy_shell(args: &[&str]) -> Result<String, String> {
    let out = Command::new("omarchy-shell").args(args).output().map_err(|e| format!("omarchy-shell: {e}"))?;
    if !out.status.success() {
        return Err(format!("omarchy-shell {} failed", args.join(" ")));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Enable or disable the plugin. Right after a rescan the shell may not know
/// it yet ("unknown"), so retry for a few seconds.
fn set_plugin_enabled(enabled: bool) -> Result<(), String> {
    let flag = if enabled { "true" } else { "false" };
    for _ in 0..30 {
        if omarchy_shell(&["shell", "setPluginEnabled", PLUGIN_ID, flag])? == "ok" {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    Err(format!("omarchy-shell doesn't know the plugin {PLUGIN_ID}"))
}

/// Remove the plugin directory, or a development link in its place.
fn remove_plugin(dir: &Path) -> Result<(), String> {
    let err = |e: std::io::Error| format!("{}: {e}", dir.display());
    match fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => fs::remove_dir_all(dir).map_err(err),
        Ok(_) => fs::remove_file(dir).map_err(err),
        Err(_) => Ok(()),
    }
}

/// Install the backend and the desktop row's panel plugin (no t1bridge
/// needed), and enable it in omarchy-shell. omarchy-shell keeps a loaded
/// plugin's old code, so an update restarts the shell.
fn desktop_on() -> Result<(), String> {
    install_backend(&exe()?)?;
    let dir = plugin_dir();
    let update = fs::symlink_metadata(&dir).is_ok();
    remove_plugin(&dir)?;
    for (name, content) in PLUGIN_FILES {
        install(&dir.join(name), content, 0o644)?;
    }
    omarchy_shell(&["shell", "rescanPlugins"])?;
    set_plugin_enabled(true)?;
    if update {
        let status = Command::new("omarchy-restart-shell").status().map_err(|e| format!("omarchy-restart-shell: {e}"))?;
        if !status.success() {
            return Err("omarchy-restart-shell failed".into());
        }
    }
    println!("desktop row on: {}", dir.display());
    Ok(())
}

/// Remove the desktop row. The backend stays installed for the Touch Bar.
fn desktop_off() -> Result<(), String> {
    if plugin_dir().exists() {
        set_plugin_enabled(false)?;
    }
    remove_plugin(&plugin_dir())?;
    omarchy_shell(&["shell", "rescanPlugins"])?;
    println!("desktop row off");
    Ok(())
}

fn status() -> Result<(), String> {
    match fs::read_to_string(renderer_link()) {
        Ok(script) => println!("selected: {}", script.lines().last().unwrap_or("").trim_start_matches("exec ")),
        Err(_) => println!("selected: built-in"),
    }
    let dir = plugin_dir();
    match fs::read_link(&dir) {
        Ok(target) => println!("desktop row: linked to {}", target.display()),
        Err(_) if dir.is_dir() => println!("desktop row: installed in {}", dir.display()),
        Err(_) => println!("desktop row: not installed"),
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
        "desktop" => match second {
            Some("on") => desktop_on(),
            Some("off") => desktop_off(),
            _ => Err(USAGE.into()),
        },
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every plugin file in desktop/ is compiled in, apart from the
    /// development hosts.
    #[test]
    fn plugin_files_are_complete() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("desktop");
        let mut on_disk = vec![];
        for dir in ["", "glance"] {
            for entry in fs::read_dir(root.join(dir)).unwrap() {
                let path = entry.unwrap().path();
                if path.is_file() {
                    on_disk.push(path.strip_prefix(&root).unwrap().display().to_string());
                }
            }
        }
        on_disk.retain(|f| !["shell.qml", "check.qml", "shot.qml"].contains(&f.as_str()));
        on_disk.sort();
        let mut embedded: Vec<String> = PLUGIN_FILES.iter().map(|(n, _)| n.to_string()).filter(|n| n != "README.md").collect();
        embedded.sort();
        assert_eq!(embedded, on_disk);
    }
}
