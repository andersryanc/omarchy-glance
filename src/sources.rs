//! Data sources for the graph widgets (touchbar.cpu, touchbar.network, ...).
//!
//! Each source reads the system once per sample. `sample()` returns one raw
//! value per series (or None while it has nothing to show yet) and sets
//! `lines`, the text shown after the graph as [(text, urgent)]. The renderer
//! keeps the history and divides by `scale()` to get the 0-1 height it draws.

use std::collections::VecDeque;
use std::fs;
use std::path::Path;
use std::time::Instant;

use crate::config::{Spec, int, num, text, truthy};

// Fallback gradients when btop's theme doesn't have one (btop's default theme).
const GREEN_RED: &[&str] = &["#77ca9b", "#cbc06c", "#dc4c4c"];
const BLUE_RED: &[&str] = &["#4897d4", "#a77fd4", "#dc4c4c"];
const DOWNLOAD: &[&str] = &["#291f75", "#4f43a3", "#b0a9de"];
const UPLOAD: &[&str] = &["#620665", "#7d4180", "#dcafde"];
const TEMP: &[&str] = &["#4897d4", "#5474e8", "#ff40b6"];
const RED_GREEN: &[&str] = &["#dc4c4c", "#cbc06c", "#77ca9b"];

pub type Lines = Vec<(String, bool)>;
type Res<T> = Result<T, String>;

fn read_str(path: impl AsRef<Path>) -> Res<String> {
    let p = path.as_ref();
    fs::read_to_string(p).map(|s| s.trim().to_string()).map_err(|e| format!("{}: {e}", p.display()))
}

fn read_int(path: impl AsRef<Path>) -> Res<i64> {
    let p = path.as_ref();
    let s = read_str(p)?;
    s.parse().map_err(|_| format!("{}: not a number: {s:?}", p.display()))
}

/// Sorted paths matching `<dir>/<prefix>*<rest>`, like a one-level glob.
fn glob(dir: &str, prefix: &str, rest: &str) -> Vec<String> {
    let mut found: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with(prefix))
        .map(|n| format!("{dir}/{n}{rest}"))
        .filter(|p| rest.is_empty() || Path::new(p).exists())
        .collect();
    found.sort();
    found
}

/// Bytes/s, short: 512B 40K 1.2M 120M 1.5G.
pub fn human_rate(mut n: f64) -> String {
    for unit in ["B", "K", "M", "G"] {
        if n < 1000.0 || unit == "G" {
            return if n > 0.0 && n < 10.0 && unit != "B" { format!("{n:.1}{unit}") } else { format!("{n:.0}{unit}") };
        }
        n /= 1024.0;
    }
    unreachable!()
}

fn hwmon_by_name(name: &str) -> Option<String> {
    glob("/sys/class/hwmon", "hwmon", "")
        .into_iter()
        .find(|d| read_str(format!("{d}/name")).is_ok_and(|n| n == name))
}

fn pyround(v: f64) -> i64 {
    v.round_ties_even() as i64
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Cpu,
    Memory,
    Gpu,
    Network,
    Disk,
    Battery,
    Fan,
}

pub fn kind_of(id: &str) -> Option<Kind> {
    Some(match id {
        "touchbar.cpu" => Kind::Cpu,
        "touchbar.memory" => Kind::Memory,
        "touchbar.gpu" => Kind::Gpu,
        "touchbar.network" => Kind::Network,
        "touchbar.disk" => Kind::Disk,
        "touchbar.battery" => Kind::Battery,
        "touchbar.fan" => Kind::Fan,
        _ => return None,
    })
}

pub struct Source {
    pub kind: Kind,
    spec: Spec,
    pub lines: Lines,
    pub cores: Vec<f64>,                   // cpu: per-core fractions, for "cores": true
    prev_times: Option<Vec<(i64, i64)>>,   // cpu
    prev_counts: Option<(Instant, Vec<i64>)>, // network, disk
    temp_path: Option<String>,
    device: Option<String>, // gpu device dir, disk name, battery dir, fan input
}

