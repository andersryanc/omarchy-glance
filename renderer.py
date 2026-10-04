#!/usr/bin/env python3
"""Custom Touch Bar renderer for t1bridge (Touch Bar hardware IPC v1).

Base layer: an Esc key on the left (KEY_ESC through the root service) and, on
the right, a Claude usage widget with the session and weekly limits from the
Omarchy agents usage record; tapping it toggles the agents panel in the
Omarchy bar. Holding Fn adds brightness and volume keys next to Esc, which
run the same Omarchy commands as the keyboard's media keys (so the OSD shows).
Optionally dims after IDLE_SECONDS without input to spare the OLED panel.

Spec: ~/Work/touchbar/t1bridge-interfaces.md
"""

import array
import fcntl
import json
import mmap
import os
import select
import signal
import socket
import struct
import subprocess
import sys
import time

import cairo

SOCK_PATH = "/run/t1bridge/touchbar.sock"

# --- protocol constants -----------------------------------------------------
MAGIC = b"T1HW"
MAJOR = 1
HDR = struct.Struct("<4sHHII")  # magic, major, type, len, req id

HELLO, REGISTER_BUFFER, SUBMIT_FRAME, TAP_KEYS = 0x0001, 0x0002, 0x0003, 0x0004
HELLO_ACK, ACK, ERROR = 0x8001, 0x8002, 0x8003
FRAME_RELEASED, INPUT_FRAME = 0x9001, 0x9002

FEAT_MEMFD, FEAT_INPUT, FEAT_KEYS = 0x01, 0x02, 0x04
KEY_ESC = 1

ERRORS = {1: "unsupported message", 2: "unsupported feature", 3: "resource limit",
          4: "invalid buffer", 5: "unknown buffer", 6: "buffer busy",
          7: "action denied", 8: "device unavailable", 9: "I/O failure",
          10: "internal failure"}

# --- look and feel ----------------------------------------------------------
# Debug background: blue, purple while Fn is held. Off = black, which also
# leaves those OLED pixels switched off.
DEBUG_BG = False
BG_DEBUG = (0x10, 0x60, 0x90)    # (R, G, B)
BG_DEBUG_FN = (0x60, 0x20, 0x90)
BG = (0x00, 0x00, 0x00)
ESC_BG = (0x30, 0x30, 0x30)      # key face (every key and the usage widget)
ESC_PRESSED = (0x80, 0x80, 0x80)
ESC_FG = (0xFF, 0xFF, 0xFF)
ESC_WIDTH = 140                  # logical px along the long axis
KEY_PAD = 4                      # inset of each key face inside its slot
IDLE_SECONDS = 0                 # dim after this long without touches; 0 = never
DIM = 0.25                       # brightness multiplier while idle
FONT = "JetBrainsMono Nerd Font"

# Claude usage widget. The record is written by omarchy-agent-usage-update,
# which the Omarchy bar's agents widget runs every refreshIntervalSec (900 s
# by default); we only watch the file.
USAGE_FILE = os.path.expanduser("~/.local/state/omarchy/agents/usage/claude.json")
USAGE_POLL_SECONDS = 5
USAGE_ICON = "\U000F16A3"        # the Omarchy bar's agents glyph (Nerd Font)
METER_WIDTH = 320                # px per progress bar
METER_ALARM = 0.9                # turn red at this fraction used, like the bar panel
URGENT = (0xE0, 0x5A, 0x5A)

# Desktop commands run as the user. Absolute paths, plus PATH/OMARCHY_PATH
# fallbacks in spawn(), in case the service started before the session env.
OMARCHY_BIN = "/usr/share/omarchy/bin"
USAGE_TAP = [f"{OMARCHY_BIN}/omarchy-shell", "-q", "omarchy.agents", "toggle"]

