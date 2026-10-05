#!/usr/bin/env python3
"""Make the T1 draw its own built-in Esc/F-key layout, with no frames from the host.

Used to locate the 2060 px cutoff: the T1's own layout (keyboard.plist in its OS
image) ends F12 at x=2170. If F12 shows in full here, the cutoff is in the host
frame path (appletbdrm/t1bridge); if it's cut at the same spot, it's iBoot or the panel.

This is what the old apple-ib-tb driver (macbook12-spi-driver) did, done from
userspace: in USB configuration 1 the T1 exposes a HID keyboard whose "mode"
report picks a built-in layout, and a second HID interface whose display report
turns the panel on. Standard library only.

    t1_native_keys.py status              # read-only: config, interfaces, HID reports
    sudo t1_native_keys.py config 1       # switch the T1 to configuration 1 (t1bridge stopped first!)
    sudo t1_native_keys.py show fn        # display on + Esc/F1-F12 layout
    sudo t1_native_keys.py show special   # display on + Esc/brightness/media layout
    sudo t1_native_keys.py mode esc|fn|special|off
    sudo t1_native_keys.py disp on|dim|off

Undo: reboot (t1_cfgsel picks configuration 2 again at enumeration).
"""
import ctypes
import fcntl
import glob
import os
import struct
import sys

VID, PID = "05ac", "8600"
MODES = {"esc": 0, "fn": 1, "special": 2, "off": 3}
DISPS = {"on": 1, "dim": 2, "off": 4}

USAGE_GD_KEYBOARD = 0x00010006   # application of the mode report
USAGE_MODE = 0x00FF0004          # HID_UP_CUSTOM | 0x0004
USAGE_APPLE_APP = 0xFF120001     # application of the display report
USAGE_DISP = 0xFF120021

REPORT_TYPES = {8: ("input", 1), 9: ("output", 2), 11: ("feature", 3)}


def find_t1():
    for d in glob.glob("/sys/bus/usb/devices/*"):
        try:
            if (open(f"{d}/idVendor").read().strip(), open(f"{d}/idProduct").read().strip()) == (VID, PID):
                return d
        except OSError:
            pass
    sys.exit("T1 (05ac:8600) not found; is it booted?")


def read(path):
    return open(path).read().strip()


def parse_hid(desc):
    """Return [(application, report_type_name, type_code, report_id, [usages])] for every main item."""
    out, i = [], 0
    page = rid = 0
    usages, stack, app = [], [], None
    while i < len(desc):
        b = desc[i]
        if b == 0xFE:  # long item
            i += 3 + desc[i + 1]
            continue
        size = (0, 1, 2, 4)[b & 3]
        typ, tag = (b >> 2) & 3, b >> 4
        val = int.from_bytes(desc[i + 1:i + 1 + size], "little")
        i += 1 + size
        if typ == 1:  # global
            if tag == 0:
                page = val
            elif tag == 8:
                rid = val
        elif typ == 2:  # local
            if tag == 0:
                usages.append(val if size == 4 else (page << 16) | val)
        elif typ == 0:  # main
            if tag == 10:  # collection
                if not stack and val == 1:
                    app = usages[0] if usages else None
                stack.append(val)
            elif tag == 12:  # end collection
                if stack:
                    stack.pop()
            elif tag in REPORT_TYPES:
                name, code = REPORT_TYPES[tag]
                out.append((app, name, code, rid, list(usages)))
            usages = []
    return out


def hid_interfaces(t1):
    """[(ifnum, hidraw_node, parsed_reports)] for the T1's HID interfaces."""
    res = []
    for intf in sorted(glob.glob(f"{t1}:*")):
        for hr in glob.glob(f"{intf}/*/hidraw/hidraw*"):
            desc = open(f"{os.path.dirname(os.path.dirname(hr))}/report_descriptor", "rb").read()
            res.append((int(read(f"{intf}/bInterfaceNumber"), 16), "/dev/" + os.path.basename(hr), parse_hid(desc)))
    return res


