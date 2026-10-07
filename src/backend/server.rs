//! The backend's Unix socket: accepts clients, frames newline-delimited JSON,
//! and keeps one slow client from holding up the rest.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use super::{Backend, Paths, Token};
use crate::protocol::MAX_LINE;
use crate::{log, now};

const STALE_SECONDS: f64 = 10.0; // drop a client that reads nothing for this long while data waits

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::Relaxed);
}

pub fn socket_path() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })), PathBuf::from);
    runtime.join("omarchy-glance/backend.sock")
}

struct Client {
    stream: UnixStream,
    session: u64,
    rbuf: Vec<u8>,
    wbuf: Vec<u8>,
    stuck_since: Option<f64>, // when unsent data stopped draining
}

pub struct Server {
    listener: UnixListener,
    pub backend: Backend,
    clients: HashMap<RawFd, Client>,
    pub stale_seconds: f64,
}

/// The listening socket from systemd (LISTEN_FDS), if we were socket-activated.
fn activated_listener() -> Option<UnixListener> {
    let pid: u32 = std::env::var("LISTEN_PID").ok()?.parse().ok()?;
    let fds: i32 = std::env::var("LISTEN_FDS").ok()?.parse().ok()?;
    if pid != std::process::id() || fds < 1 {
        return None;
    }
    unsafe {
        std::env::remove_var("LISTEN_PID");
        std::env::remove_var("LISTEN_FDS");
        std::env::remove_var("LISTEN_FDNAMES");
        libc::fcntl(3, libc::F_SETFD, libc::FD_CLOEXEC);
        Some(UnixListener::from_raw_fd(3))
    }
}

