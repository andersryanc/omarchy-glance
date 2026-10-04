#!/usr/bin/env python3
"""Custom Touch Bar renderer for t1bridge (Touch Bar hardware IPC v1).

The bar is built from ~/.config/touchbar/config.json (or config.default.json
next to this script when there is none) and reloads when that file changes.
A config has two layers, "default" and "fn" (shown while Fn is held), each
with left/center/right lists of widgets, like the Omarchy bar's shell.json.
See README.md for the format.

Spec: ~/Work/touchbar/t1bridge-interfaces.md
"""

import array
import fcntl
import json
import mmap
import os
import select
import socket
import struct
import subprocess
import sys
import time

import cairo

SOCK_PATH = "/run/t1bridge/touchbar.sock"
HERE = os.path.dirname(os.path.realpath(__file__))
DEFAULT_CONFIG = os.path.join(HERE, "config.default.json")
USER_CONFIG = os.path.join(os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config"),
                           "touchbar", "config.json")
POLL_SECONDS = 1                 # how often to check the config and usage files

# --- protocol constants -----------------------------------------------------
MAGIC = b"T1HW"
MAJOR = 1
HDR = struct.Struct("<4sHHII")  # magic, major, type, len, req id

HELLO, REGISTER_BUFFER, SUBMIT_FRAME, TAP_KEYS = 0x0001, 0x0002, 0x0003, 0x0004
HELLO_ACK, ACK, ERROR = 0x8001, 0x8002, 0x8003
FRAME_RELEASED, INPUT_FRAME = 0x9001, 0x9002

FEAT_MEMFD, FEAT_INPUT, FEAT_KEYS = 0x01, 0x02, 0x04

# Keys the service will tap for us (minor 0): Esc, F1-F12.
KEYCODES = {"esc": 1, **{f"f{n}": 58 + n for n in range(1, 11)}, "f11": 87, "f12": 88}

ERRORS = {1: "unsupported message", 2: "unsupported feature", 3: "resource limit",
          4: "invalid buffer", 5: "unknown buffer", 6: "buffer busy",
          7: "action denied", 8: "device unavailable", 9: "I/O failure",
          10: "internal failure"}

# --- hardware facts and widget metrics ---------------------------------------
# The panel reports 2170 px along the bar, but only the first 2060 light up
# (same with the stock renderer; measured 2026-10-04). Lay out within this.
VISIBLE_WIDTH = 2060

# Orientation of the logical (landscape) canvas relative to the buffer.
# Flip these if Esc shows up on the wrong end or the label is mirrored.
# Cairo drawing (everything but Esc and the overlays) assumes no flips.
FLIP_X = False
FLIP_Y = False

KEY_WIDTH = 140                  # default width of Esc and buttons
KEY_PAD = 4                      # inset of each key face inside its slot
DIM = 0.25                       # brightness multiplier while idle
COMMAND_TIMEOUT = 10             # kill a command widget's script after this long
OMARCHY_BIN = "/usr/share/omarchy/bin"
AGENTS_TOGGLE = "omarchy-shell -q omarchy.agents toggle"
USAGE_DIR = os.path.expanduser("~/.local/state/omarchy/agents/usage")
USAGE_ICON = "\U000F16A3"        # the Omarchy bar's agents glyph (Nerd Font)
METER_ALARM = 0.9                # turn red at this fraction used, like the bar panel

# 5x7 glyphs for the Esc label
GLYPHS = {
    "e": ["     ", "     ", " ### ", "#   #", "#####", "#    ", " ### "],
    "s": ["     ", "     ", " ####", "#    ", " ### ", "    #", "#### "],
    "c": ["     ", "     ", " ### ", "#    ", "#    ", "#   #", " ### "],
}


def log(*args):
    print("touchbar-renderer:", *args, file=sys.stderr, flush=True)


def hex_rgb(value, fallback):
    try:
        v = value.lstrip("#")
        return tuple(int(v[i:i + 2], 16) for i in (0, 2, 4)) if len(v) == 6 else fallback
    except (AttributeError, ValueError):
        return fallback