def find_report(t1, app, usage):
    for ifnum, node, reports in hid_interfaces(t1):
        for a, name, code, rid, usages in reports:
            if a == app and usage in usages:
                return ifnum, node, name, code, rid
    sys.exit(f"no HID report with usage {usage:#010x} in application {app:#010x}; "
             "is the T1 in configuration 1? (run `status`)")


class CtrlTransfer(ctypes.Structure):
    _fields_ = [("bRequestType", ctypes.c_uint8), ("bRequest", ctypes.c_uint8),
                ("wValue", ctypes.c_uint16), ("wIndex", ctypes.c_uint16),
                ("wLength", ctypes.c_uint16), ("timeout", ctypes.c_uint32),
                ("data", ctypes.c_void_p)]


def _ioc(direction, typ, nr, size):
    return (direction << 30) | (size << 16) | (ord(typ) << 8) | nr


USBDEVFS_CONTROL = _ioc(3, "U", 0, ctypes.sizeof(CtrlTransfer))


def set_mode(t1, mode):
    # apple-ib-tb sends this as a *vendor* device request, not a HID class request.
    ifnum, _node, _name, code, rid = find_report(t1, USAGE_GD_KEYBOARD, USAGE_MODE)
    busnum, devnum = int(read(f"{t1}/busnum")), int(read(f"{t1}/devnum"))
    buf = ctypes.create_string_buffer(bytes([mode]), 1)
    ct = CtrlTransfer(0x40, 0x09, (code << 8) | rid, ifnum, 1, 2000, ctypes.cast(buf, ctypes.c_void_p))
    with open(f"/dev/bus/usb/{busnum:03d}/{devnum:03d}", "wb") as f:
        fcntl.ioctl(f, USBDEVFS_CONTROL, ct)
    print(f"mode {mode} sent (interface {ifnum}, report {rid})")


def set_disp(t1, disp):
    _ifnum, node, name, _code, rid = find_report(t1, USAGE_APPLE_APP, USAGE_DISP)
    report = bytes([rid, 1, disp] + [0] * 8)
    with open(node, "r+b", buffering=0) as f:
        if name == "feature":
            fcntl.ioctl(f, _ioc(3, "H", 0x06, len(report)), report)
        else:
            f.write(report)
    print(f"display {disp} sent ({name} report {rid} on {node})")


def status(t1):
    print(f"T1 at {t1}: configuration {read(f'{t1}/bConfigurationValue')} "
          f"of {read(f'{t1}/bNumConfigurations')}, power/control={read(f'{t1}/power/control')}")
    for intf in sorted(glob.glob(f"{t1}:*")):
        drv = os.path.basename(os.readlink(f"{intf}/driver")) if os.path.exists(f"{intf}/driver") else "-"
        cls = " ".join(read(f"{intf}/{k}") for k in ("bInterfaceClass", "bInterfaceSubClass", "bInterfaceProtocol"))
        print(f"  {os.path.basename(intf)}  class {cls}  driver {drv}")
    for ifnum, node, reports in hid_interfaces(t1):
        print(f"  interface {ifnum} {node}:")
        seen = []
        for app, name, _code, rid, usages in reports:
            line = f"    app {app or 0:#010x} {name:7} id {rid:3}  usages {', '.join(f'{u:#010x}' for u in usages[:6])}"
            if usages and line not in seen:
                seen.append(line)
                print(line)


def main():
    t1 = find_t1()
    args = sys.argv[1:] or ["status"]
    cmd = args[0]
    if cmd == "status":
        status(t1)
    elif cmd == "config" and len(args) == 2:
        open(f"{t1}/bConfigurationValue", "w").write(args[1])
        print(f"configuration now {read(f'{t1}/bConfigurationValue')}")
    elif cmd == "mode" and len(args) == 2 and args[1] in MODES:
        set_mode(t1, MODES[args[1]])
    elif cmd == "disp" and len(args) == 2 and args[1] in DISPS:
        set_disp(t1, DISPS[args[1]])
    elif cmd == "show" and len(args) == 2 and args[1] in MODES:
        open(f"{t1}/power/control", "w").write("on")  # the old driver kept autosuspend off too
        set_disp(t1, DISPS["on"])
        set_mode(t1, MODES[args[1]])
    else:
        sys.exit(__doc__)


if __name__ == "__main__":
    main()
