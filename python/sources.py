"""Data sources for the graph widgets (glance.cpu, glance.network, ...).

Each source reads the system once per sample. `sample()` returns one raw value
per series (or None while it has nothing to show yet) and sets `lines`, the
text shown after the graph as [(text, urgent)]. The renderer keeps the history
and divides by `scale()` to get the 0-1 height it draws.
"""

import glob
import os
import shutil
import time

# Fallback gradients when btop's theme doesn't have one (btop's default theme).
GREEN_RED = ["#77ca9b", "#cbc06c", "#dc4c4c"]
BLUE_RED = ["#4897d4", "#a77fd4", "#dc4c4c"]
DOWNLOAD = ["#291f75", "#4f43a3", "#b0a9de"]
UPLOAD = ["#620665", "#7d4180", "#dcafde"]
TEMP = ["#4897d4", "#5474e8", "#ff40b6"]
RED_GREEN = ["#dc4c4c", "#cbc06c", "#77ca9b"]


def read_int(path):
    with open(path) as f:
        return int(f.read().strip())


def human_rate(n):
    """Bytes/s, short: 512B 40K 1.2M 120M 1.5G."""
    for unit in ("B", "K", "M", "G"):
        if n < 1000 or unit == "G":
            return f"{n:.1f}{unit}" if 0 < n < 10 and unit != "B" else f"{n:.0f}{unit}"
        n /= 1024


def hwmon_by_name(name):
    for d in sorted(glob.glob("/sys/class/hwmon/hwmon*")):
        try:
            with open(os.path.join(d, "name")) as f:
                if f.read().strip() == name:
                    return d
        except OSError:
            continue
    return None


class Source:
    label = ""                       # default text before the graph
    meter = False                    # draw the latest value as a level meter, not a history
    gradients = [("cpu", GREEN_RED)]  # per series: btop theme gradient, fallback
    mirrored = False                 # two series: [0] grows up from the middle, [1] down
    percent = True                   # values are fractions; `alarm` applies

    def __init__(self, spec):
        self.spec = spec
        self.lines = []

    def label_text(self):
        return str(self.spec.get("label", self.label))

    def widest(self):
        """Longest value lines, for sizing the widget so it doesn't jump."""
        return ["100%"]

    def scale(self, series, history):
        return 1.0

    def sample(self):
        raise NotImplementedError

    def percent_line(self, frac):
        return (f"{round(frac * 100)}%", frac >= float(self.spec.get("alarm", 0.9)))


class TemperatureMixin:
    """`"temperature": true` adds a °C line from an hwmon temp input."""
    default_sensor = "coretemp"
    temp_path = None

    def temperature_path(self):
        if self.temp_path is None:
            name = self.spec.get("sensor")
            d = hwmon_by_name(str(name)) if name else self.default_hwmon()
            if not d:
                raise OSError(f"no hwmon sensor named {name or self.default_sensor!r}")
            self.temp_path = os.path.join(d, "temp1_input")
        return self.temp_path

    def default_hwmon(self):
        return hwmon_by_name(self.default_sensor)

    def temperature_line(self):
        t = read_int(self.temperature_path()) / 1000
        return (f"{round(t)}°", t >= float(self.spec.get("temperatureAlarm", 90)))

    def widest(self):
        return ["100%", "100°"] if self.spec.get("temperature") else ["100%"]


class Cpu(TemperatureMixin, Source):
    label = "cpu"

    def __init__(self, spec):
        super().__init__(spec)
        self.prev = None
        self.cores = []              # per-core fractions, for "cores": true

    @staticmethod
    def read_times():
        """[(busy, total)] jiffies for all CPUs, then each core."""
        times = []
        with open("/proc/stat") as f:
            for line in f:
                if not line.startswith("cpu"):
                    break
                v = [int(n) for n in line.split()[1:]]
                idle = v[3] + v[4]                   # idle + iowait
                total = sum(v[:8])                   # guest time is already in user/nice
                times.append((total - idle, total))
        return times

    def sample(self):
        times = self.read_times()
        prev, self.prev = self.prev, times
        if not prev:
            return None                  # usage needs two readings
        fracs = [(b - pb) / (t - pt) if t > pt else 0.0
                 for (b, t), (pb, pt) in zip(times, prev)]
        self.cores = fracs[1:]
        self.lines = [self.percent_line(fracs[0])]
        if self.spec.get("temperature"):
            self.lines.append(self.temperature_line())
        return [fracs[0]]