class Config:
    """A validated view of one config file."""

    def __init__(self, data):
        if not isinstance(data, dict) or data.get("version") != 1:
            raise ValueError('expected an object with "version": 1')
        debug = data.get("debug") or {}
        colors = data.get("colors") or {}
        self.debug_background = bool(debug.get("background"))
        self.test_pattern = bool(debug.get("testPattern"))
        self.border = bool(debug.get("border"))
        self.background = hex_rgb(colors.get("background"), (0, 0, 0))
        self.debug_bg = hex_rgb(colors.get("debugBackground"), (0x10, 0x60, 0x90))
        self.debug_bg_fn = hex_rgb(colors.get("debugBackgroundFn"), (0x60, 0x20, 0x90))
        self.key = hex_rgb(colors.get("key"), (0x30, 0x30, 0x30))
        self.key_pressed = hex_rgb(colors.get("keyPressed"), (0x80, 0x80, 0x80))
        self.text = hex_rgb(colors.get("text"), (0xFF, 0xFF, 0xFF))
        self.urgent = hex_rgb(colors.get("urgent"), (0xE0, 0x5A, 0x5A))
        self.font = str(data.get("font") or "JetBrainsMono Nerd Font")
        self.idle_dim = float(data.get("idleDimSeconds") or 0)
        self.repeat_delay = float(data.get("repeatDelay", 0.4))
        self.repeat_interval = max(0.03, float(data.get("repeatInterval", 0.12)))
        layers = data.get("layers")
        if not isinstance(layers, dict) or not isinstance(layers.get("default"), dict):
            raise ValueError('"layers" needs at least a "default" layer')
        self.layers = {}
        for name in ("default", "fn"):
            layer = layers.get(name) or {}
            if not isinstance(layer, dict):
                raise ValueError(f'layer "{name}" must be an object')
            sections = {}
            for section in ("left", "center", "right"):
                items = layer.get(section) or []
                if not isinstance(items, list) or not all(isinstance(i, dict) for i in items):
                    raise ValueError(f'"layers.{name}.{section}" must be a list of objects')
                sections[section] = items
            self.layers[name] = sections
        self.has_fn = any(self.layers["fn"].values())


class Widget:
    """One placed widget. `kind` is esc, button, command, agents or spacer."""

    def __init__(self, key, layer, kind, spec):
        self.key, self.layer, self.kind, self.spec = key, layer, kind, spec
        self.rect = (0, 0, 0, 0)
        self.repeats = kind == "button" and bool(spec.get("repeat"))

    def hit(self, x, y):
        x0, y0, x1, y1 = self.rect
        return x0 <= x < x1 and y0 <= y < y1


