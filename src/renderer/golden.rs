//! Golden previews: draw configs with the fixed provider data in
//! tests/golden/fixture.json and a fixed clock, and compare every pixel with
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

/// A renderer for `config` with the fixture's data instead of the system's.
fn renderer(config: &str, fn_layer: bool) -> Renderer {
    let data = fixture();
    let mut r = Renderer::new(None);
    r.config = Rc::new(Config::parse(config).unwrap());
    r.set_geometry(2170, 60);
    r.fn_held = fn_layer;
    r.clock = Some(DateTime::parse_from_rfc3339(data["now"].as_str().unwrap()).unwrap());
    r.build();

    let keys: Vec<(String, Kind, String)> = r.widgets.iter().map(|w| (w.key.clone(), w.kind, w.id())).collect();
    for (key, kind, id) in keys {
        match kind {
            Kind::Agents => {
                let agent = text(&r.widget(&key).unwrap().spec, "agent", "claude");
                let record = data["usage"].get(&agent).cloned();
                r.set_usage(agent, None, record);
            }
            Kind::Graph => {
                let g = r.graphs.get_mut(&key).unwrap();
                let d = &data["graphs"][&id];
                for (history, series) in g.history.iter_mut().zip(d["history"].as_array().unwrap()) {
                    let values: Vec<f64> = series.as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
                    history.extend(&values[values.len().saturating_sub(g.columns)..]);
                }
                let line = |l: &Value| (l[0].as_str().unwrap().to_string(), l[1].as_bool().unwrap());
                g.source.lines = d["lines"].as_array().unwrap().iter().map(line).collect();
                let spec = &r.widgets.iter().find(|w| w.key == key).unwrap().spec;
                if truthy(spec.get("temperature")) || text(spec, "detail", "none") != "none" {
                    g.source.lines.push(line(&d["detail"]));
                }
                g.source.cores = d["cores"].as_array().map_or(vec![], |c| c.iter().map(|v| v.as_f64().unwrap()).collect());
                g.source.fixed_icon = d["icon"].as_str().map(String::from);
            }
            Kind::Media => r.commands.get_mut(&key).unwrap().media = Some(data["media"].clone()),
            Kind::Command => {
                let state = r.commands.get_mut(&key).unwrap();
                state.text = data["commands"][&id]["text"].as_str().unwrap().to_string();
                state.urgent = data["commands"][&id]["urgent"].as_bool().unwrap_or(false);
            }
            Kind::Mic if r.mic.is_none() => {
                let d = &data["mic"];
                let mut mic = Mic::new(20);
                mic.muted = d["muted"].as_bool();
                mic.in_use = d["inUse"].as_bool().unwrap();
                mic.levels = d["levels"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
                r.mic = Some(mic);
            }
            _ => {}
        }
    }
    r.relayout();
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
            .filter(|w| w.kind == Kind::Graph && w.layer == Layer::Default).map(|w| w.key.clone()).collect();
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
