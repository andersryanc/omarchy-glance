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
    let (id, snapshot) = hello_as(b, "touchbar");
    (id, snapshot["config"]["generation"].as_u64().unwrap())
}

/// Connect a session for `output`; returns it and its first snapshot.
fn hello_as(b: &mut Backend, output: &str) -> (u64, Value) {
    let id = b.connect();
    b.handle_line(id, &json!({"type":"hello","id":1,"protocol":1,"output":output,"client":"test"}).to_string(), now());
    let mut msgs = parse(b.drain(id, now()));
    assert_eq!(msgs[0], json!({"type":"ack","id":1}));
    assert_eq!(msgs[1]["type"], "welcome");
    assert_eq!(msgs[2]["type"], "snapshot");
    (id, msgs.remove(2))
}

fn code(m: &Value) -> String {
    m["code"].as_str().unwrap().to_string()
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

/// Wait for a command to write its background child's pid; then whether it lives.
fn child_pid(pidfile: &PathBuf) -> libc::pid_t {
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(pid) = fs::read_to_string(pidfile).ok().and_then(|s| s.trim().parse().ok()) {
            return pid;
        }
        assert!(Instant::now() < end, "the command never started");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn alive(pid: libc::pid_t) -> bool {
    // Orphaned by the killed shell, it may take a moment to be reaped.
    for _ in 0..100 {
        if unsafe { libc::kill(pid, 0) } != 0 {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    true
}

#[test]
fn stopping_a_command_ends_its_children() {
    let dir = Dir::new("tree");
    let pidfile = dir.0.join("pid");
    let exec = format!("sleep 30 & echo $! > {}; wait", pidfile.display());

    // The last client leaves.
    dir.write_config(json!({"default": {"left": [{"id": "tree", "type": "command", "exec": exec}]}}));
    let mut b = dir.backend();
    let (a, _) = hello(&mut b);
    b.tick(now());
    let pid = child_pid(&pidfile);
    b.disconnect(a, now());
    assert!(!alive(pid), "the command's child outlived it");

    // The command times out.
    fs::remove_file(&pidfile).unwrap();
    let mut job = providers::Job::new(&exec, 0.0);
    let t = now();
    job.tick(t, "test");
    let pid = child_pid(&pidfile);
    assert!(job.tick(t + providers::COMMAND_TIMEOUT + 1.0, "test").is_some());
    assert!(!alive(pid), "the timed-out command's child lives on");
}

/// Wait for a job's fd and handle it, as the loop would.
fn job_readable(job: &mut providers::Job) -> Option<String> {
    let mut pfd = libc::pollfd { fd: job.fd().expect("an fd to wait on"), events: libc::POLLIN, revents: 0 };
    assert_eq!(unsafe { libc::poll(&mut pfd, 1, 2000) }, 1, "nothing to read");
    job.readable()
}

#[test]
fn command_that_closes_its_output_early_does_not_block() {
    let mut job = providers::Job::new("echo hi; exec 1>&-; sleep 0.3", 0.0);
    job.tick(now(), "test");
    let start = Instant::now();
    let mut out = None;
    while out.is_none() {
        out = job_readable(&mut job);
    }
    assert_eq!(out.as_deref(), Some("hi"));
    assert!(start.elapsed() < Duration::from_millis(150), "waited for the script: {:?}", start.elapsed());
    assert!(job.running(), "it runs on, so no second run starts");
    assert_eq!(job_readable(&mut job), None); // its exit
    assert!(!job.running() && job.fd().is_none());
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
                  r#"{"type":"hello","id":1,"protocol":1,"output":"wall"}"#] {
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
fn closing_session_does_nothing_more() {
    let dir = Dir::new("closing");
    let log = dir.0.join("presses");
    dir.write_config(json!({"default": {"left": [
        {"id": "up", "type": "button", "exec": format!("echo x >> {}", log.display()), "repeat": true}
    ]}}));
    let mut b = dir.backend();
    let (id, generation) = hello(&mut b);
    let press = |req: u64, pointer: u64| json!({"type":"press","id":req,"generation":generation,"widget":"default.left.0","pointer":pointer}).to_string();
    let t0 = now();
    b.handle_line(id, &press(2, 1), t0);
    b.too_large(id); // fatal, while pointer 1 is held
    b.handle_line(id, &press(3, 2), t0);
    b.tick(t0 + 1.0); // past the repeat delay
    assert_eq!(wait_for_lines(&log, 2), 1, "only the press before the error ran");
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

    // An invalid file keeps the previous config and reports the error in a
    // snapshot of its own, without a resync.
    std::thread::sleep(Duration::from_millis(20));
    fs::write(dir.0.join("touchbar.json"), "{ nope").unwrap();
    b.tick(t0 + 4.5);
    let msgs = parse(b.drain(id, t0 + 4.5));
    let snapshot = msgs.iter().find(|m| m["type"] == "snapshot").unwrap();
    assert_eq!(snapshot["config"]["generation"], generation + 1);
    assert!(snapshot["config"]["error"].is_string());
    assert_eq!(snapshot["widgets"].as_array().unwrap().len(), 2);
}

#[test]
fn shared_mic_and_media_follow_config_changes() {
    let dir = Dir::new("shared-options");
    let config = |fps: i64, interval: f64| json!({"default": {"left": [
        {"id": "glance.mic", "fps": fps}, {"id": "glance.media", "interval": interval}
    ]}});
    dir.write_config(config(10, 30.0));
    let mut b = dir.backend();
    hello(&mut b);
    let options = |b: &Backend| match (&b.providers[&ProviderKey::Mic], &b.providers[&ProviderKey::Media]) {
        (Provider::Mic(_, fps), Provider::Media(m)) => (*fps, m.status.interval()),
        _ => unreachable!(),
    };
    assert_eq!(options(&b), (10, 30.0));

    // A reload changes them in place.
    dir.write_config(config(40, 5.0));
    b.load_config(Output::Touchbar);
    b.sync_providers(now());
    assert_eq!(options(&b), (40, 5.0));

    // Two outputs: the higher fps and the shorter interval win, whichever said hello first.
    fs::write(dir.0.join("desktop.json"), json!({"version": 1, "left": [
        {"id": "glance.mic", "fps": 60}, {"id": "glance.media", "interval": 10}
    ]}).to_string()).unwrap();
    hello_as(&mut b, "desktop");
    assert_eq!(options(&b), (60, 5.0));
}

#[test]
fn desktop_default_and_settings() {
    let dir = Dir::new("desktop-default");
    let mut b = dir.backend();
    let (_, snapshot) = hello_as(&mut b, "desktop");
    assert_eq!(snapshot["config"]["path"], "<built-in desktop.default.json>");
    assert_eq!(snapshot["settings"]["colors"], json!({}));
    assert_eq!(snapshot["settings"]["monitors"], json!([]));
    assert_eq!(snapshot["settings"]["height"], Value::Null);
    let kinds: Vec<&str> = snapshot["widgets"].as_array().unwrap().iter().map(|w| w["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["media", "graph", "graph", "agents"]);

    fs::write(dir.0.join("desktop.json"), json!({
        "version": 1, "colors": {"background": "#102030"}, "font": "Iosevka", "monitor": "DP-1", "height": 32,
        "left": [{"id": "glance.esc"}, {"id": "f1", "type": "button", "key": "f1"},
                 {"id": "go", "type": "button", "label": "go", "exec": "true"}],
    }).to_string()).unwrap();
    let mut b = dir.backend();
    let (id, snapshot) = hello_as(&mut b, "desktop");
    let settings = &snapshot["settings"];
    assert_eq!((&settings["colors"], &settings["font"], &settings["monitors"], &settings["height"]),
               (&json!({"background": "#102030"}), &json!("Iosevka"), &json!(["DP-1"]), &json!(32.0)));
    let widgets = snapshot["widgets"].as_array().unwrap();
    for w in &widgets[..2] {
        assert_eq!((&w["supported"], &w["reason"], &w["action"]), (&json!(false), &json!("Touch Bar only"), &Value::Null));
    }
    assert_eq!(widgets[2]["key"], "default.left.2");
    assert_eq!(widgets[2]["action"], json!({"press": true, "repeat": false}));
    let generation = snapshot["config"]["generation"].as_u64().unwrap();
    let press = json!({"type":"press","id":2,"generation":generation,"widget":"default.left.0","pointer":1});
    assert_eq!(code(&request(&mut b, id, press)), "not_pressable");
}

#[test]
fn invalid_desktop_config_leaves_the_touch_bar_alone() {
    let dir = Dir::new("desktop-invalid");
    dir.write_config(json!({"default": {"left": [{"id": "glance.spacer"}]}}));
    for (bad, why) in [(json!({"version": 1, "layers": {"default": {}}}), "has no layers"),
                       (json!({"version": 1, "colors": {"key": "#000000"}}), "isn't a desktop colour"),
                       (json!({"version": 1, "colors": {"background": "red"}}), "must be"),
                       (json!({"version": 1, "monitor": 3}), "monitor"),
                       (json!({"version": 1, "height": 1000}), "height")] {
        fs::write(dir.0.join("desktop.json"), bad.to_string()).unwrap();
        let mut b = dir.backend();
        let (tb, tb_generation) = hello(&mut b);
        let (_, snapshot) = hello_as(&mut b, "desktop");
        let error = snapshot["config"]["error"].as_str().unwrap();
        assert!(error.contains(why), "{error}");
        assert_eq!(snapshot["config"]["path"], "<built-in desktop.default.json>");
        // The Touch Bar's session sees nothing of it.
        assert!(parse(b.drain(tb, now())).iter().all(|m| m["type"] != "snapshot"));
        b.handle_line(tb, r#"{"type":"resync","id":3}"#, now());
        let msgs = parse(b.drain(tb, now()));
        let tb_snapshot = msgs.iter().find(|m| m["type"] == "snapshot").unwrap();
        assert_eq!(tb_snapshot["config"], json!({"generation": tb_generation, "path": dir.0.join("touchbar.json").display().to_string(), "error": null}));
    }
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

#[test]
fn stuck_client_with_a_due_update_does_not_spin() {
    let spacers: Vec<Value> = (0..400).map(|i| json!({"id": "glance.spacer", "size": i, "note": "x".repeat(100)})).collect();
    let mut ts = TestServer::new("spin", json!({"default": {"left": spacers}}));
    ts.server.stale_seconds = 10.0;
    let (mut stuck, _never_read) = ts.client();
    stuck.write_all(HELLO).unwrap();
    stuck.set_nonblocking(true).unwrap();
    // Resync until a snapshot waits behind a full socket.
    let start = Instant::now();
    while ts.server.backend.sessions.values().all(|s| s.out.is_empty()) {
        assert!(start.elapsed() < Duration::from_secs(3), "the socket never filled");
        let _ = stuck.write_all(b"{\"type\":\"resync\",\"id\":10}\n");
        ts.server.step(0.01).unwrap();
    }
    for s in ts.server.backend.sessions.values_mut() {
        s.dirty.insert("default.left.0".into());
        s.last_update = 0.0;
    }
    let start = Instant::now();
    for _ in 0..5 {
        ts.server.step(0.05).unwrap();
    }
    assert!(start.elapsed() >= Duration::from_millis(200), "polled without waiting: {:?}", start.elapsed());
}

#[test]
fn many_short_lines_are_cheap() {
    let mut ts = TestServer::new("short-lines", json!({"default": {"left": []}}));
    let (mut s, mut r) = ts.client();
    s.write_all(HELLO).unwrap();
    ts.expect(&mut r, |m| m["type"] == "snapshot");
    let writer = std::thread::spawn(move || {
        s.write_all(&vec![b'\n'; 4 << 20]).unwrap(); // blank lines, which get no reply
        s.write_all(b"{\"type\":\"resync\",\"id\":9}\n").unwrap();
        s
    });
    let start = Instant::now();
    ts.expect(&mut r, |m| m["id"] == 9);
    assert!(start.elapsed() < Duration::from_secs(2), "{:?}", start.elapsed());
    drop(writer.join());
}