class Renderer:
    def __init__(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
        self.sock.connect(SOCK_PATH)
        self.next_req = 1
        self.pending = {}            # req id -> description
        self.buffers = {}            # buffer id -> mmap
        self.surfaces = {}           # buffer id -> cairo surface over the mmap
        self.free = []               # buffer ids we may draw into
        self.init_state()

    def init_state(self):
        self.dirty = True
        self.frame_id = 0
        self.widgets = []
        self.owner = {}              # contact id -> Widget it first touched
        self.repeat_at = {}          # contact id -> monotonic time of next repeat
        self.last_input = time.monotonic()
        self.dimmed = False
        self.fn = False              # Fn key held
        self.logged_contacts = set()
        self.touch_marks = []        # logical x of current touches (test pattern)
        self.usage = {}              # agent -> (mtime, [(label, fraction used)])
        self.commands = {}           # widget key -> command widget state
        self.children = []           # fire-and-forget processes to reap
        self.config_source = None    # (path, mtime) of the loaded config
        self.config = None
        # scratch context for measuring text outside a frame
        self.measure = cairo.Context(cairo.ImageSurface(cairo.FORMAT_RGB24, 1, 1))

    # --- wire helpers -------------------------------------------------------
    def send(self, mtype, payload=b"", fds=None, what=""):
        req = self.next_req
        self.next_req = self.next_req % 0xFFFFFFFF + 1
        pkt = HDR.pack(MAGIC, MAJOR, mtype, len(payload), req) + payload
        if fds:
            anc = [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", fds))]
            self.sock.sendmsg([pkt], anc)
        else:
            self.sock.send(pkt)
        self.pending[req] = what or hex(mtype)
        return req

    def recv(self):
        data = self.sock.recv(65536)
        if not data:
            raise ConnectionError("service closed the connection")
        magic, major, mtype, length, req = HDR.unpack_from(data)
        if magic != MAGIC or major != MAJOR or len(data) != HDR.size + length:
            raise ConnectionError("malformed packet")
        return mtype, req, data[HDR.size:]

    # --- setup --------------------------------------------------------------
    def handshake(self):
        self.send(HELLO, struct.pack("<HHQ", 0, 0, FEAT_MEMFD | FEAT_INPUT | FEAT_KEYS), what="hello")
        mtype, req, payload = self.recv()
        self.pending.pop(req, None)
        if mtype == ERROR:
            raise RuntimeError("hello rejected: " + ERRORS.get(struct.unpack("<I", payload)[0], "?"))
        minor, _, w, h, fmt, max_buffers = struct.unpack("<HHIIII", payload)
        log(f"connected: minor={minor} {w}x{h} fmt={fmt} max_buffers={max_buffers}")
        self.set_geometry(w, h)

        for buf_id in range(1, min(2, max_buffers) + 1):
            size = self.stride * self.h
            fd = os.memfd_create(f"touchbar-{buf_id}", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
            os.ftruncate(fd, size)
            fcntl.fcntl(fd, fcntl.F_ADD_SEALS,
                        fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SEAL)
            self.buffers[buf_id] = mmap.mmap(fd, size, mmap.MAP_SHARED, mmap.PROT_READ | mmap.PROT_WRITE)
            self.add_surface(buf_id)
            self.send(REGISTER_BUFFER, struct.pack("<IIQ", buf_id, self.stride, size),
                      fds=[fd], what=f"register buffer {buf_id}")
            os.close(fd)
            self.free.append(buf_id)

    def set_geometry(self, w, h):
        self.w, self.h = w, h
        self.stride = w * 4
        self.portrait = h > w
        self.long, self.short = max(w, h), min(w, h)
        self.visible = min(self.long, VISIBLE_WIDTH)   # usable length for layout

    def add_surface(self, buf_id):
        # Cairo's RGB24 is the same little-endian XRGB8888 the service wants.
        if cairo.ImageSurface.format_stride_for_width(cairo.FORMAT_RGB24, self.w) == self.stride:
            self.surfaces[buf_id] = cairo.ImageSurface.create_for_data(
                self.buffers[buf_id], cairo.FORMAT_RGB24, self.w, self.h, self.stride)

    # --- config -------------------------------------------------------------
    def poll_config(self):
        path = USER_CONFIG if os.path.exists(USER_CONFIG) else DEFAULT_CONFIG
        try:
            source = (path, os.stat(path).st_mtime_ns)
        except OSError:
            source = (path, None)
        if source == self.config_source:
            return
        self.config_source = source
        try:
            with open(path, encoding="utf-8") as f:
                config = Config(json.load(f))
        except (OSError, ValueError, TypeError) as e:
            log(f"{path}: {e}; " + ("keeping the previous config" if self.config else "using the default"))
            if self.config:
                return
            with open(DEFAULT_CONFIG, encoding="utf-8") as f:
                config = Config(json.load(f))
        log(f"loaded {path}")
        self.config = config
        for state in self.commands.values():
            if state.get("proc"):
                state["proc"].kill()
        self.commands = {}
        self.owner, self.repeat_at = {}, {}
        self.build()
        self.dirty = True

    def build(self):
        """Create and place the widgets of both layers from the config."""
        self.widgets = []
        for layer, sections in self.config.layers.items():
            for section, items in sections.items():
                placed = []
                for i, spec in enumerate(items):
                    kind = self.kind_of(spec)
                    if kind is None:
                        log(f"layers.{layer}.{section}[{i}]: unknown widget {spec.get('id')!r}")
                        continue
                    placed.append(Widget(f"{layer}.{section}.{i}", layer, kind, spec))
                self.place(placed, section)
                self.widgets += placed
        for w in self.widgets:
            if w.kind == "command" and w.key not in self.commands:
                self.commands[w.key] = {"text": "", "urgent": False, "next": 0.0,
                                        "proc": None, "out": b"", "started": 0.0}

    def relayout(self):
        """Re-place widgets after a width change, keeping fingers attached."""
        by_key = {w.key: w for w in self.widgets}
        for layer, sections in self.config.layers.items():
            for section in sections:
                prefix = f"{layer}.{section}."
                self.place([w for w in self.widgets if w.key.startswith(prefix)], section)
        self.owner = {cid: by_key[w.key] for cid, w in self.owner.items()}
        self.dirty = True

    @staticmethod
    def kind_of(spec):
        t = spec.get("type")
        if t in ("button", "command"):
            return t
        return {"touchbar.esc": "esc", "touchbar.agents": "agents",
                "touchbar.spacer": "spacer"}.get(spec.get("id"))

    def place(self, widgets, section):
        widths = [self.width_of(w) for w in widgets]
        total = sum(widths)
        x = {"left": 0, "center": (self.visible - total) // 2, "right": self.visible - total}[section]
        for w, width in zip(widgets, widths):
            w.rect = (x, 0, x + width, self.short)
            x += width

    def width_of(self, w):
        s = w.spec
        if w.kind == "spacer":
            return int(s.get("size", 40))
        if w.kind == "agents":
            n = len(self.limits_for(w))
            return 14 + 30 + 14 + n * self.meter_width(w) + (n - 1) * 24 + 16 + 2 * KEY_PAD
        if w.kind == "command" and not s.get("width"):
            text = self.commands.get(w.key, {}).get("text", "")
            self.measure.select_font_face(self.config.font)
            self.measure.set_font_size(float(s.get("fontSize", 18)))
            return max(KEY_WIDTH // 2, int(self.measure.text_extents(text).x_advance) + 32 + 2 * KEY_PAD)
        return int(s.get("width", KEY_WIDTH))

    # --- coordinates --------------------------------------------------------
    def to_logical(self, x, y):
        """Buffer/touch coordinates -> logical landscape (lx along the bar)."""
        lx, ly = (y, x) if self.portrait else (x, y)
        if FLIP_X:
            lx = self.long - 1 - lx
        if FLIP_Y:
            ly = self.short - 1 - ly
        return lx, ly

    def fill(self, buf, lx0, ly0, lx1, ly1, rgb):
        """Fill a logical rect [lx0,lx1) x [ly0,ly1)."""
        if FLIP_X:
            lx0, lx1 = self.long - lx1, self.long - lx0
        if FLIP_Y:
            ly0, ly1 = self.short - ly1, self.short - ly0
        bx0, by0, bx1, by1 = (ly0, lx0, ly1, lx1) if self.portrait else (lx0, ly0, lx1, ly1)
        bx0, bx1 = max(0, bx0), min(self.w, bx1)
        by0, by1 = max(0, by0), min(self.h, by1)
        if bx0 >= bx1 or by0 >= by1:
            return
        if self.dimmed:
            rgb = tuple(int(c * DIM) for c in rgb)
        row = bytes((rgb[2], rgb[1], rgb[0], 0)) * (bx1 - bx0)
        for by in range(by0, by1):
            off = by * self.stride + bx0 * 4
            buf[off:off + len(row)] = row

    # --- state --------------------------------------------------------------
    def fn_layer(self):
        """The Fn layer stays up while a finger is still on one of its widgets."""
        return self.config.has_fn and (self.fn or any(w.layer == "fn" for w in self.owner.values()))

    def visible_widgets(self):
        layer = "fn" if self.fn_layer() else "default"
        return [w for w in self.widgets if w.layer == layer]

    def pressed(self, widget):
        return widget in self.owner.values()

    # --- actions ------------------------------------------------------------
    def press(self, w):
        s = w.spec
        if w.kind == "esc":
            self.tap_key("esc")
        elif w.kind == "button":
            if s.get("key"):
                self.tap_key(str(s["key"]).lower())
            elif s.get("exec"):
                self.spawn(s["exec"])
        elif w.kind == "agents":
            cmd = s.get("onTap", AGENTS_TOGGLE)
            if cmd:
                self.spawn(cmd)
        elif w.kind == "command" and s.get("onTap"):
            self.spawn(s["onTap"])

    def tap_key(self, name):
        code = KEYCODES.get(name)
        if code is None:
            log(f"unsupported key {name!r}; the service allows esc and f1-f12")
            return
        self.send(TAP_KEYS, struct.pack("<B3xHHHH", 1, code, 0, 0, 0), what=f"tap {name}")

    def command_env(self):
        env = dict(os.environ)
        # in case the service started before the session environment was imported
        env.setdefault("OMARCHY_PATH", "/usr/share/omarchy")
        if OMARCHY_BIN not in env.get("PATH", "").split(":"):
            env["PATH"] = OMARCHY_BIN + ":" + env.get("PATH", "/usr/bin")
        return env

    def spawn(self, cmd):
        """Run a shell command without blocking the render loop."""
        try:
            self.children.append(subprocess.Popen(
                ["bash", "-c", cmd], env=self.command_env(), stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True))
        except OSError as e:
            log(f"could not run {cmd!r}: {e}")

    # --- command widgets ----------------------------------------------------
    def run_commands(self, now):
        self.children = [p for p in self.children if p.poll() is None]
        for w in self.widgets:
            if w.kind != "command" or not w.spec.get("exec"):
                continue
            state = self.commands[w.key]
            proc = state["proc"]
            if proc and now - state["started"] > COMMAND_TIMEOUT:
                log(f"{w.spec.get('id')}: command timed out")
                proc.kill()
                self.finish_command(w, state)
            elif not proc and now >= state["next"]:
                try:
                    state["proc"] = subprocess.Popen(
                        ["bash", "-c", w.spec["exec"]], env=self.command_env(),
                        stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                        stderr=subprocess.DEVNULL, start_new_session=True)
                    state["out"], state["started"] = b"", now
                except OSError as e:
                    log(f"{w.spec.get('id')}: could not run: {e}")
                interval = float(w.spec.get("interval", 0))
                state["next"] = now + interval if interval > 0 else float("inf")

    def command_fds(self):
        return {s["proc"].stdout.fileno(): key for key, s in self.commands.items() if s["proc"]}

    def read_command(self, key):
        state = self.commands[key]
        chunk = os.read(state["proc"].stdout.fileno(), 65536)
        if chunk and len(state["out"]) < 65536:
            state["out"] += chunk
            return
        w = next(w for w in self.widgets if w.key == key)
        self.finish_command(w, state)

    def finish_command(self, w, state):
        proc, state["proc"] = state["proc"], None
        proc.stdout.close()
        try:
            proc.wait(timeout=1)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
        # Plain text, or Waybar-style JSON: {"text": ..., "class": ...}
        out = state["out"].decode("utf-8", "replace").strip()
        text, urgent = out.splitlines()[0] if out else "", False
        if out.startswith("{"):
            try:
                data = json.loads(out)
                text = str(data.get("text", ""))
                classes = data.get("class", [])
                classes = [classes] if isinstance(classes, str) else classes
                urgent = any(c in ("urgent", "critical") for c in classes)
            except (ValueError, AttributeError):
                pass
        if (text, urgent) != (state["text"], state["urgent"]):
            state["text"], state["urgent"] = text, urgent
            if not w.spec.get("width"):
                self.relayout()
            self.dirty = True

    # --- agent usage records -----------------------------------------------
    def poll_usage(self):
        """Read limits for every agents widget from its Omarchy usage record."""
        for agent in {str(w.spec.get("agent", "claude")) for w in self.widgets if w.kind == "agents"}:
            path = os.path.join(USAGE_DIR, f"{agent}.json")
            try:
                mtime = os.stat(path).st_mtime_ns
            except OSError:
                mtime = None
            old = self.usage.get(agent)
            if old and old[0] == mtime:
                continue
            limits = []
            try:
                with open(path) as f:
                    record = json.load(f)
                for entry in record.get("limits") or []:
                    frac = float(entry.get("percent", -1))
                    if frac >= 0:
                        label = str(entry.get("label", "")).split(" (")[0] or "Limit"
                        limits.append((label, frac))
            except (OSError, ValueError, TypeError, AttributeError) as e:
                if mtime is not None:
                    log(f"unreadable usage record {path}: {e}")
            self.usage[agent] = (mtime, limits)
            if not old or old[1] != limits:
                self.relayout()

    def limits_for(self, w):
        limits = self.usage.get(str(w.spec.get("agent", "claude")), (None, []))[1]
        return limits or [("Session", None), ("Weekly", None)]

    @staticmethod
    def meter_width(w):
        return int(w.spec.get("meterWidth", 320))

    # --- drawing ------------------------------------------------------------
    def draw(self, buf):
        c = self.config
        if c.debug_background:
            bg = c.debug_bg_fn if self.fn else c.debug_bg
        else:
            bg = c.background
        self.fill(buf, 0, 0, self.long, self.short, bg)
        widgets = self.visible_widgets()
        for w in widgets:
            if w.kind == "esc":
                self.draw_esc(buf, w)
        surface = self.surfaces.get(self.drawing)
        if surface and not self.portrait and not FLIP_X and not FLIP_Y:
            surface.mark_dirty()     # we wrote pixels behind cairo's back
            cr = cairo.Context(surface)
            cr.select_font_face(c.font)
            for w in widgets:
                if w.kind == "button":
                    self.draw_button(cr, w)
                elif w.kind == "command":
                    self.draw_command(cr, w)
                elif w.kind == "agents":
                    self.draw_agents(cr, w)
            surface.flush()
        if c.test_pattern:
            self.draw_test_pattern(buf)
        if c.border or c.test_pattern:
            self.draw_border(buf)

    def draw_esc(self, buf, w):
        x0, y0, x1, y1 = w.rect
        pad = KEY_PAD
        self.fill(buf, x0 + pad, y0 + pad, x1 - pad, y1 - pad,
                  self.config.key_pressed if self.pressed(w) else self.config.key)
        # "esc" label, 5x7 glyphs scaled
        scale = max(1, (self.short - 2 * pad) // 14)
        text = "esc"
        tw = (len(text) * 6 - 1) * scale
        tx = x0 + (x1 - x0 - tw) // 2
        ty = (self.short - 7 * scale) // 2
        for i, ch in enumerate(text):
            for gy, line in enumerate(GLYPHS[ch]):
                for gx, px in enumerate(line):
                    if px == "#":
                        lx = tx + (i * 6 + gx) * scale
                        ly = ty + gy * scale
                        self.fill(buf, lx, ly, lx + scale, ly + scale, self.config.text)

    def color(self, rgb, alpha=1.0):
        k = DIM if self.dimmed else 1.0
        return tuple(c / 255 * k for c in rgb) + (alpha,)

    def rounded_rect(self, cr, x, y, w, h, r):
        cr.new_sub_path()
        cr.arc(x + w - r, y + r, r, -1.5708, 0)
        cr.arc(x + w - r, y + h - r, r, 0, 1.5708)
        cr.arc(x + r, y + h - r, r, 1.5708, 3.1416)
        cr.arc(x + r, y + r, r, 3.1416, 4.7124)
        cr.close_path()

    def draw_key_face(self, cr, w):
        x0, y0, x1, y1 = w.rect
        p = KEY_PAD
        cr.rectangle(x0 + p, y0 + p, x1 - x0 - 2 * p, y1 - y0 - 2 * p)
        cr.set_source_rgba(*self.color(self.config.key_pressed if self.pressed(w) else self.config.key))
        cr.fill()

    def draw_text(self, cr, text, cx, cy, size, rgb=None):
        """Draw text centred on (cx, cy) by its ink extents (good for icons)."""
        cr.set_font_size(size)
        ext = cr.text_extents(text)
        cr.move_to(cx - ext.width / 2 - ext.x_bearing, cy - ext.height / 2 - ext.y_bearing)
        cr.set_source_rgba(*self.color(rgb or self.config.text))
        cr.show_text(text)

    def draw_button(self, cr, w):
        self.draw_key_face(cr, w)
        x0, y0, x1, y1 = w.rect
        s = w.spec
        if s.get("icon"):
            self.draw_text(cr, str(s["icon"]), (x0 + x1) / 2, (y0 + y1) / 2, float(s.get("iconSize", 30)))
        elif s.get("label"):
            self.draw_label(cr, str(s["label"]), w, float(s.get("fontSize", 18)), self.config.text)

    def draw_label(self, cr, text, w, size, rgb):
        """Draw a text line centred in the widget on the font's baseline."""
        x0, y0, x1, y1 = w.rect
        cr.set_font_size(size)
        ext, fext = cr.text_extents(text), cr.font_extents()
        cr.move_to((x0 + x1 - ext.x_advance) / 2, (y0 + y1) / 2 + (fext[0] - fext[1]) / 2)
        cr.set_source_rgba(*self.color(rgb))
        cr.show_text(text)

    def draw_command(self, cr, w):
        self.draw_key_face(cr, w)
        state = self.commands[w.key]
        self.draw_label(cr, state["text"], w, float(w.spec.get("fontSize", 18)),
                        self.config.urgent if state["urgent"] else self.config.text)

    def draw_agents(self, cr, w):
        """Key-style container: agents icon, then one meter per limit."""
        gap, icon_w, meter_w = 24, 30, self.meter_width(w)
        white, urgent = self.config.text, self.config.urgent
        self.draw_key_face(cr, w)
        x0 = w.rect[0] + KEY_PAD
        self.draw_text(cr, USAGE_ICON, x0 + 14 + icon_w / 2, self.short / 2, 30)

        cr.set_font_size(15)
        x = x0 + 14 + icon_w + 14
        for label, frac in self.limits_for(w):
            alarm = frac is not None and frac >= METER_ALARM
            cr.set_source_rgba(*self.color(white))
            cr.move_to(x, 25)
            cr.show_text(label)
            pct = "—" if frac is None else f"{round(frac * 100)}%"
            ext = cr.text_extents(pct)
            cr.set_source_rgba(*self.color(urgent if alarm else white))
            cr.move_to(x + meter_w - ext.x_advance, 25)
            cr.show_text(pct)

            self.rounded_rect(cr, x, 33, meter_w, 10, 5)
            cr.set_source_rgba(*self.color(white, 0.2))
            cr.fill()
            if frac:
                fw = max(10, meter_w * min(frac, 1.0))
                self.rounded_rect(cr, x, 33, fw, 10, 5)
                cr.set_source_rgba(*self.color(urgent if alarm else white))
                cr.fill()
            x += meter_w + gap

    def draw_border(self, buf, b=2, red=(0xFF, 0, 0)):
        v = self.visible
        self.fill(buf, 0, 0, v, b, red)
        self.fill(buf, 0, self.short - b, v, self.short, red)
        self.fill(buf, 0, 0, b, self.short, red)
        self.fill(buf, v - b, 0, v, self.short, red)

    def draw_test_pattern(self, buf):
        white, yellow = (0xFF, 0xFF, 0xFF), (0xFF, 0xFF, 0)
        for x in range(100, self.long, 100):
            if x % 500 == 0:
                self.fill(buf, x - 1, 0, x + 1, self.short, yellow)
            else:
                self.fill(buf, x - 1, 0, x + 1, self.short // 2, white)
        for x in self.touch_marks:   # white bar under each finger
            self.fill(buf, x - 3, 0, x + 3, self.short, white)

    def present(self):
        if not self.dirty or not self.free:
            return
        buf_id = self.free.pop(0)
        self.drawing = buf_id
        self.draw(self.buffers[buf_id])
        self.frame_id += 1
        self.send(SUBMIT_FRAME, struct.pack("<IQI", buf_id, self.frame_id, 0),
                  what=f"submit frame {self.frame_id}")
        self.dirty = False

    # --- events -------------------------------------------------------------
    def on_input(self, payload):
        _ns, fn, count, _ = struct.unpack_from("<QBBH", payload)
        before = (self.fn, self.fn_layer(), set(self.owner.values()))
        self.fn = bool(fn)

        touching = {}
        for i in range(count):
            cid, tip, in_range, _, x, y = struct.unpack_from("<BBBBII", payload, 12 + i * 12)
            if tip:
                touching[cid] = self.to_logical(x, y)

        # A contact belongs to whatever it first touched until it lifts.
        for cid in list(self.owner):
            if cid not in touching:
                del self.owner[cid]
                self.repeat_at.pop(cid, None)
        now = time.monotonic()
        for cid, (lx, ly) in touching.items():
            if self.config.test_pattern and cid not in self.logged_contacts:
                log(f"touch down id={cid} x={lx} y={ly}")
            if cid in self.owner:
                continue
            widget = next((w for w in self.visible_widgets()
                           if w.kind != "spacer" and w.hit(lx, ly)), None)
            if widget:
                self.owner[cid] = widget
                self.press(widget)
                if widget.repeats:
                    self.repeat_at[cid] = now + self.config.repeat_delay

        if self.config.test_pattern:
            self.logged_contacts = set(touching)
            marks = [lx for lx, _ in touching.values()]
            if marks != self.touch_marks:
                self.touch_marks = marks
                self.dirty = True
        after = (self.fn, self.fn_layer(), set(self.owner.values()))
        if after[1:] != before[1:] or (self.config.debug_background and after[0] != before[0]):
            self.dirty = True
        if count:
            self.last_input = now
            if self.dimmed:
                self.dimmed = False
                self.dirty = True

    def run_repeats(self, now):
        for cid, due in list(self.repeat_at.items()):
            if now >= due:
                self.press(self.owner[cid])
                self.repeat_at[cid] = max(due + self.config.repeat_interval, now)

    def handle(self, mtype, req, payload):
        if mtype == ACK:
            self.pending.pop(req, None)
        elif mtype == ERROR:
            code = struct.unpack("<I", payload)[0]
            log(f"{self.pending.pop(req, '?')}: error {code} ({ERRORS.get(code, '?')})")
        elif mtype == FRAME_RELEASED:
            buf_id, _ = struct.unpack("<IQ", payload)
            self.free.append(buf_id)
        elif mtype == INPUT_FRAME:
            self.on_input(payload)
        else:
            log(f"ignoring message type {mtype:#06x}")

    def run(self):
        self.handshake()
        next_poll = 0.0
        while True:
            now = time.monotonic()
            if now >= next_poll:
                self.poll_config()
                self.poll_usage()
                next_poll = now + POLL_SECONDS
            self.run_commands(now)
            self.run_repeats(now)
            idle = self.config.idle_dim
            if idle > 0 and not self.dimmed and now >= self.last_input + idle:
                self.dimmed = True
                self.dirty = True
            self.present()
            deadlines = [next_poll, *self.repeat_at.values(),
                         *(s["next"] for s in self.commands.values() if not s["proc"])]
            if idle > 0 and not self.dimmed:
                deadlines.append(self.last_input + idle)
            timeout = max(0.0, min(deadlines) - now)
            fds = self.command_fds()
            ready, _, _ = select.select([self.sock, *fds], [], [], timeout)
            for fd in ready:
                if fd is self.sock:
                    self.handle(*self.recv())
                else:
                    self.read_command(fds[fd])


if __name__ == "__main__":
    try:
        Renderer().run()
    except KeyboardInterrupt:
        pass
