"""Microphone state for the glance.mic widget.

Tracks whether the default source is muted and whether any other app is
recording from it, by listening to `pactl subscribe` and re-reading PulseAudio
(PipeWire) state when sources or recording streams change. While another app
records and the mic is live, it opens its own small capture with `parec` to
compute levels for the waveform, and closes it as soon as that app stops, so
the mic is never held open just for the Touch Bar.
"""

import array
import collections
import json
import math
import os
import subprocess
import time

RATE = 8000                      # level-meter sample rate (mono s16le)
FLOOR_DB = -50                   # levels below this draw as silence
RESUBSCRIBE_SECONDS = 5          # retry if `pactl subscribe` dies
RECHECK_SECONDS = 5              # re-read state even without events


class Mic:
    def __init__(self, env, log, fps=20):
        self.env, self.log = env, log
        self.frame_bytes = 2 * max(1, RATE // max(1, fps))
        self.muted = None            # None until the first read
        self.in_use = False          # another app is recording from the default source
        self.levels = collections.deque(maxlen=512)   # 0-1, newest last
        self.sub = self.rec = None
        self.pcm = b""
        self.events = b""
        self.refresh_at = 0.0
        self.sub_at = 0.0
        self.changed = True          # state or levels changed since the renderer looked
        self.wanted = True           # a mic widget is on screen; levels are only captured then

    def close(self):
        for p in (self.sub, self.rec):
            if p:
                p.kill()
                p.wait()
        self.sub = self.rec = None

    # --- processes ----------------------------------------------------------
    def popen(self, args):
        return subprocess.Popen(args, env=self.env, stdin=subprocess.DEVNULL,
                                stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                                start_new_session=True)

    def pactl_json(self, *args):
        out = subprocess.run(["pactl", "-f", "json", *args], env=self.env, capture_output=True,
                             timeout=2, check=True).stdout
        return json.loads(out or b"[]")

    def tick(self, now):
        if self.sub and self.sub.poll() is not None:
            self.sub = None
        if not self.sub and now >= self.sub_at:
            self.sub_at = now + RESUBSCRIBE_SECONDS
            try:
                self.sub = self.popen(["pactl", "subscribe"])
                self.refresh_at = now
            except OSError as e:
                self.log(f"mic: could not run pactl: {e}")
        if self.rec and self.rec.poll() is not None:
            self.stop_levels()
            self.refresh_at = now
        if now >= self.refresh_at:
            self.refresh_at = now + RECHECK_SECONDS
            self.refresh()

    def set_wanted(self, wanted):
        if wanted != self.wanted:
            self.wanted = wanted
            self.refresh_at = 0.0    # start or stop the level meter now

    def deadline(self):
        return min(self.refresh_at, self.sub_at if not self.sub else float("inf"))

    def fds(self):
        handlers = {}
        if self.sub:
            handlers[self.sub.stdout.fileno()] = self.read_events
        if self.rec:
            handlers[self.rec.stdout.fileno()] = self.read_audio
        return handlers

    # --- state --------------------------------------------------------------
    def read_events(self):
        chunk = os.read(self.sub.stdout.fileno(), 65536)
        if not chunk:
            self.sub.wait()
            self.sub = None
            return
        self.events = (self.events + chunk)[-4096:]
        # Sources (mute), recording streams, and the server (default source) matter.
        if any(k in self.events for k in (b"on source", b"on server")):
            self.refresh_at = min(self.refresh_at, time.monotonic() + 0.1)
        self.events = self.events[self.events.rfind(b"\n") + 1:]

    def refresh(self):
        try:
            default = subprocess.run(["pactl", "get-default-source"], env=self.env,
                                     capture_output=True, text=True, timeout=2,
                                     check=True).stdout.strip()
            sources = self.pactl_json("list", "sources")
            outputs = self.pactl_json("list", "source-outputs")
        except (OSError, subprocess.SubprocessError, ValueError) as e:
            self.log(f"mic: could not read PulseAudio state: {e}")
            return
        source = next((s for s in sources if s.get("name") == default), None)
        muted = bool(source.get("mute")) if source else True
        mics = {s["index"] for s in sources
                if s.get("properties", {}).get("device.class") != "monitor"}
        own = str(self.rec.pid) if self.rec else None
        in_use = any(o.get("source") in mics and not o.get("corked") and not o.get("mute")
                     and o.get("properties", {}).get("application.process.id") != own
                     for o in outputs)
        if (muted, in_use) != (self.muted, self.in_use):
            self.muted, self.in_use = muted, in_use
            self.changed = True
        if in_use and not muted and source and self.wanted:
            if not self.rec:
                self.start_levels(default)
        elif self.rec:
            self.stop_levels()

    # --- level meter --------------------------------------------------------
    def start_levels(self, device):
        try:
            self.rec = self.popen(["parec", "--raw", "--format=s16le", f"--rate={RATE}",
                                   "--channels=1", "--latency-msec=20", f"--device={device}",
                                   "--client-name=omarchy-glance", "--stream-name=Touch Bar level meter"])
            self.pcm = b""
        except OSError as e:
            self.log(f"mic: could not run parec: {e}")

    def stop_levels(self):
        if self.rec:
            self.rec.kill()
            self.rec.wait()
            self.rec = None
        self.levels.clear()
        self.changed = True

    def read_audio(self):
        chunk = os.read(self.rec.stdout.fileno(), 65536)
        if not chunk:
            self.stop_levels()
            return
        self.pcm += chunk
        n = self.frame_bytes
        while len(self.pcm) >= n:
            samples = array.array("h", self.pcm[:n])
            self.pcm = self.pcm[n:]
            peak = max(1, max(samples), -min(samples))
            db = 20 * math.log10(peak / 32768)
            self.levels.append(max(0.0, min(1.0, (db - FLOOR_DB) / -FLOOR_DB)))
            self.changed = True