impl Source {
    pub fn new(kind: Kind, spec: &Spec) -> Source {
        Source {
            kind,
            spec: spec.clone(),
            lines: vec![],
            cores: vec![],
            prev_times: None,
            prev_counts: None,
            temp_path: None,
            device: None,
        }
    }

    /// Draw the latest value as a level meter, not a history.
    pub fn meter(&self) -> bool {
        self.kind == Kind::Battery && text(&self.spec, "graph", "level") == "level"
    }

    /// Two series: [0] grows up from the middle, [1] down.
    pub fn mirrored(&self) -> bool {
        matches!(self.kind, Kind::Network | Kind::Disk)
    }

    fn battery_power(&self) -> bool {
        text(&self.spec, "graph", "") == "power"
    }

    /// Per series: btop theme gradient name, fallback stops.
    pub fn gradients(&self) -> Vec<(&'static str, &'static [&'static str])> {
        match self.kind {
            Kind::Cpu | Kind::Gpu => vec![("cpu", GREEN_RED)],
            Kind::Memory => vec![("used", BLUE_RED)],
            Kind::Network | Kind::Disk => vec![("download", DOWNLOAD), ("upload", UPLOAD)],
            Kind::Fan => vec![("temp", TEMP)],
            Kind::Battery if self.meter() => vec![("battery", RED_GREEN)], // btop has none
            Kind::Battery if self.battery_power() => vec![("temp", TEMP)],
            Kind::Battery => vec![("cpu", GREEN_RED)],
        }
    }

    fn default_label(&self) -> &'static str {
        match self.kind {
            Kind::Cpu => "cpu",
            Kind::Memory => "mem",
            Kind::Gpu => "gpu",
            Kind::Network => "net",
            Kind::Disk => "disk",
            Kind::Fan => "fan",
            Kind::Battery => "",
        }
    }

    pub fn label_text(&self) -> String {
        if self.kind == Kind::Battery && !self.spec.contains_key("label") {
            return self.battery_icon();
        }
        text(&self.spec, "label", self.default_label())
    }

    /// Longest value lines, for sizing the widget so it doesn't jump.
    pub fn widest(&self) -> Vec<String> {
        let v = |items: &[&str]| items.iter().map(|s| s.to_string()).collect();
        match self.kind {
            Kind::Cpu | Kind::Gpu if truthy(self.spec.get("temperature")) => v(&["100%", "100°"]),
            Kind::Disk if text(&self.spec, "show", "") == "usage" => v(&["100%"]),
            Kind::Network => v(&["↓999.9M"]),
            Kind::Disk => v(&["R 999.9M"]),
            Kind::Battery if text(&self.spec, "detail", "none") != "none" => v(&["100%", "10:00"]),
            Kind::Fan => v(&["9999"]),
            _ => v(&["100%"]),
        }
    }

    pub fn scale(&self, history: &VecDeque<f64>) -> f64 {
        let peak = |floor: f64| history.iter().copied().fold(floor, f64::max);
        match self.kind {
            // Auto-scale to the busiest recent sample, like btop.
            Kind::Network | Kind::Disk => peak(num(&self.spec, "minScale", 10240.0)),
            Kind::Battery if !self.battery_power() => 1.0,
            Kind::Battery => match num(&self.spec, "maxPower", 0.0) {
                m if m != 0.0 => m,
                _ => peak(10.0),
            },
            Kind::Fan => self
                .device
                .as_ref()
                .and_then(|p| read_int(p.replace("_input", "_max")).ok())
                .map_or_else(|| peak(1000.0), |m| m as f64),
            _ => 1.0,
        }
    }

    fn percent_line(&self, frac: f64) -> (String, bool) {
        (format!("{}%", pyround(frac * 100.0)), frac >= num(&self.spec, "alarm", 0.9))
    }

