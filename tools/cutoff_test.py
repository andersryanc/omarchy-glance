#!/usr/bin/env python3
"""Probe why only the first ~2060 of 2170 Touch Bar pixels light up.

Run with the normal renderer stopped (systemctl --user stop t1-touchbar).
Each phase holds for HOLD seconds; watch the right-hand end of the bar.

  phase 1  full frame: dark blue up to x=2000, then bands every 20 px
           (red/green/blue/white/yellow/cyan) to 2170, then a magenta
           strip 2160-2170. Shows where a full-frame update stops.
  phase 2  partial update, damage x=2000..2170 only, all yellow.
  phase 3  partial update, damage x=2060..2170 only, all green.
  phase 4  full frame again, all white.

If phases 2/3 light pixels past 2060 that phase 1 didn't, large transfers
are being truncated (driver or T1), not the panel being short.
"""
import array, mmap, os, socket, struct, sys, time, fcntl

SOCK = "/run/t1bridge/touchbar.sock"
HDR = struct.Struct("<4sHHII")
HELLO, REGISTER, SUBMIT = 1, 2, 3
ACK, ERROR, RELEASED = 0x8002, 0x8003, 0x9001
HOLD = float(sys.argv[1]) if len(sys.argv) > 1 else 6

sock = socket.socket(socket.AF_UNIX, socket.SOCK_SEQPACKET)
sock.connect(SOCK)
req = 0


def send(t, payload=b"", fds=None):
    global req
    req += 1
    pkt = HDR.pack(b"T1HW", 1, t, len(payload), req) + payload
    if fds:
        sock.sendmsg([pkt], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, array.array("i", fds))])
    else:
        sock.send(pkt)


def recv():
    data = sock.recv(65536)
    _, _, t, n, r = HDR.unpack_from(data)
    return t, data[HDR.size:HDR.size + n]


send(HELLO, struct.pack("<HHQ", 0, 0, 0x01))
t, p = recv()
_, _, W, H, _, maxbuf = struct.unpack("<HHIIII", p)
print(f"geometry {W}x{H}, max buffers {maxbuf}", flush=True)
stride, size = W * 4, W * 4 * H

bufs = {}
for bid in (1, 2):
    fd = os.memfd_create(f"cutoff-{bid}", os.MFD_CLOEXEC | os.MFD_ALLOW_SEALING)
    os.ftruncate(fd, size)
    fcntl.fcntl(fd, fcntl.F_ADD_SEALS, fcntl.F_SEAL_SHRINK | fcntl.F_SEAL_GROW | fcntl.F_SEAL_SEAL)
    bufs[bid] = mmap.mmap(fd, size)
    send(REGISTER, struct.pack("<IIQ", bid, stride, size), [fd])
    os.close(fd)
    t, p = recv()
    assert t == ACK, (t, p)

BANDS = [0xFF0000, 0x00FF00, 0x0000FF, 0xFFFFFF, 0xFFFF00, 0x00FFFF]


def fill(buf, x0, x1, rgb):
    px = struct.pack("<I", rgb) * (x1 - x0)
    for y in range(H):
        buf[y * stride + x0 * 4:y * stride + x1 * 4] = px


frame = 0
current = 2


def submit(rects, painter, label):
    global frame, current
    nxt = 1 if current == 2 else 2
    buf = bufs[nxt]
    buf[:] = bufs[current]          # start from the previous frame
    painter(buf)
    frame += 1
    payload = struct.pack("<IQI", nxt, frame, len(rects))
    payload += b"".join(struct.pack("<IIII", *r) for r in rects)
    send(SUBMIT, payload)
    while True:
        t, p = recv()
        if t == ERROR:
            print("  error", struct.unpack("<I", p)[0], flush=True)
            break
        if t == RELEASED:
            break
    current = nxt
    print(label, flush=True)
    time.sleep(HOLD)


def phase1(buf):
    fill(buf, 0, 2000, 0x103060)
    for i, x in enumerate(range(2000, W, 20)):
        fill(buf, x, min(x + 20, W), BANDS[i % len(BANDS)])
    fill(buf, W - 10, W, 0xFF00FF)


submit([], phase1, "phase 1: full frame, bands from 2000 (R G B W Y C ...), magenta last 10 px")
submit([(2000, 0, W - 2000, H)], lambda b: fill(b, 2000, W, 0xFFFF00),
       "phase 2: partial update 2000..end, yellow")
submit([(2060, 0, W - 2060, H)], lambda b: fill(b, 2060, W, 0x00FF00),
       "phase 3: partial update 2060..end, green")
submit([], lambda b: fill(b, 0, W, 0xFFFFFF), "phase 4: full frame, white")
print("done", flush=True)
