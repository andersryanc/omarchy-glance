//! Child processes: shell commands, helpers like pactl, and their cleanup.

use std::io::{self, Read};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

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

pub fn kill(mut child: Child) {
    let _ = child.kill();
    let _ = child.wait();
}

/// Wait up to `timeout` seconds for a child to exit, then kill it.
pub fn reap(mut child: Child, timeout: f64) {
    let until = Instant::now() + Duration::from_secs_f64(timeout);
    while Instant::now() < until {
        if !matches!(child.try_wait(), Ok(None)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    kill(child);
}

/// Run a program to completion and return its stdout; an error if it fails or
/// takes longer than `timeout` seconds.
pub fn run_output(args: &[&str], timeout: f64) -> Result<Vec<u8>, String> {
    let mut child = popen(args, true).map_err(|e| format!("{}: {e}", args[0]))?;
    let until = Instant::now() + Duration::from_secs_f64(timeout);
    let mut out = vec![];
    let mut stdout = child.stdout.take().unwrap();
    let mut buf = [0u8; 65536];
    loop {
        let left = until.saturating_duration_since(Instant::now()).as_millis() as i32;
        let mut pfd = libc::pollfd { fd: stdout.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        if left == 0 || unsafe { libc::poll(&mut pfd, 1, left) } == 0 {
            kill(child);
            return Err(format!("{} timed out", args[0]));
        }
        match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => {
                kill(child);
                return Err(e.to_string());
            }
        }
    }
    match child.wait() {
        Ok(s) if s.success() => Ok(out),
        Ok(s) => Err(format!("{} exited with {s}", args[0])),
        Err(e) => Err(e.to_string()),
    }
}