    pub fn sample(&mut self) -> Res<Option<Vec<f64>>> {
        match self.kind {
            Kind::Cpu => self.sample_cpu(),
            Kind::Memory => {
                let info = fs::read_to_string("/proc/meminfo").map_err(|e| e.to_string())?;
                let field = |k: &str| -> Res<f64> {
                    info.lines()
                        .find_map(|l| l.strip_prefix(k)?.strip_prefix(':'))
                        .and_then(|r| r.split_whitespace().next()?.parse().ok())
                        .ok_or_else(|| k.to_string())
                };
                let frac = 1.0 - field("MemAvailable")? / field("MemTotal")?; // as btop and free count it
                self.lines = vec![self.percent_line(frac)];
                Ok(Some(vec![frac]))
            }
            Kind::Gpu => {
                let frac = read_int(format!("{}/gpu_busy_percent", self.gpu_device()?))? as f64 / 100.0;
                self.lines = vec![self.percent_line(frac)];
                if truthy(self.spec.get("temperature")) {
                    let t = self.temperature_line()?;
                    self.lines.push(t);
                }
                Ok(Some(vec![frac]))
            }
            Kind::Network | Kind::Disk => self.sample_rates(),
            Kind::Battery => self.sample_battery(),
            Kind::Fan => {
                let rpm = read_int(self.fan_input()?)?;
                self.lines = vec![(rpm.to_string(), false)];
                Ok(Some(vec![rpm as f64]))
            }
        }
    }

    // --- temperature ("temperature": true on cpu and gpu) --------------------
    fn temperature_line(&mut self) -> Res<(String, bool)> {
        if self.temp_path.is_none() {
            let name = text(&self.spec, "sensor", "");
            let dir = if !name.is_empty() {
                hwmon_by_name(&name)
            } else if self.kind == Kind::Gpu {
                glob(&format!("{}/hwmon", self.gpu_device()?), "hwmon", "").into_iter().next()
            } else {
                hwmon_by_name("coretemp")
            };
            let shown = if name.is_empty() { "coretemp".to_string() } else { name };
            let dir = dir.ok_or_else(|| format!("no hwmon sensor named {shown:?}"))?;
            self.temp_path = Some(format!("{dir}/temp1_input"));
        }
        let t = read_int(self.temp_path.as_ref().unwrap())? as f64 / 1000.0;
        Ok((format!("{}°", pyround(t)), t >= num(&self.spec, "temperatureAlarm", 90.0)))
    }

    // --- cpu ----------------------------------------------------------------
    fn sample_cpu(&mut self) -> Res<Option<Vec<f64>>> {
        // [(busy, total)] jiffies for all CPUs, then each core.
        let stat = fs::read_to_string("/proc/stat").map_err(|e| e.to_string())?;
        let mut times = vec![];
        for line in stat.lines().take_while(|l| l.starts_with("cpu")) {
            let v: Vec<i64> = line.split_whitespace().skip(1).map(|n| n.parse().unwrap_or(0)).collect();
            if v.len() < 8 {
                return Err("short /proc/stat line".into());
            }
            let idle = v[3] + v[4]; // idle + iowait
            let total: i64 = v[..8].iter().sum(); // guest time is already in user/nice
            times.push((total - idle, total));
        }
        let Some(prev) = self.prev_times.replace(times.clone()) else {
            return Ok(None); // usage needs two readings
        };
        let fracs: Vec<f64> = times
            .iter()
            .zip(&prev)
            .map(|((b, t), (pb, pt))| if t > pt { (b - pb) as f64 / (t - pt) as f64 } else { 0.0 })
            .collect();
        self.cores = fracs[1..].to_vec();
        self.lines = vec![self.percent_line(fracs[0])];
        if truthy(self.spec.get("temperature")) {
            let t = self.temperature_line()?;
            self.lines.push(t);
        }
        Ok(Some(vec![fracs[0]]))
    }

    // --- gpu ----------------------------------------------------------------
    fn gpu_device(&mut self) -> Res<String> {
        if self.device.is_none() {
            let card = text(&self.spec, "card", "");
            let paths = if card.is_empty() {
                glob("/sys/class/drm", "card", "/device")
            } else {
                vec![format!("/sys/class/drm/{card}/device")]
            };
            let found = paths.into_iter().find(|p| Path::new(&format!("{p}/gpu_busy_percent")).exists());
            self.device = Some(found.ok_or("no GPU with gpu_busy_percent (amdgpu) found")?);
        }
        Ok(self.device.clone().unwrap())
    }