class Memory(Source):
    label = "mem"
    gradients = [("used", BLUE_RED)]

    def sample(self):
        info = {}
        with open("/proc/meminfo") as f:
            for line in f:
                key, _, rest = line.partition(":")
                info[key] = int(rest.split()[0])
        frac = 1 - info["MemAvailable"] / info["MemTotal"]   # as btop and free count it
        self.lines = [self.percent_line(frac)]
        return [frac]


class Gpu(TemperatureMixin, Source):
    label = "gpu"
    device = None

    def find_device(self):
        if self.device is None:
            card = self.spec.get("card")
            paths = ([f"/sys/class/drm/{card}/device"] if card else
                     sorted(os.path.dirname(p) for p in glob.glob("/sys/class/drm/card*/device/gpu_busy_percent")))
            paths = [p for p in paths if os.path.exists(os.path.join(p, "gpu_busy_percent"))]
            if not paths:
                raise OSError("no GPU with gpu_busy_percent (amdgpu) found")
            self.device = paths[0]
        return self.device

    def default_hwmon(self):
        found = glob.glob(os.path.join(self.find_device(), "hwmon", "hwmon*"))
        return found[0] if found else None

    def sample(self):
        frac = read_int(os.path.join(self.find_device(), "gpu_busy_percent")) / 100
        self.lines = [self.percent_line(frac)]
        if self.spec.get("temperature"):
            self.lines.append(self.temperature_line())
        return [frac]


class Rates(Source):
    """Two byte counters shown as rates, mirrored: [0] up, [1] down."""
    mirrored = True
    percent = False
    arrows = ("↓", "↑")

    def __init__(self, spec):
        super().__init__(spec)
        self.prev = None

    def widest(self):
        return [self.arrows[0] + "999.9M"]

    def scale(self, series, history):
        # Auto-scale to the busiest recent sample, like btop.
        return max([float(self.spec.get("minScale", 10240)), *(v for v in history if v is not None)])

    def counters(self):
        raise NotImplementedError

    def sample(self):
        now, counts = time.monotonic(), self.counters()
        prev, self.prev = self.prev, (now, counts)
        if not prev or len(prev[1]) != len(counts):
            return None
        dt = max(now - prev[0], 1e-3)
        rates = [max(0, c - p) / dt for c, p in zip(counts, prev[1])]
        self.lines = [(a + human_rate(r), False) for a, r in zip(self.arrows, rates)]
        return rates


class Network(Rates):
    label = "net"
    gradients = [("download", DOWNLOAD), ("upload", UPLOAD)]

    @staticmethod
    def default_interface():
        with open("/proc/net/route") as f:
            for line in f.readlines()[1:]:
                fields = line.split()
                if fields[1] == "00000000":
                    return fields[0]
        raise OSError("no default route")

    def counters(self):
        iface = self.spec.get("interface") or self.default_interface()
        stats = f"/sys/class/net/{iface}/statistics"
        return [read_int(f"{stats}/rx_bytes"), read_int(f"{stats}/tx_bytes")]


class Disk(Rates):
    label = "disk"
    gradients = [("download", DOWNLOAD), ("upload", UPLOAD)]
    arrows = ("R ", "W ")
    device = None

    def find_device(self):
        if self.device is None:
            self.device = self.spec.get("device")
            if not self.device:
                disks = [d for d in sorted(os.listdir("/sys/block"))
                         if not d.startswith(("loop", "ram", "zram", "dm-", "md", "sr"))]
                if not disks:
                    raise OSError("no disk found in /sys/block")
                self.device = disks[0]
        return self.device

    def counters(self):
        dev = self.find_device()
        with open("/proc/diskstats") as f:
            for line in f:
                fields = line.split()
                if fields[2] == dev:
                    return [int(fields[5]) * 512, int(fields[9]) * 512]   # sectors read, written
        raise OSError(f"no disk {dev!r} in /proc/diskstats")

    def widest(self):
        return ["100%"] if self.spec.get("show") == "usage" else super().widest()

    def sample(self):
        rates = super().sample()
        if self.spec.get("show") == "usage":
            usage = shutil.disk_usage(str(self.spec.get("mount", "/")))
            self.lines = [self.percent_line(usage.used / usage.total)]
        return rates


