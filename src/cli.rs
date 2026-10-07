//! Control subcommands: switch the Touch Bar between this renderer (or the
//! Python one with `on python`) and the t1bridge built-in, and manage the
//! user config.

use std::fs;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::Command;

use crate::renderer::{DEFAULT_CONFIG, user_config};

pub const USAGE: &str =
    "usage: omarchy-glance [on [python]|off|restart|status|log|config [edit]|--preview out.png [fn]]";

/// The Python renderer in the checkout this binary was built from; removed in T05.
const PYTHON: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/python/renderer.py");

fn renderer_link() -> PathBuf {
    user_config().parent().unwrap().parent().unwrap().join("t1bridge/renderer")
}

fn systemctl(args: &[&str]) -> Result<(), String> {
    let status = Command::new("systemctl").arg("--user").args(args).status().map_err(|e| format!("systemctl: {e}"))?;
    if status.success() { Ok(()) } else { Err(format!("systemctl --user {} failed", args.join(" "))) }
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
    fs::create_dir_all(link.parent().unwrap()).map_err(|e| format!("{}: {e}", link.display()))?;
    let _ = fs::remove_file(&link);
    std::os::unix::fs::symlink(&target, &link).map_err(|e| format!("{}: {e}", link.display()))?;
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
    match fs::read_link(renderer_link()) {
        Ok(target) => println!("selected: {}", target.display()),
        Err(_) => println!("selected: built-in"),
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