    // --- network and disk: two byte counters shown as rates ------------------
    fn counters(&mut self) -> Res<Vec<i64>> {
        if self.kind == Kind::Network {
            let mut iface = text(&self.spec, "interface", "");
            if iface.is_empty() {
                let route = fs::read_to_string("/proc/net/route").map_err(|e| e.to_string())?;
                iface = route
                    .lines()
                    .skip(1)
                    .map(|l| l.split_whitespace().collect::<Vec<_>>())
                    .find(|f| f.get(1) == Some(&"00000000"))
                    .map(|f| f[0].to_string())
                    .ok_or("no default route")?;
            }
            let stats = format!("/sys/class/net/{iface}/statistics");
            return Ok(vec![read_int(format!("{stats}/rx_bytes"))?, read_int(format!("{stats}/tx_bytes"))?]);
        }
        if self.device.is_none() {
            let mut dev = text(&self.spec, "device", "");
            if dev.is_empty() {
                let skip = ["loop", "ram", "zram", "dm-", "md", "sr"];
                let mut disks: Vec<String> = fs::read_dir("/sys/block")
                    .map_err(|e| e.to_string())?
                    .flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|d| !skip.iter().any(|s| d.starts_with(s)))
                    .collect();
                disks.sort();
                dev = disks.into_iter().next().ok_or("no disk found in /sys/block")?;
            }
            self.device = Some(dev);
        }
        let dev = self.device.as_ref().unwrap();
        let stats = fs::read_to_string("/proc/diskstats").map_err(|e| e.to_string())?;
        for line in stats.lines() {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.len() > 9 && f[2] == dev {
                let n = |i: usize| f[i].parse::<i64>().unwrap_or(0) * 512; // sectors read, written
                return Ok(vec![n(5), n(9)]);
            }
        }
        Err(format!("no disk {dev:?} in /proc/diskstats"))
    }

    fn sample_rates(&mut self) -> Res<Option<Vec<f64>>> {
        let (now, counts) = (Instant::now(), self.counters()?);
        let prev = self.prev_counts.replace((now, counts.clone()));
        if self.kind == Kind::Disk && text(&self.spec, "show", "") == "usage" {
            self.lines = vec![self.percent_line(disk_usage(&text(&self.spec, "mount", "/"))?)];
        }
        let Some((then, before)) = prev.filter(|(_, b)| b.len() == counts.len()) else {
            return Ok(None);
        };
        let dt = now.duration_since(then).as_secs_f64().max(1e-3);
        let rates: Vec<f64> = counts.iter().zip(&before).map(|(c, p)| (c - p).max(0) as f64 / dt).collect();
        if !(self.kind == Kind::Disk && text(&self.spec, "show", "") == "usage") {
            let arrows = if self.kind == Kind::Network { ["↓", "↑"] } else { ["R ", "W "] };
            self.lines = arrows.iter().zip(&rates).map(|(a, r)| (format!("{a}{}", human_rate(*r)), false)).collect();
        }
        Ok(Some(rates))
    }

    // --- battery ------------------------------------------------------------
    fn battery_dir(&mut self) -> Res<String> {
        if self.device.is_none() {
            let name = text(&self.spec, "battery", "");
            let found = if name.is_empty() {
                glob("/sys/class/power_supply", "BAT", "")
            } else {
                vec![format!("/sys/class/power_supply/{name}")]
            };
            let dir = found.into_iter().next().filter(|d| Path::new(d).exists()).ok_or("no battery found")?;
            self.device = Some(dir);
        }
        Ok(self.device.clone().unwrap())
    }

    fn bat(&self, name: &str) -> Res<String> {
        let dir = self.device.as_ref().ok_or("no battery found")?;
        read_str(format!("{dir}/{name}"))
    }

    fn bat_int(&self, name: &str) -> Res<i64> {
        let s = self.bat(name)?;
        s.parse().map_err(|_| format!("{name}: not a number"))
    }

    /// Percent of what the battery holds now, as UPower and the Omarchy bar
    /// report it. Some drivers' `capacity` (e.g. this Mac's) is relative to the
    /// design capacity instead, which reads low on a worn battery.
    fn charge(&self) -> Res<i64> {
        for (now, full) in [("charge_now", "charge_full"), ("energy_now", "energy_full")] {
            if let (Ok(n), Ok(f)) = (self.bat_int(now), self.bat_int(full)) && f != 0 {
                return Ok(pyround(100.0 * n as f64 / f as f64).min(100));
            }
        }
        self.bat_int("capacity")
    }

    fn battery_icon(&self) -> String {
        let mut me = Source::new(self.kind, &self.spec);
        me.device = self.device.clone();
        let state = me.battery_dir().and_then(|_| Ok((me.charge()?, me.bat("status")?)));
        let code = match state {
            Err(_) => 0xF0091,                         // battery unknown
            Ok((_, s)) if s == "Charging" => 0xF0084,  // battery charging
            Ok((c, _)) if c >= 95 => 0xF0079,          // battery full
            Ok((c, _)) => 0xF007A + ((c as f64 / 10.0).round_ties_even() as i64 - 1).clamp(0, 8) as u32, // 10% .. 90%
        };
        char::from_u32(code).map(String::from).unwrap_or_default()
    }

    fn sample_battery(&mut self) -> Res<Option<Vec<f64>>> {
        self.battery_dir()?;
        let (capacity, status) = (self.charge()?, self.bat("status")?);
        let (current, volts) = match (self.bat_int("current_now"), self.bat_int("voltage_now")) {
            (Ok(c), Ok(v)) => (c as f64 / 1e6, v as f64 / 1e6), // A, V
            _ => (self.bat_int("power_now")? as f64 / 1e6, 1.0), // power_now is in µW
        };
        let watts = current * volts;
        let low = status == "Discharging" && capacity <= int(&self.spec, "low", 15);
        self.lines = vec![(format!("{capacity}%"), low)];
        match text(&self.spec, "detail", "none").as_str() {
            "power" => self.lines.push((format!("{watts:.1}W"), false)),
            "time" => {
                let t = self.time_left(&status, current)?;
                self.lines.push((t, false));
            }
            _ => {}
        }
        Ok(Some(vec![if self.battery_power() { watts } else { capacity as f64 / 100.0 }]))
    }

    /// h:mm until empty (discharging) or full (charging), from charge or energy.
    fn time_left(&self, status: &str, current: f64) -> Res<String> {
        if current <= 0.0 || (status != "Charging" && status != "Discharging") {
            return Ok(if status == "Full" { "full" } else { "—" }.into());
        }
        let (now, full, rate) = match (self.bat_int("charge_now"), self.bat_int("charge_full")) {
            (Ok(n), Ok(f)) => (n as f64, f as f64, current * 1e6),
            _ => (self.bat_int("energy_now")? as f64, self.bat_int("energy_full")? as f64,
                  self.bat_int("power_now")? as f64),
        };
        let hours = if status == "Discharging" { now } else { full - now } / rate;
        Ok(format!("{}:{:02}", hours as i64, (hours * 60.0) as i64 % 60))
    }

    // --- fan ----------------------------------------------------------------
    fn fan_input(&mut self) -> Res<String> {
        if self.device.is_none() {
            let n = int(&self.spec, "fan", 1);
            let mut found = glob("/sys/class/hwmon", "hwmon", &format!("/fan{n}_input"));
            found.extend(glob("/sys/class/hwmon", "hwmon", &format!("/device/fan{n}_input")));
            found.sort();
            self.device = Some(found.into_iter().next().ok_or_else(|| format!("no fan{n}_input in /sys/class/hwmon"))?);
        }
        Ok(self.device.clone().unwrap())
    }
}

/// Fraction of the filesystem at `mount` in use, like shutil.disk_usage.
fn disk_usage(mount: &str) -> Res<f64> {
    let path = std::ffi::CString::new(mount).map_err(|e| e.to_string())?;
    let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut st) } != 0 {
        return Err(format!("{mount}: {}", std::io::Error::last_os_error()));
    }
    let total = st.f_blocks as f64 * st.f_frsize as f64;
    let used = (st.f_blocks - st.f_bfree) as f64 * st.f_frsize as f64;
    if total == 0.0 { Err(format!("{mount}: empty filesystem")) } else { Ok(used / total) }
}