class Battery(Source):
    """Charge and state as text, and a level meter filled to the charge in one
    colour from red (empty) to green (full). "graph": "charge" or "power"
    shows a history of the charge or the power draw in watts instead."""
    percent = False
    path = None

    @property
    def power(self):
        return self.spec.get("graph") == "power"

    @property
    def meter(self):
        return self.spec.get("graph", "level") == "level"

    @property
    def gradients(self):
        if self.meter:
            return [("battery", RED_GREEN)]      # btop has none; RED_GREEN unless configured
        return [("temp", TEMP)] if self.power else [("cpu", GREEN_RED)]

    def find_battery(self):
        if self.path is None:
            name = self.spec.get("battery")
            found = ([f"/sys/class/power_supply/{name}"] if name else
                     sorted(glob.glob("/sys/class/power_supply/BAT*")))
            if not found or not os.path.exists(found[0]):
                raise OSError("no battery found")
            self.path = found[0]
        return self.path

    def read(self, name):
        with open(os.path.join(self.find_battery(), name)) as f:
            return f.read().strip()

    def charge(self):
        """Percent of what the battery holds now, as UPower and the Omarchy bar
        report it. Some drivers' `capacity` (e.g. this Mac's) is relative to
        the design capacity instead, which reads low on a worn battery."""
        for now, full in (("charge_now", "charge_full"), ("energy_now", "energy_full")):
            try:
                return min(100, round(100 * int(self.read(now)) / int(self.read(full))))
            except (OSError, ValueError, ZeroDivisionError):
                continue
        return int(self.read("capacity"))

    def label_text(self):
        if "label" in self.spec:
            return str(self.spec["label"])
        try:
            capacity, status = self.charge(), self.read("status")
        except (OSError, ValueError):
            return "\U000F0091"                              # battery unknown
        if status == "Charging":
            return "\U000F0084"                              # battery charging
        if capacity >= 95:
            return "\U000F0079"                              # battery full
        return chr(0xF007A + max(0, min(8, round(capacity / 10) - 1)))   # 10% .. 90%

    def widest(self):
        return ["100%", "10:00"] if self.spec.get("detail", "none") != "none" else ["100%"]

    def scale(self, series, history):
        if not self.power:
            return 1.0
        return float(self.spec.get("maxPower") or
                     max([10.0, *(v for v in history if v is not None)]))

    def sample(self):
        capacity, status = self.charge(), self.read("status")
        try:
            current = int(self.read("current_now")) / 1e6        # A
            volts = int(self.read("voltage_now")) / 1e6          # V
        except (OSError, ValueError):
            current, volts = int(self.read("power_now")) / 1e6, 1.0   # power_now is in µW
        watts = current * volts
        self.lines = [(f"{capacity}%", status == "Discharging" and capacity <= int(self.spec.get("low", 15)))]
        detail = self.spec.get("detail", "none")
        if detail == "power":
            self.lines.append((f"{watts:.1f}W", False))
        elif detail == "time":
            self.lines.append((self.time_left(status, current), False))
        return [watts] if self.power else [capacity / 100]

    def time_left(self, status, current):
        """h:mm until empty (discharging) or full (charging), from charge or energy."""
        if current <= 0 or status not in ("Charging", "Discharging"):
            return "full" if status == "Full" else "—"
        try:
            now, full, rate = int(self.read("charge_now")), int(self.read("charge_full")), current * 1e6
        except (OSError, ValueError):
            now, full = int(self.read("energy_now")), int(self.read("energy_full"))
            rate = int(self.read("power_now"))
        hours = (now if status == "Discharging" else full - now) / rate
        return f"{int(hours)}:{int(hours * 60) % 60:02d}"


class Fan(Source):
    label = "fan"
    gradients = [("temp", TEMP)]
    percent = False
    input_path = None

    def find_fan(self):
        if self.input_path is None:
            n = int(self.spec.get("fan", 1))
            found = sorted(glob.glob(f"/sys/class/hwmon/hwmon*/fan{n}_input") +
                           glob.glob(f"/sys/class/hwmon/hwmon*/device/fan{n}_input"))
            if not found:
                raise OSError(f"no fan{n}_input in /sys/class/hwmon")
            self.input_path = found[0]
        return self.input_path

    def widest(self):
        return ["9999"]

    def scale(self, series, history):
        try:
            return float(read_int(self.find_fan().replace("_input", "_max")))
        except (OSError, ValueError):
            return max([1000.0, *(v for v in history if v is not None)])

    def sample(self):
        rpm = read_int(self.find_fan())
        self.lines = [(str(rpm), False)]
        return [rpm]


SOURCES = {
    "glance.cpu": Cpu,
    "glance.memory": Memory,
    "glance.gpu": Gpu,
    "glance.network": Network,
    "glance.disk": Disk,
    "glance.battery": Battery,
    "glance.fan": Fan,
}
