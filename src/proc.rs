//! Child processes: shell commands, helpers like pactl, and their cleanup.

use std::io;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};

const OMARCHY_BIN: &str = "/usr/share/omarchy/bin";

fn command(args: &[&str], piped: bool) -> Command {
    let mut cmd = Command::new(args[0]);
    cmd.args(&args[1..]);
    // In case the service started before the session environment was imported.
    if std::env::var_os("OMARCHY_PATH").is_none() {
        cmd.env("OMARCHY_PATH", "/usr/share/omarchy");
    }
    let path = std::env::var("PATH").unwrap_or_else(|_| "/usr/bin".into());
    if !path.split(':').any(|p| p == OMARCHY_BIN) {
        cmd.env("PATH", format!("{OMARCHY_BIN}:{path}"));
    }
    cmd.stdin(Stdio::null())
        .stdout(if piped { Stdio::piped() } else { Stdio::null() })
        .stderr(Stdio::null());
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid(); // its own session, like start_new_session=True
            Ok(())
        });
    }
    cmd
}

/// Start a program, with its stdout piped to us if `piped`.
pub fn popen(args: &[&str], piped: bool) -> io::Result<Child> {
    command(args, piped).spawn()
}

/// Start `bash -c cmd`.
pub fn shell(cmd: &str, piped: bool) -> io::Result<Child> {
    popen(&["bash", "-c", cmd], piped)
}

/// Kill a child's whole process group (its session, from setsid), so a
/// shell's own children go with it. Launched apps are never killed.
fn kill_group(child: &Child) {
    unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
}

pub fn kill(mut child: Child) {
    kill_group(&child);
    let _ = child.wait();
}