# Fn layer: brightness and volume keys, laid out after Esc. Same commands as
# the keyboard's media keys in Omarchy's Hyprland bindings.
FN_KEY_WIDTH = 140
FN_GROUP_GAP = 40                # space between the brightness and volume groups
REPEAT_DELAY = 0.4               # holding a key repeats after this ...
REPEAT_INTERVAL = 0.12           # ... every this many seconds
FN_KEYS = [
    # (name, glyph, glyph px, command, repeats, starts a new group)
    ("brightness-down", "\U000F00DE", 20, [f"{OMARCHY_BIN}/omarchy-brightness-display", "5%-"], True, False),
    ("brightness-up", "\U000F00DE", 32, [f"{OMARCHY_BIN}/omarchy-brightness-display", "+5%"], True, False),
    ("mute", "", 30, [f"{OMARCHY_BIN}/omarchy-audio-output-volume", "mute-toggle"], False, True),
    ("volume-down", "", 30, [f"{OMARCHY_BIN}/omarchy-audio-output-volume", "lower"], True, False),
    ("volume-up", "", 30, [f"{OMARCHY_BIN}/omarchy-audio-output-volume", "raise"], True, False),
]

# The panel reports 2170 px along the bar, but only the first 2060 light up
# (same with the stock renderer; measured 2026-10-04). Lay out within this.
VISIBLE_WIDTH = 2060

# Orientation of the logical (landscape) canvas relative to the buffer.
# Flip these if Esc shows up on the wrong end or the label is mirrored.
# Cairo drawing (everything but Esc and the overlays) assumes no flips.
FLIP_X = False
FLIP_Y = False

# Diagnostic overlay: ticks along the bar every 100 px (white, half height),
# every 500 px (yellow, full height), a marker under each finger, touch logging.
TEST_PATTERN = False

# Red 2 px outline around the visible area (0..VISIBLE_WIDTH).
SHOW_BORDER = False

# 5x7 glyphs for the Esc label
GLYPHS = {
    "e": ["     ", "     ", " ### ", "#   #", "#####", "#    ", " ### "],
    "s": ["     ", "     ", " ####", "#    ", " ### ", "    #", "#### "],
    "c": ["     ", "     ", " ### ", "#    ", "#    ", "#   #", " ### "],
}


def log(*args):
    print("touchbar-renderer:", *args, file=sys.stderr, flush=True)


