//! Backend tests: the core API directly, and the socket server end to end.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::server::{Server, bind};
use super::*;
use crate::now;

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let dir = std::env::temp_dir().join(format!("omarchy-glance-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("usage")).unwrap();
        Dir(dir)
    }

    fn write_config(&self, layers: Value) {
        let config = json!({"version": 1, "repeatDelay": 0.4, "repeatInterval": 0.12, "layers": layers});
        fs::write(self.0.join("touchbar.json"), config.to_string()).unwrap();
    }

    fn backend(&self) -> Backend {
        Backend::new(Paths { config_dir: self.0.clone(), usage_dir: self.0.join("usage") })
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn parse(lines: Vec<String>) -> Vec<Value> {
    lines.iter().map(|l| serde_json::from_str(l).unwrap()).collect()
}

/// Connect a session and say hello; returns it and the generation.
fn hello(b: &mut Backend) -> (u64, u64) {
    let id = b.connect();
    b.handle_line(id, r#"{"type":"hello","id":1,"protocol":1,"output":"touchbar","client":"test"}"#, now());
    let msgs = parse(b.drain(id, now()));
    assert_eq!(msgs[0], json!({"type":"ack","id":1}));
    assert_eq!(msgs[1]["type"], "welcome");
    assert_eq!(msgs[2]["type"], "snapshot");
    (id, msgs[2]["config"]["generation"].as_u64().unwrap())
}

fn request(b: &mut Backend, id: u64, msg: Value) -> Value {
    b.handle_line(id, &msg.to_string(), now());
    let mut msgs = parse(b.drain(id, now()));
    msgs.retain(|m| m["type"] != "update");
    assert_eq!(msgs.len(), 1, "{msgs:?}");
    msgs.remove(0)
}

/// Run the backend's own loop (ticks and child output) until `until` holds.
fn pump(b: &mut Backend, mut until: impl FnMut(&mut Backend) -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        b.tick(now());
        for (fd, token) in b.fds() {
            let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
            if unsafe { libc::poll(&mut pfd, 1, 0) } > 0 {
                b.readable(token, now());
            }
        }
        if until(b) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out");
}

fn wait_for_lines(path: &PathBuf, n: usize) -> usize {
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        let count = fs::read_to_string(path).map_or(0, |s| s.lines().count());
        if count >= n || Instant::now() > end {
            std::thread::sleep(Duration::from_millis(50)); // let stragglers land
            return fs::read_to_string(path).map_or(0, |s| s.lines().count());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn snapshot_then_updates() {
    let dir = Dir::new("snapshot");
    dir.write_config(json!({"default": {"left": [
        {"id": "glance.esc"},
        {"id": "up", "type": "command", "exec": "echo hi", "onTap": "true"},
        {"id": "glance.spacer"}
    ]}}));
    let mut b = dir.backend();
    let id = b.connect();
    b.handle_line(id, r#"{"type":"hello","id":1,"protocol":1,"output":"touchbar"}"#, now());
    let msgs = parse(b.drain(id, now()));
    let snapshot = &msgs[2];
    assert_eq!(snapshot["rev"], 1);
    assert_eq!(snapshot["config"]["generation"], 1);
    let widgets = snapshot["widgets"].as_array().unwrap();
    assert_eq!(widgets.len(), 3);
    assert_eq!(widgets[0]["action"], json!({"key": "esc", "repeat": false}));
    assert_eq!(widgets[1]["key"], "default.left.1");
    assert_eq!(widgets[1]["options"].get("exec"), None, "commands never reach clients");
    assert_eq!(widgets[1]["action"], json!({"press": true, "repeat": false}));
    assert_eq!(widgets[2]["action"], Value::Null);

    let mut update = Value::Null;
    pump(&mut b, |b| {
        if let Some(u) = parse(b.drain(id, now())).into_iter().find(|m| m["type"] == "update") {
            update = u;
        }
        !update.is_null()
    });
    assert_eq!(update["rev"], 2);
    assert_eq!(update["widgets"]["default.left.1"], json!({"text": "hi", "urgent": false}));
}

#[test]
fn last_disconnect_stops_providers() {
    let dir = Dir::new("demand");
    dir.write_config(json!({"default": {"left": [
        {"id": "slow", "type": "command", "exec": "sleep 30"},
        {"id": "glance.memory"}
    ]}}));
    let mut b = dir.backend();
    let (a, _) = hello(&mut b);
    let (c, _) = hello(&mut b);
    b.tick(now());
    assert_eq!(b.providers.len(), 2);
    assert_eq!(b.fds().len(), 1, "the command is running");
    b.disconnect(a, now());
    assert_eq!(b.providers.len(), 2, "another client still needs them");
    b.disconnect(c, now());
    assert!(b.providers.is_empty());
    assert!(b.fds().is_empty(), "the command was stopped");
    assert!(b.outputs.is_empty());
}

#[test]
fn malformed_requests() {
    let dir = Dir::new("malformed");
    dir.write_config(json!({"default": {"left": [
        {"id": "glance.spacer"},
        {"id": "f1", "type": "button", "key": "F1"}
    ]}}));
    let mut b = dir.backend();
    let id = b.connect();
    let code = |m: &Value| m["code"].as_str().unwrap().to_string();
    for (line, expected) in [
        ("not json", "bad_message"),
        ("[1,2]", "bad_message"),
        (r#"{"id":1}"#, "bad_message"),
        (r#"{"type":"view"}"#, "bad_message"),
        (r#"{"type":"press","id":2,"widget":"default.left.0","pointer":1}"#, "not_ready"),
    ] {
        b.handle_line(id, line, now());
        let msgs = parse(b.drain(id, now()));
        assert_eq!(code(&msgs[0]), expected, "{line}");
    }
    assert!(!b.closing(id));

    for hello in [r#"{"type":"hello","id":1,"protocol":2,"output":"touchbar"}"#,
                  r#"{"type":"hello","id":1,"protocol":1,"output":"desktop"}"#] {
        let other = b.connect();
        b.handle_line(other, hello, now());
        let msgs = parse(b.drain(other, now()));
        assert!(["unsupported_protocol", "unsupported_output"].contains(&code(&msgs[0]).as_str()));
        assert!(b.closing(other), "fatal");
    }

    let (id, generation) = hello(&mut b);
    let press = |widget: &str, generation: u64| json!({"type":"press","id":5,"generation":generation,"widget":widget,"pointer":1});
    assert_eq!(code(&request(&mut b, id, press("default.left.9", generation))), "unknown_widget");
    assert_eq!(code(&request(&mut b, id, press("default.left.0", generation + 1))), "stale_config");
    assert_eq!(code(&request(&mut b, id, press("default.left.0", generation))), "not_pressable");
    assert_eq!(code(&request(&mut b, id, press("default.left.1", generation))), "not_pressable");
    assert_eq!(code(&request(&mut b, id, json!({"type":"release","id":6,"pointer":7}))), "unknown_pointer");
    assert_eq!(code(&request(&mut b, id, json!({"type":"view","id":7,"layer":"top","shown":true}))), "bad_message");
    assert_eq!(code(&request(&mut b, id, json!({"type":"frobnicate","id":8}))), "bad_message");
    assert_eq!(request(&mut b, id, json!({"type":"view","id":9,"layer":"fn","shown":true})), json!({"type":"ack","id":9}));
    assert!(!b.closing(id));
}

#[test]
fn hold_to_repeat_and_cancel() {
    let dir = Dir::new("repeat");
    let log = dir.0.join("presses");
    dir.write_config(json!({"default": {"left": [
        {"id": "up", "type": "button", "exec": format!("echo x >> {}", log.display()), "repeat": true}
    ]}}));
    let mut b = dir.backend();
    let (id, generation) = hello(&mut b);
    let t0 = now();
    b.handle_line(id, &json!({"type":"press","id":2,"generation":generation,"widget":"default.left.0","pointer":3}).to_string(), t0);
    b.tick(t0 + 0.39);
    b.tick(t0 + 0.40); // first repeat
    b.tick(t0 + 0.53); // second (0.4 + 0.12 can round past 0.52)
    b.handle_line(id, r#"{"type":"release","id":3,"pointer":3}"#, t0 + 0.6);
    b.tick(t0 + 1.0);
    assert_eq!(wait_for_lines(&log, 3), 3);
    let replies = parse(b.drain(id, t0 + 1.0));
    assert!(replies.contains(&json!({"type":"ack","id":2})) && replies.contains(&json!({"type":"ack","id":3})));

    // A client that disappears mid-press leaves nothing repeating.
    b.handle_line(id, &json!({"type":"press","id":4,"generation":generation,"widget":"default.left.0","pointer":3}).to_string(), t0 + 2.0);
    b.tick(t0 + 2.4);
    b.disconnect(id, t0 + 2.45);
    b.tick(t0 + 3.0);
    assert_eq!(wait_for_lines(&log, 6), 5);
}

#[test]
fn config_reload_sends_snapshot_and_ends_presses() {
    let dir = Dir::new("reload");
    let log = dir.0.join("presses");
    let button = json!({"id": "up", "type": "button", "exec": format!("echo x >> {}", log.display()), "repeat": true});
    dir.write_config(json!({"default": {"left": [button]}}));
    let mut b = dir.backend();
    let (id, generation) = hello(&mut b);
    let t0 = now();
    b.handle_line(id, &json!({"type":"press","id":2,"generation":generation,"widget":"default.left.0","pointer":1}).to_string(), t0);
    let _ = b.drain(id, t0);
    std::thread::sleep(Duration::from_millis(20));
    dir.write_config(json!({"default": {"left": [button, {"id": "glance.spacer"}]}}));
    b.tick(t0 + 1.5); // config poll
    let msgs = parse(b.drain(id, t0 + 1.5));
    let snapshot = msgs.iter().find(|m| m["type"] == "snapshot").expect("snapshot after reload");
    assert_eq!(snapshot["config"]["generation"], generation + 1);
    assert_eq!(snapshot["widgets"].as_array().unwrap().len(), 2);
    b.tick(t0 + 3.0);
    assert_eq!(wait_for_lines(&log, 2), 1, "no repeats after the reload");

    // An invalid file keeps the previous config and reports the error.
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dir.0.join("touchbar.json"), "{ nope").unwrap();
    b.tick(t0 + 4.5);
    b.handle_line(id, r#"{"type":"resync","id":3}"#, t0 + 4.5);
    let msgs = parse(b.drain(id, t0 + 4.5));
    let snapshot = msgs.iter().find(|m| m["type"] == "snapshot").unwrap();
    assert_eq!(snapshot["config"]["generation"], generation + 1);
    assert!(snapshot["config"]["error"].is_string());
    assert_eq!(snapshot["widgets"].as_array().unwrap().len(), 2);
}

#[test]
fn unread_replies_overflow() {
    let dir = Dir::new("overflow");
    dir.write_config(json!({"default": {"left": [{"id": "glance.spacer"}]}}));
    let mut b = dir.backend();
    let (id, _) = hello(&mut b);
    for i in 0..300 {
        b.handle_line(id, &format!(r#"{{"type":"resync","id":{}}}"#, i + 10), now());
    }
    assert!(b.closing(id));
    let msgs = parse(b.drain(id, now()));
    assert_eq!(msgs.iter().filter(|m| m["type"] == "snapshot").count(), 1, "snapshots coalesce");
    assert_eq!(msgs.last().unwrap()["code"], "queue_overflow");
}

#[test]
fn hidden_agents_widgets() {
    let dir = Dir::new("agents");
    dir.write_config(json!({"default": {"left": [{"id": "glance.agents", "agent": "nobody-here"}]}}));
    let mut b = dir.backend();
    let (id, generation) = hello(&mut b);
    b.tick(now());
    b.handle_line(id, r#"{"type":"resync","id":9}"#, now());
    let msgs = parse(b.drain(id, now()));
    let state = &msgs.iter().find(|m| m["type"] == "snapshot").unwrap()["widgets"][0]["state"];
    assert_eq!(state["visible"], false);
    assert_eq!(state["limits"][0]["fraction"], Value::Null);
    let press = json!({"type":"press","id":2,"generation":generation,"widget":"default.left.0","pointer":1});
    assert_eq!(request(&mut b, id, press)["code"], "hidden");

    fs::write(dir.0.join("usage/nobody-here.json"),
              r#"{"id":"nobody-here","limits":[{"label":"Session (5h)","percent":0.25,"resetsAt":"2026-10-06T14:00:00-07:00"}]}"#).unwrap();
    b.tick(now() + 1.5);
    let msgs = parse(b.drain(id, now() + 1.5));
    let state = &msgs.iter().find(|m| m["type"] == "update").unwrap()["widgets"]["default.left.0"];
    assert_eq!(state["visible"], true);
    assert_eq!(state["limits"], json!([{"label":"Session","fraction":0.25,"resetsAt":"2026-10-06T14:00:00-07:00"}]));
}

// --- over the socket -----------------------------------------------------------

struct TestServer {
    server: Server,
    path: PathBuf,
    _dir: Dir,
}

impl TestServer {
    fn new(name: &str, layers: Value) -> TestServer {
        let dir = Dir::new(name);
        dir.write_config(layers);
        let path = dir.0.join("backend.sock");
        let server = Server::new(bind(&path).unwrap(), dir.backend()).unwrap();
        TestServer { server, path, _dir: dir }
    }

    fn client(&self) -> (UnixStream, BufReader<UnixStream>) {
        let s = UnixStream::connect(&self.path).unwrap();
        s.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let r = BufReader::new(s.try_clone().unwrap());
        (s, r)
    }

    /// Step the server until the client has read a message matching `want`.
    fn expect(&mut self, r: &mut BufReader<UnixStream>, want: impl Fn(&Value) -> bool) -> Value {
        let end = Instant::now() + Duration::from_secs(3);
        let mut line = String::new();
        while Instant::now() < end {
            self.server.step(0.01).unwrap();
            match r.read_line(&mut line) {
                Ok(0) => panic!("connection closed"),
                Ok(_) if line.ends_with('\n') => {
                    let msg: Value = serde_json::from_str(&line).unwrap();
                    line.clear();
                    if want(&msg) {
                        return msg;
                    }
                }
                _ => {}
            }
        }
        panic!("timed out");
    }
}

const HELLO: &[u8] = b"{\"type\":\"hello\",\"id\":1,\"protocol\":1,\"output\":\"touchbar\"}\n";

#[test]
fn socket_session_and_reconnect() {
    let mut ts = TestServer::new("socket", json!({"default": {"left": [{"id": "glance.spacer"}]}}));
    let (mut s, mut r) = ts.client();
    s.write_all(HELLO).unwrap();
    let welcome = ts.expect(&mut r, |m| m["type"] == "welcome");
    let snapshot = ts.expect(&mut r, |m| m["type"] == "snapshot");
    assert_eq!(snapshot["rev"], 1);
    // Two requests in one write, and one split across writes.
    s.write_all(b"{\"type\":\"resync\",\"id\":2}\n{\"type\":\"view\",\"id\":3,").unwrap();
    ts.expect(&mut r, |m| m["id"] == 2);
    s.write_all(b"\"layer\":\"fn\",\"shown\":true}\n").unwrap();
    ts.expect(&mut r, |m| m["id"] == 3 && m["type"] == "ack");
    drop((s, r));
    for _ in 0..10 {
        ts.server.step(0.01).unwrap();
    }
    assert_eq!(ts.server.client_count(), 0);
    assert!(ts.server.backend.sessions.is_empty());

    let (mut s, mut r) = ts.client();
    s.write_all(HELLO).unwrap();
    let again = ts.expect(&mut r, |m| m["type"] == "welcome");
    assert_ne!(again["session"], welcome["session"]);
    assert_eq!(ts.expect(&mut r, |m| m["type"] == "snapshot")["rev"], 1);
}

#[test]
fn too_large_line_closes() {
    let mut ts = TestServer::new("large", json!({"default": {"left": []}}));
    let (mut s, mut r) = ts.client();
    s.set_nonblocking(true).unwrap();
    let big = vec![b'x'; crate::protocol::MAX_LINE + 10];
    let mut sent = 0;
    while sent < big.len() {
        ts.server.step(0.01).unwrap();
        match s.write(&big[sent..]) {
            Ok(n) => sent += n,
            Err(_) => {}
        }
    }
    s.set_nonblocking(false).unwrap();
    let err = ts.expect(&mut r, |m| m["type"] == "error");
    assert_eq!(err["code"], "too_large");
    for _ in 0..5 {
        ts.server.step(0.01).unwrap();
    }
    assert_eq!(ts.server.client_count(), 0);
}

#[test]
fn stuck_client_is_dropped_without_holding_up_others() {
    // A big snapshot fills the socket buffers quickly.
    let spacers: Vec<Value> = (0..400).map(|i| json!({"id": "glance.spacer", "size": i, "note": "x".repeat(100)})).collect();
    let mut ts = TestServer::new("stuck", json!({"default": {"left": spacers}}));
    ts.server.stale_seconds = 0.3;
    let (mut stuck, _never_read) = ts.client();
    stuck.write_all(HELLO).unwrap();
    stuck.set_nonblocking(true).unwrap();
    let (mut ok, mut r) = ts.client();
    ok.write_all(HELLO).unwrap();
    ts.expect(&mut r, |m| m["type"] == "snapshot");
    let start = Instant::now();
    while ts.server.client_count() > 1 && start.elapsed() < Duration::from_secs(3) {
        // Each resync is a fresh snapshot once the previous one went out, so
        // the stuck client's buffers fill up.
        let _ = stuck.write_all(b"{\"type\":\"resync\",\"id\":10}\n");
        ts.server.step(0.05).unwrap();
        ok.write_all(b"{\"type\":\"view\",\"id\":5,\"layer\":\"default\",\"shown\":true}\n").unwrap();
        ts.expect(&mut r, |m| m["id"] == 5);
    }
    assert_eq!(ts.server.client_count(), 1, "the stuck client was dropped");
    assert_eq!(ts.server.backend.sessions.len(), 1);
}
