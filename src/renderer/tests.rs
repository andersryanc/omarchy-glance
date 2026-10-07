//! Client tests against a backend in this process.

use super::*;

struct Client {
    r: Renderer,
    dir: std::path::PathBuf,
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

impl Client {
    fn new(name: &str, layers: Value) -> Client {
        let dir = std::env::temp_dir().join(format!("omarchy-glance-client-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("usage")).unwrap();
        let config = json!({"version": 1, "repeatDelay": 0.4, "repeatInterval": 0.12, "layers": layers});
        fs::write(dir.join("touchbar.json"), config.to_string()).unwrap();
        let mut backend = Box::new(Backend::new(Paths { config_dir: dir.clone(), usage_dir: dir.join("usage") }));
        let session = backend.connect();
        let mut r = Renderer::new(Some(Link::Local { backend, session }));
        r.set_geometry(2170, 60);
        r.request(json!({"type": "hello", "protocol": 1, "output": "touchbar", "client": "test"}));
        r.pull();
        assert!(r.synced);
        Client { r, dir }
    }

    fn backend(&mut self) -> (&mut Backend, u64) {
        let Some(Link::Local { backend, session }) = self.r.link.as_mut() else { unreachable!() };
        (backend, *session)
    }

    /// What the backend has queued for the client, without handling it.
    fn replies(&mut self) -> Vec<Value> {
        let (b, session) = self.backend();
        b.drain(session, now()).iter().map(|l| serde_json::from_str(l).unwrap()).collect()
    }

    /// A Touch Bar input frame with these (contact id, x) touches.
    fn touch(&mut self, contacts: &[(u8, i32)]) {
        let mut p = vec![0u8; 12 + 12 * contacts.len()];
        p[9] = contacts.len() as u8;
        for (i, (cid, x)) in contacts.iter().enumerate() {
            let at = 12 + 12 * i;
            (p[at], p[at + 1]) = (*cid, 1);
            p[at + 4..at + 8].copy_from_slice(&(*x as u32).to_le_bytes());
            p[at + 8..at + 12].copy_from_slice(&30u32.to_le_bytes());
        }
        self.r.on_input(&p);
    }

    fn x_of(&self, key: &str) -> i32 {
        self.r.widget(key).unwrap().rect[0] + 10
    }
}

#[test]
fn touches_become_presses_and_keys_stay_local() {
    let mut c = Client::new("touch", json!({"default": {"left": [
        {"id": "glance.esc"},
        {"id": "up", "type": "button", "label": "+", "exec": "true", "repeat": true},
        {"id": "f1", "type": "button", "label": "F1", "key": "F1", "repeat": true},
        {"id": "label", "type": "button", "label": "nothing"}
    ]}}));
    c.replies(); // the view request's ack

    // Esc and F-keys are tapped here; the backend hears nothing.
    let (esc, f1) = (c.x_of("default.left.0"), c.x_of("default.left.2"));
    c.touch(&[(1, esc), (2, f1)]);
    assert!(c.replies().is_empty());
    assert!(c.r.repeat_at.contains_key(&2) && !c.r.repeat_at.contains_key(&1));
    assert!(c.r.pressed(c.r.widget("default.left.0").unwrap()));
    c.touch(&[]);
    assert!(c.replies().is_empty() && c.r.repeat_at.is_empty() && c.r.owner.is_empty());

    // An exec button is pressed and released in the backend, by contact id.
    c.touch(&[(5, c.x_of("default.left.1"))]);
    let replies = c.replies();
    assert_eq!(replies.len(), 1);
    assert_eq!(replies[0]["type"], "ack", "{replies:?}");
    c.touch(&[]);
    assert_eq!(c.replies()[0]["type"], "ack"); // the release: the backend knew pointer 5

    // A button without an action still shows the pressed face.
    c.touch(&[(6, c.x_of("default.left.3"))]);
    assert!(c.replies().is_empty());
    assert!(c.r.pressed(c.r.widget("default.left.3").unwrap()));
}

#[test]
fn hidden_agents_reclaim_space() {
    let mut c = Client::new("agents", json!({"default": {"right": [
        {"id": "glance.agents", "agent": "nobody-here", "layout": "stacked"},
        {"id": "glance.esc"}
    ]}}));
    let agents = |c: &Client| c.r.width_of(c.r.widget("default.right.0").unwrap());
    assert_eq!(agents(&c), 0);
    let esc_rect = c.r.widget("default.right.1").unwrap().rect;
    let record = c.dir.join("usage/nobody-here.json");
    let mut t = now();
    for content in [r#"{"id":"nobody-here","limits":[{"label":"5h window","percent":0}]}"#,
                    r#"{"id":"nobody-here","totalSessions":1}"#] {
        fs::write(&record, content).unwrap();
        t += 1.5;
        c.backend().0.tick(t);
        std::thread::sleep(std::time::Duration::from_millis(20)); // past the update coalescing
        c.r.pull();
        assert!(agents(&c) > 0);
        assert_eq!(c.r.widget("default.right.1").unwrap().rect, esc_rect);
        c.r.owner.insert(1, "default.right.0".into());
        fs::remove_file(&record).unwrap();
        t += 1.5;
        c.backend().0.tick(t);
        std::thread::sleep(std::time::Duration::from_millis(20));
        c.r.pull();
        assert_eq!(agents(&c), 0);
        assert!(c.r.owner.is_empty());
    }
}

#[test]
fn missed_update_resyncs_and_lost_backend_shows_offline() {
    let mut c = Client::new("resync", json!({"default": {"left": [{"id": "glance.esc"}]}}));
    c.replies();
    let rev = c.r.rev;
    c.r.on_message(&json!({"type": "update", "rev": rev + 2, "widgets": {}}));
    assert!(c.r.resyncing);
    c.r.pull();
    assert!(!c.r.resyncing && c.r.synced);

    c.touch(&[(3, 10)]);
    c.r.lost_backend(now(), "test");
    assert!(c.r.offline && !c.r.synced && c.r.widgets.is_empty());
    assert!(c.r.ignored.contains(&3)); // the finger that was down stays ignored until it lifts
    let surface = ImageSurface::create(Format::Rgb24, 2170, 60).unwrap();
    c.r.draw(&surface, None); // the disconnected state
}