class Button:
    """A touch target. `layer` is "base", "fn" (only while Fn is held) or "all"."""

    def __init__(self, name, rect, draw, press, layer="all", repeats=False):
        self.name, self.rect, self.draw, self.press = name, rect, draw, press
        self.layer, self.repeats = layer, repeats

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
        self.owner = {}              # contact id -> Button it first touched
        self.repeat_at = {}          # contact id -> monotonic time of next repeat
        self.last_input = time.monotonic()
        self.dimmed = False
        self.fn = False              # Fn key held
        self.logged_contacts = set()
        self.touch_marks = []        # logical x of current touches (test pattern)
        self.usage_mtime = None
        self.limits = []             # [(label, fraction used)] from the usage record

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
        self.buttons = self.layout()

    def add_surface(self, buf_id):
        # Cairo's RGB24 is the same little-endian XRGB8888 the service wants.
        if cairo.ImageSurface.format_stride_for_width(cairo.FORMAT_RGB24, self.w) == self.stride:
            self.surfaces[buf_id] = cairo.ImageSurface.create_for_data(
                self.buffers[buf_id], cairo.FORMAT_RGB24, self.w, self.h, self.stride)

    def layout(self):
        buttons = [Button("esc", (0, 0, ESC_WIDTH, self.short), None,
                          lambda: self.send(TAP_KEYS, struct.pack("<B3xHHHH", 1, KEY_ESC, 0, 0, 0),
                                            what="tap esc"))]
        x = ESC_WIDTH
        for name, glyph, size, argv, repeats, new_group in FN_KEYS:
            if new_group:
                x += FN_GROUP_GAP
            buttons.append(Button(name, (x, 0, x + FN_KEY_WIDTH, self.short),
                                  lambda cr, b, g=glyph, size=size: self.draw_glyph_key(cr, b, g, size),
                                  lambda argv=argv: self.spawn(argv),
                                  layer="fn", repeats=repeats))
            x += FN_KEY_WIDTH
        buttons.append(Button("usage", self.usage_rect(), self.draw_usage,
                              lambda: self.spawn(USAGE_TAP), layer="base"))
        return buttons

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
        """Fn keys stay up while a finger is still on one, even if Fn is let go."""
        return self.fn or any(b.layer == "fn" for b in self.owner.values())

    def visible_buttons(self):
        hidden = "base" if self.fn_layer() else "fn"
        return [b for b in self.buttons if b.layer != hidden]

    def pressed(self, button):
        return button in self.owner.values()

    # --- drawing ------------------------------------------------------------
    def draw(self, buf):
        if DEBUG_BG:
            bg = BG_DEBUG_FN if self.fn else BG_DEBUG
        else:
            bg = BG
        self.fill(buf, 0, 0, self.long, self.short, bg)
        buttons = self.visible_buttons()
        for b in buttons:
            if b.name == "esc":
                self.draw_esc(buf, b)
        surface = self.surfaces.get(self.drawing)
        if surface and not self.portrait and not FLIP_X and not FLIP_Y:
            surface.mark_dirty()     # we wrote pixels behind cairo's back
            cr = cairo.Context(surface)
            cr.select_font_face(FONT)
            for b in buttons:
                if b.draw:
                    b.draw(cr, b)
            surface.flush()
        if TEST_PATTERN:
            self.draw_test_pattern(buf)
        if SHOW_BORDER or TEST_PATTERN:
            self.draw_border(buf)

    def draw_esc(self, buf, b):
        x0, y0, x1, y1 = b.rect
        pad = KEY_PAD
        self.fill(buf, x0 + pad, y0 + pad, x1 - pad, y1 - pad,
                  ESC_PRESSED if self.pressed(b) else ESC_BG)
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
                        self.fill(buf, lx, ly, lx + scale, ly + scale, ESC_FG)

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

    def draw_key_face(self, cr, b):
        x0, y0, x1, y1 = b.rect
        p = KEY_PAD
        cr.rectangle(x0 + p, y0 + p, x1 - x0 - 2 * p, y1 - y0 - 2 * p)
        cr.set_source_rgba(*self.color(ESC_PRESSED if self.pressed(b) else ESC_BG))
        cr.fill()

    def draw_glyph(self, cr, glyph, cx, cy, size):
        cr.set_font_size(size)
        ext = cr.text_extents(glyph)
        cr.move_to(cx - ext.width / 2 - ext.x_bearing, cy - ext.height / 2 - ext.y_bearing)
        cr.set_source_rgba(*self.color(ESC_FG))
        cr.show_text(glyph)

    def draw_glyph_key(self, cr, b, glyph, size):
        self.draw_key_face(cr, b)
        x0, y0, x1, y1 = b.rect
        self.draw_glyph(cr, glyph, (x0 + x1) / 2, (y0 + y1) / 2, size)

    def usage_limits(self):
        return self.limits or [("Session", None), ("Weekly", None)]

    def usage_rect(self):
        n = len(self.usage_limits())
        width = 14 + 30 + 14 + n * METER_WIDTH + (n - 1) * 24 + 16 + 2 * KEY_PAD
        return self.visible - width, 0, self.visible, self.short

    def draw_usage(self, cr, b):
        """Key-style container: agents icon, then one meter per limit."""
        gap, icon_w = 24, 30
        self.draw_key_face(cr, b)
        x0 = b.rect[0] + KEY_PAD
        white = (0xFF, 0xFF, 0xFF)
        self.draw_glyph(cr, USAGE_ICON, x0 + 14 + icon_w / 2, self.short / 2, 30)

        cr.set_font_size(15)
        x = x0 + 14 + icon_w + 14
        for label, frac in self.usage_limits():
            alarm = frac is not None and frac >= METER_ALARM
            cr.set_source_rgba(*self.color(white))
            cr.move_to(x, 25)
            cr.show_text(label)
            pct = "—" if frac is None else f"{round(frac * 100)}%"
            ext = cr.text_extents(pct)
            cr.set_source_rgba(*self.color(URGENT if alarm else white))
            cr.move_to(x + METER_WIDTH - ext.x_advance, 25)
            cr.show_text(pct)

            self.rounded_rect(cr, x, 33, METER_WIDTH, 10, 5)
            cr.set_source_rgba(*self.color(white, 0.2))
            cr.fill()
            if frac:
                fw = max(10, METER_WIDTH * min(frac, 1.0))
                self.rounded_rect(cr, x, 33, fw, 10, 5)
                cr.set_source_rgba(*self.color(URGENT if alarm else white))
                cr.fill()
            x += METER_WIDTH + gap

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

    # --- usage record -------------------------------------------------------
    def poll_usage(self):
        try:
            mtime = os.stat(USAGE_FILE).st_mtime_ns
        except OSError:
            mtime = None
        if mtime == self.usage_mtime:
            return
        self.usage_mtime = mtime
        limits = []
        try:
            with open(USAGE_FILE) as f:
                record = json.load(f)
            for entry in record.get("limits") or []:
                frac = float(entry.get("percent", -1))
                if frac >= 0:
                    label = str(entry.get("label", "")).split(" (")[0] or "Limit"
                    limits.append((label, frac))
        except (OSError, ValueError, TypeError, AttributeError) as e:
            if mtime is not None:
                log(f"unreadable usage record: {e}")
        if limits != self.limits:
            self.limits = limits
            self.relayout()
            self.dirty = True

    def relayout(self):
        """Rebuild buttons (the usage widget's width follows the limit count),
        keeping each finger attached to the rebuilt version of its button."""
        old = {id(b): b.name for b in self.buttons}
        self.buttons = self.layout()
        by_name = {b.name: b for b in self.buttons}
        self.owner = {cid: by_name[old[id(b)]] for cid, b in self.owner.items()}

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
            if TEST_PATTERN and cid not in self.logged_contacts:
                log(f"touch down id={cid} x={lx} y={ly}")
            if cid in self.owner:
                continue
            button = next((b for b in self.visible_buttons() if b.hit(lx, ly)), None)
            if button:
                self.owner[cid] = button
                button.press()
                if button.repeats:
                    self.repeat_at[cid] = now + REPEAT_DELAY

        if TEST_PATTERN:
            self.logged_contacts = set(touching)
            marks = [lx for lx, _ in touching.values()]
            if marks != self.touch_marks:
                self.touch_marks = marks
                self.dirty = True
        after = (self.fn, self.fn_layer(), set(self.owner.values()))
        if after[1:] != before[1:] or (DEBUG_BG and after[0] != before[0]):
            self.dirty = True
        if count:
            self.last_input = now
            if self.dimmed:
                self.dimmed = False
                self.dirty = True

    def run_repeats(self, now):
        for cid, due in list(self.repeat_at.items()):
            if now >= due:
                self.owner[cid].press()
                self.repeat_at[cid] = max(due + REPEAT_INTERVAL, now)

    def spawn(self, argv):
        """Run a desktop command without blocking the render loop."""
        env = dict(os.environ)
        env.setdefault("OMARCHY_PATH", "/usr/share/omarchy")
        if OMARCHY_BIN not in env.get("PATH", "").split(":"):
            env["PATH"] = OMARCHY_BIN + ":" + env.get("PATH", "/usr/bin")
        try:
            subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             start_new_session=True)
        except OSError as e:
            log(f"could not run {argv[0]}: {e}")

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
                self.poll_usage()
                next_poll = now + USAGE_POLL_SECONDS
            self.run_repeats(now)
            if IDLE_SECONDS > 0 and not self.dimmed and now >= self.last_input + IDLE_SECONDS:
                self.dimmed = True
                self.dirty = True
            self.present()
            deadlines = [next_poll, *self.repeat_at.values()]
            if IDLE_SECONDS > 0 and not self.dimmed:
                deadlines.append(self.last_input + IDLE_SECONDS)
            timeout = max(0.0, min(deadlines) - now)
            ready, _, _ = select.select([self.sock], [], [], timeout)
            if ready:
                self.handle(*self.recv())


if __name__ == "__main__":
    signal.signal(signal.SIGCHLD, signal.SIG_IGN)   # auto-reap spawned commands
    try:
        Renderer().run()
    except KeyboardInterrupt:
        pass
