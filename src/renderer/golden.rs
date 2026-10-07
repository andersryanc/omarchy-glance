//! Golden previews: draw configs through the backend protocol with the fixed
//! provider data in tests/golden/fixture.json (injected on the backend side)
//! and a fixed clock, and compare every pixel with
//! tests/golden/<name>.png. `UPDATE_GOLDEN=1 cargo test golden` rewrites the
//! images; on a mismatch the actual frame is written to target/golden/.
//!
//! Text is drawn with the installed fonts, so the images are only valid on a
//! machine with the same font files (JetBrainsMono Nerd Font).

use super::*;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden");

fn fixture() -> Value {
    let path = format!("{DIR}/fixture.json");
    serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap_or_else(|e| panic!("{path}: {e}"))
}

static SCRATCH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A client drawing `config` from a backend in this process that serves the
/// fixture's data instead of the system's.
fn renderer(config: &str, fn_layer: bool) -> Renderer {
    let data = fixture();
    let n = SCRATCH.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("omarchy-glance-golden-{}-{n}", std::process::id()));
    fs::create_dir_all(dir.join("usage")).unwrap();
    fs::write(dir.join("touchbar.json"), config).unwrap();
    let mut backend = Box::new(Backend::new(Paths { config_dir: dir.clone(), usage_dir: dir.join("usage") }));
    let session = backend.connect();
    let mut r = Renderer::new(Some(Link::Local { backend, session }));
    r.set_geometry(2170, 60);
    r.fn_held = fn_layer;
    r.clock = Some(DateTime::parse_from_rfc3339(data["now"].as_str().unwrap()).unwrap());
    r.request(json!({"type": "hello", "protocol": 1, "output": "touchbar", "client": "golden"}));
    let Some(Link::Local { backend, .. }) = r.link.as_mut() else { unreachable!() };
    backend.inject(&data);
    r.pull();
    fs::remove_dir_all(&dir).unwrap();
    assert!(r.synced, "no snapshot from the backend");
    r
}

/// One frame of `config` with the fixture's data.
fn render(config: &str, fn_layer: bool) -> ImageSurface {
    let r = renderer(config, fn_layer);
    let surface = ImageSurface::create(Format::Rgb24, r.w, r.h).unwrap();
    r.draw(&surface, None);
    surface
}

/// RGB of every pixel (Rgb24 leaves the top byte undefined).
fn pixels(mut surface: ImageSurface) -> (i32, i32, Vec<u32>) {
    let (w, h, stride) = (surface.width(), surface.height(), surface.stride() as usize);
    let data = surface.data().unwrap();
    let mut out = Vec::with_capacity((w * h) as usize);
    for y in 0..h as usize {
        for x in 0..w as usize {
            let p = &data[y * stride + 4 * x..][..4];
            out.push(u32::from_ne_bytes([p[0], p[1], p[2], p[3]]) & 0x00FF_FFFF);
        }
    }
    (w, h, out)
}

fn check(name: &str, config: &str, fn_layer: bool) {
    let surface = render(config, fn_layer);
    let golden = format!("{DIR}/{name}.png");
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        surface.write_to_png(&mut fs::File::create(&golden).unwrap()).unwrap();
        return;
    }
    let actual_dir = concat!(env!("CARGO_MANIFEST_DIR"), "/target/golden");
    let actual = format!("{actual_dir}/{name}.png");
    fs::create_dir_all(actual_dir).unwrap();
    surface.write_to_png(&mut fs::File::create(&actual).unwrap()).unwrap();
    let expected = ImageSurface::create_from_png(&mut fs::File::open(&golden).unwrap_or_else(|e| panic!("{golden}: {e}")))
        .unwrap_or_else(|e| panic!("{golden}: {e}"));
    let (aw, ah, a) = pixels(surface);
    let (ew, eh, e) = pixels(expected);
    assert_eq!((aw, ah), (ew, eh), "{name}: size differs from {golden}");
    let diff: Vec<usize> = (0..a.len()).filter(|&i| a[i] != e[i]).collect();
    if let (Some(first), Some(last)) = (diff.first(), diff.last()) {
        let xs = diff.iter().map(|i| *i as i32 % aw);
        let (x0, x1) = (xs.clone().min().unwrap(), xs.max().unwrap());
        panic!("{name}: {} pixels differ from {golden} (x {x0}..={x1}, rows {}..={}); actual frame in {actual}",
               diff.len(), *first as i32 / aw, *last as i32 / aw);
    }
    fs::remove_file(&actual).unwrap();
}

#[test]
fn golden_default() {
    check("default", DEFAULT_CONFIG, false);
}

#[test]
fn golden_default_fn() {
    check("default-fn", DEFAULT_CONFIG, true);
}

#[test]
fn golden_every_widget() {
    check("every-widget", &fs::read_to_string(format!("{DIR}/every-widget.json")).unwrap(), false);
}

#[test]
fn golden_every_widget_fn() {
    check("every-widget-fn", &fs::read_to_string(format!("{DIR}/every-widget.json")).unwrap(), true);
}

/// Frame times with the fixture data: `cargo test --release bench_frames -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_frames() {
    let surface = ImageSurface::create(Format::Rgb24, 2170, 60).unwrap();
    for (name, config) in [("default", DEFAULT_CONFIG.to_string()),
                           ("every-widget", fs::read_to_string(format!("{DIR}/every-widget.json")).unwrap())] {
        let r = renderer(&config, false);
        let graphs: HashSet<String> = r.widgets.iter()
            .filter(|w| w.kind == WidgetKind::Graph && w.layer == Layer::Default).map(|w| w.key.clone()).collect();
        let time = |only: Option<&HashSet<String>>| {
            let mut ms: Vec<f64> = (0..500).map(|_| {
                let start = std::time::Instant::now();
                r.draw(&surface, only);
                start.elapsed().as_secs_f64() * 1000.0
            }).collect();
            ms.sort_by(f64::total_cmp);
            ms[ms.len() / 2]
        };
        println!("{name}: full frame {:.2} ms, graph redraw ({} graphs) {:.2} ms (median of 500)",
                 time(None), graphs.len(), time(Some(&graphs)));
    }
}