/// Bind `path` (0600, in a 0700 directory), unless another backend answers there.
pub fn bind(path: &Path) -> Result<UnixListener, String> {
    let dir = path.parent().unwrap();
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{}: {e}", dir.display()))?;
    if path.exists() {
        if UnixStream::connect(path).is_ok() {
            return Err(format!("another backend is already listening on {}", path.display()));
        }
        let _ = std::fs::remove_file(path); // left over from a backend that died
    }
    let listener = UnixListener::bind(path).map_err(|e| format!("{}: {e}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(listener)
}

fn peer_uid(stream: &UnixStream) -> Option<u32> {
    let mut cred: libc::ucred = unsafe { std::mem::zeroed() };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let r = unsafe {
        libc::getsockopt(stream.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut _, &mut len)
    };
    (r == 0).then_some(cred.uid)
}

/// `omarchy-glance backend`: serve until killed.
pub fn run() -> Result<(), String> {
    let listener = match activated_listener() {
        Some(l) => l,
        None => bind(&socket_path())?,
    };
    let mut server = Server::new(listener, Backend::new(Paths::user()))?;
    // Helpers run in their own sessions, so stop them ourselves on the way out.
    unsafe {
        let handler = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
        libc::signal(libc::SIGTERM, handler);
        libc::signal(libc::SIGINT, handler);
    }
    log("backend ready");
    let result = loop {
        if STOP.load(Ordering::Relaxed) {
            break Ok(());
        }
        if let Err(e) = server.step(f64::INFINITY) {
            break Err(e);
        }
    };
    server.backend.shutdown();
    log("backend stopped");
    result
}

impl Server {
    pub fn new(listener: UnixListener, backend: Backend) -> Result<Server, String> {
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        Ok(Server { listener, backend, clients: HashMap::new(), stale_seconds: STALE_SECONDS })
    }

    #[cfg(test)]
    pub fn client_count(&self) -> usize {
        self.clients.len()
    }

    /// Wait for at most `max_wait` seconds for something to do, and do it.
    pub fn step(&mut self, max_wait: f64) -> Result<(), String> {
        let t = now();
        self.backend.tick(t);
        self.flush_all(t);

        let mut deadline = self.backend.deadline().min(t + max_wait);
        for c in self.clients.values() {
            // An update can't go out until the client takes what it has.
            if c.wbuf.is_empty() && let Some(due) = self.backend.update_due(c.session) {
                deadline = deadline.min(due);
            }
            if let Some(since) = c.stuck_since {
                deadline = deadline.min(since + self.stale_seconds);
            }
        }
        let mut fds: Vec<(RawFd, Option<Token>)> = vec![(self.listener.as_raw_fd(), None)];
        fds.extend(self.clients.keys().map(|fd| (*fd, None)));
        let client_fds = fds.len();
        fds.extend(self.backend.fds().into_iter().map(|(fd, tok)| (fd, Some(tok))));
        let mut pfds: Vec<libc::pollfd> = fds.iter().enumerate().map(|(i, (fd, _))| {
            let mut events = libc::POLLIN;
            if i > 0 && i < client_fds && !self.clients[fd].wbuf.is_empty() {
                events |= libc::POLLOUT;
            }
            libc::pollfd { fd: *fd, events, revents: 0 }
        }).collect();
        let timeout = ((deadline - now()).max(0.0) * 1000.0).ceil().min(f64::from(i32::MAX)) as i32;
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout) };
        if n < 0 {
            let err = io::Error::last_os_error();
            return if err.kind() == io::ErrorKind::Interrupted { Ok(()) } else { Err(format!("poll: {err}")) };
        }
        let t = now();
        for (i, (pfd, (fd, token))) in pfds.iter().zip(fds).enumerate() {
            if pfd.revents == 0 {
                continue;
            }
            if i == 0 {
                self.accept(t);
            } else if let Some(token) = token {
                self.backend.readable(token, t);
            } else if pfd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
                self.read(fd, t);
            }
        }
        self.flush_all(t);
        Ok(())
    }

    fn accept(&mut self, t: f64) {
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => {
                    let me = unsafe { libc::getuid() };
                    if peer_uid(&stream) != Some(me) {
                        log("refused a connection from another user");
                        continue;
                    }
                    if stream.set_nonblocking(true).is_err() {
                        continue;
                    }
                    let session = self.backend.connect();
                    let fd = stream.as_raw_fd();
                    self.clients.insert(fd, Client { stream, session, rbuf: vec![], wbuf: vec![], stuck_since: None });
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    log(&format!("accept: {e}"));
                    break;
                }
            }
        }
        let _ = t;
    }

    fn read(&mut self, fd: RawFd, t: f64) {
        let Some(c) = self.clients.get_mut(&fd) else { return };
        let mut buf = [0u8; 65536];
        let mut eof = false;
        loop {
            match c.stream.read(&mut buf) {
                Ok(0) => {
                    eof = true;
                    break;
                }
                Ok(n) => c.rbuf.extend_from_slice(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => {
                    eof = true;
                    break;
                }
            }
        }
        let session = c.session;
        let mut lines = vec![];
        let mut too_large = false;
        while let Some(end) = c.rbuf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = c.rbuf.drain(..=end).collect();
            if line.len() > MAX_LINE {
                too_large = true;
                break;
            }
            lines.push(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned());
        }
        if c.rbuf.len() >= MAX_LINE {
            too_large = true;
        }
        for line in lines {
            if !line.trim().is_empty() {
                self.backend.handle_line(session, &line, t);
            }
        }
        if too_large {
            self.backend.too_large(session);
        }
        if eof {
            self.drop_client(fd, t);
        }
    }

    fn drop_client(&mut self, fd: RawFd, t: f64) {
        if let Some(c) = self.clients.remove(&fd) {
            self.backend.disconnect(c.session, t);
        }
    }

    /// Move each client's queued lines to its socket as far as it will take them.
    /// A client gets new lines only once its previous ones are all sent, so
    /// updates for a slow client coalesce in the backend instead of piling up.
    fn flush_all(&mut self, t: f64) {
        let fds: Vec<RawFd> = self.clients.keys().copied().collect();
        for fd in fds {
            if !self.flush(fd, t) {
                self.drop_client(fd, t);
            }
        }
    }

    /// False if the client should be dropped.
    fn flush(&mut self, fd: RawFd, t: f64) -> bool {
        let stale = self.stale_seconds;
        let c = self.clients.get_mut(&fd).unwrap();
        loop {
            if c.wbuf.is_empty() {
                for line in self.backend.drain(c.session, t) {
                    c.wbuf.extend_from_slice(line.as_bytes());
                    c.wbuf.push(b'\n');
                }
            }
            if c.wbuf.is_empty() {
                c.stuck_since = None;
                return !self.backend.closing(c.session);
            }
            match c.stream.write(&c.wbuf) {
                Ok(n) if n > 0 => {
                    c.wbuf.drain(..n);
                    c.stuck_since = None;
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    let since = *c.stuck_since.get_or_insert(t);
                    if t - since >= stale {
                        log("dropping a client that stopped reading");
                        return false;
                    }
                    return true; // try again when writable
                }
                _ => return false,
            }
        }
    }
}
