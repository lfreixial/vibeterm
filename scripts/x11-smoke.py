#!/usr/bin/env python3
"""Exercise a real X11 window with targeted X events; never types into other apps.

Requires a desktop, libX11, xprop, ImageMagick, and a built debug/release binary.
No Python packages required. Artifacts live in a temporary directory.
Pass --pointer to also test mouse selection and clipboard. This temporarily moves
the pointer into the test window and restores its original position afterward.
"""
import ctypes as C
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

binary = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/vibeterm").resolve())
work = Path(tempfile.mkdtemp(prefix="vibeterm-x11-"))
x = C.CDLL("libX11.so.6")
x.XOpenDisplay.restype = C.c_void_p
x.XOpenDisplay.argtypes = [C.c_char_p]
display = x.XOpenDisplay(None)
if not display:
    raise SystemExit("An accessible X11/XWayland display is required")
window_type = C.c_ulong
x.XDefaultRootWindow.argtypes = [C.c_void_p]
x.XDefaultRootWindow.restype = window_type
root = x.XDefaultRootWindow(display)
x.XKeysymToKeycode.argtypes = [C.c_void_p, C.c_ulong]
x.XKeysymToKeycode.restype = C.c_uint
x.XkbKeycodeToKeysym.argtypes = [C.c_void_p, C.c_uint, C.c_int, C.c_int]
x.XkbKeycodeToKeysym.restype = C.c_ulong
x.XFlush.argtypes = [C.c_void_p]
x.XResizeWindow.argtypes = [C.c_void_p, window_type, C.c_uint, C.c_uint]
x.XCloseDisplay.argtypes = [C.c_void_p]
x.XRaiseWindow.argtypes = [C.c_void_p, window_type]
x.XSetInputFocus.argtypes = [C.c_void_p, window_type, C.c_int, C.c_ulong]
x.XInternAtom.argtypes = [C.c_void_p, C.c_char_p, C.c_int]
x.XInternAtom.restype = C.c_ulong
x.XQueryTree.argtypes = [C.c_void_p, window_type, C.POINTER(window_type), C.POINTER(window_type), C.POINTER(C.POINTER(window_type)), C.POINTER(C.c_uint)]
x.XFree.argtypes = [C.c_void_p]
x.XTranslateCoordinates.argtypes = [C.c_void_p, window_type, window_type, C.c_int, C.c_int, C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(window_type)]
x.XQueryPointer.argtypes = [C.c_void_p, window_type, C.POINTER(window_type), C.POINTER(window_type), C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_int), C.POINTER(C.c_uint)]
xtest = C.CDLL("libXtst.so.6")
xtest.XTestFakeMotionEvent.argtypes = [C.c_void_p, C.c_int, C.c_int, C.c_int, C.c_ulong]
xtest.XTestFakeButtonEvent.argtypes = [C.c_void_p, C.c_uint, C.c_int, C.c_ulong]
pointer_original = None

class KeyEvent(C.Structure):
    _fields_ = [("type", C.c_int), ("serial", C.c_ulong), ("send_event", C.c_int),
                ("display", C.c_void_p), ("window", window_type), ("root", window_type),
                ("subwindow", window_type), ("time", C.c_ulong), ("x", C.c_int),
                ("y", C.c_int), ("x_root", C.c_int), ("y_root", C.c_int),
                ("state", C.c_uint), ("keycode", C.c_uint), ("same_screen", C.c_int)]

class Event(C.Union):
    _fields_ = [("key", KeyEvent), ("pad", C.c_long * 24)]

class ClientMessage(C.Structure):
    _fields_ = [("type", C.c_int), ("serial", C.c_ulong), ("send_event", C.c_int),
                ("display", C.c_void_p), ("window", window_type), ("message_type", C.c_ulong),
                ("format", C.c_int), ("data", C.c_long * 5)]

x.XSendEvent.argtypes = [C.c_void_p, window_type, C.c_int, C.c_long, C.POINTER(Event)]

def wait_for(condition, description, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = condition()
        if value:
            return value
        time.sleep(0.05)
    raise AssertionError(description)

def own_window():
    listing = subprocess.check_output(["xprop", "-root", "_NET_CLIENT_LIST"], text=True)
    for candidate in listing.split("#")[-1].replace(",", " ").split():
        if not candidate.startswith("0x"):
            continue
        pid = subprocess.check_output(["xprop", "-id", candidate, "_NET_WM_PID"], text=True)
        if pid.strip().endswith("= " + str(process.pid)):
            return int(candidate, 16)

def key(symbol, modifiers=0):
    code = x.XKeysymToKeycode(display, symbol)
    if x.XkbKeycodeToKeysym(display, code, 0, 1) == symbol:
        modifiers |= 1
    for kind, mask in [(2, 1), (3, 2)]:
        event = Event()
        event.key = KeyEvent(kind, 0, 1, display, window, root, 0, 0, 30, 30, 30, 30, modifiers, code, 1)
        assert x.XSendEvent(display, window, 0, mask, C.byref(event))
    x.XFlush(display)
    time.sleep(0.004)

def type_text(text):
    for char in text:
        key(0xff0d if char == "\n" else ord(char))

def mouse(kind, px, py, button=1, state=0):
    global pointer_original
    if pointer_original is None:
        r, child, rx, ry, wx, wy, mask = window_type(), window_type(), C.c_int(), C.c_int(), C.c_int(), C.c_int(), C.c_uint()
        x.XQueryPointer(display, root, C.byref(r), C.byref(child), C.byref(rx), C.byref(ry), C.byref(wx), C.byref(wy), C.byref(mask))
        pointer_original = (rx.value, ry.value)
        x.XRaiseWindow(display, window)
        message = ClientMessage(33, 0, 1, display, window, x.XInternAtom(display, b"_NET_ACTIVE_WINDOW", 0), 32, (C.c_long * 5)(2, 0, 0, 0, 0))
        x.XSendEvent(display, root, 0, (1 << 20) | (1 << 19), C.cast(C.byref(message), C.POINTER(Event)))
        x.XSetInputFocus(display, window, 2, 0)
        x.XFlush(display)
        time.sleep(0.5)
    rx, ry, child = C.c_int(), C.c_int(), window_type()
    x.XTranslateCoordinates(display, window, root, px, py, C.byref(rx), C.byref(ry), C.byref(child))
    xtest.XTestFakeMotionEvent(display, -1, rx.value, ry.value, 0)
    x.XFlush(display)
    time.sleep(0.08)
    if kind in (4, 5):
        # Never click if another application is under the pointer.
        r, under, qx, qy, wx, wy, mask = window_type(), window_type(), C.c_int(), C.c_int(), C.c_int(), C.c_int(), C.c_uint()
        x.XQueryPointer(display, root, C.byref(r), C.byref(under), C.byref(qx), C.byref(qy), C.byref(wx), C.byref(wy), C.byref(mask))
        ancestor = window
        ancestors = {window}
        while ancestor != root:
            parent, children, count = window_type(), C.POINTER(window_type)(), C.c_uint()
            assert x.XQueryTree(display, ancestor, C.byref(r), C.byref(parent), C.byref(children), C.byref(count))
            x.XFree(children)
            ancestor = parent.value
            ancestors.add(ancestor)
        assert under.value in ancestors, "Test window is not under pointer; refusing to click"
        xtest.XTestFakeButtonEvent(display, button, kind == 4, 0)
    x.XFlush(display)
    time.sleep(0.08)

env = os.environ.copy()
env.pop("WAYLAND_DISPLAY", None)
env["WINIT_UNIX_BACKEND"] = "x11"
env["PS1"] = "$ "
env["INPUTRC"] = "/dev/null"
env["HISTFILE"] = "/dev/null"
log = (work / "diagnostics.txt").open("w")
config = work / "config.toml"
config.write_text("font_size = 15\n")
process = subprocess.Popen([binary, "--diagnostics", "--config", str(config), "-d", str(work), "-e", "/bin/bash", "--noprofile", "--norc"], env=env, stdout=log, stderr=log)
try:
    window = wait_for(own_window, "Vibeterm X11 window did not appear")
    time.sleep(0.8)
    type_text("printf keyboard-ok > keyboard.txt\n")
    wait_for(lambda: (work / "keyboard.txt").exists(), "Keyboard events did not reach Bash")
    assert (work / "keyboard.txt").read_text() == "keyboard-ok"
    print("PASS: keyboard input reaches real Bash with argument boundaries intact", flush=True)
    x.XResizeWindow(display, window, 900, 580)
    x.XFlush(display)
    time.sleep(0.5)
    type_text("stty size > size.txt\n")
    wait_for(lambda: (work / "size.txt").exists(), "Resize check did not run")
    rows, cols = map(int, (work / "size.txt").read_text().split())
    assert 1 < rows < 32 and 2 < cols < 114, (rows, cols)
    print(f"PASS: resize reaches child ({rows} rows, {cols} columns)", flush=True)
    type_text("seq 1 200 > fixture.txt\nless fixture.txt\n")
    time.sleep(0.5)
    key(0xff56)  # PageDown
    key(ord("/"))
    type_text("100\n")
    time.sleep(0.3)
    subprocess.run(["import", "-window", hex(window), str(work / "less.png")], check=True)
    key(ord("q"))
    type_text("printf tui-returned > tui.txt\n")
    wait_for(lambda: (work / "tui.txt").exists(), "less did not return to shell")
    print("PASS: less alternate screen, PageDown, search, and clean return", flush=True)
    helper = Path(binary).parent / "examples/clipboard_probe"
    if helper.exists() and "--pointer" in sys.argv:
        type_text("printf '\\033[2J\\033[Hcopyme\\r\\n'\n")
        time.sleep(0.3)
        mouse(6, 14, 24)
        mouse(4, 14, 24)
        mouse(6, 68, 24, state=256)
        mouse(5, 68, 24)
        subprocess.run(["import", "-window", hex(window), str(work / "selection.png")], check=True)
        key(ord("C"), 5)  # Ctrl Shift C
        time.sleep(0.3)
        copied = subprocess.check_output([str(helper)], env=env, text=True).strip()
        assert copied == "copyme", "Clipboard did not contain the test selection"
        primary = subprocess.check_output([str(helper), "primary"], env=env, text=True).strip()
        assert primary == "copyme", "Primary selection did not contain the test selection"
        print("PASS: drag selection, clipboard copy, and primary selection", flush=True)
        owner = subprocess.Popen([str(helper), "copy", "printf paste-ok > paste.txt"], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            assert owner.stdout.readline().strip() == "ready"
            key(ord("V"), 5)
            time.sleep(0.3)
            key(0xff0d)
            wait_for(lambda: (work / "paste.txt").exists(), "Clipboard paste did not reach Bash")
            assert (work / "paste.txt").read_text() == "paste-ok"
        finally:
            owner.communicate("done\n", timeout=3)
        print("PASS: clipboard paste through bracketed-paste shell mode", flush=True)
    else:
        print("SKIP: clipboard checks; build examples and pass --pointer to enable", flush=True)
    config.write_text("font_size = 0\n")
    key(ord("R"), 5)
    time.sleep(0.3)
    subprocess.run(["import", "-window", hex(window), str(work / "reload-error.png")], check=True)
    type_text("printf alive > reload.txt\n")
    wait_for(lambda: (work / "reload.txt").exists(), "Invalid reload killed the shell")
    print("PASS: shell survives invalid reload; inspect reload-error.png for feedback", flush=True)
    config.write_text("font_size = 15\n")
    key(ord("R"), 5)
    time.sleep(0.2)
    key(ord("g"), 4)  # Cancel any readline prefix without leaving a pending Escape.
    x.XResizeWindow(display, window, 1060, 700)
    x.XFlush(display)
    time.sleep(0.3)
    type_text("printf '\\033[2J\\033[H'; python3 " + str(Path(__file__).with_name("visual-fixture.py").resolve()) + "\n")
    time.sleep(0.5)
    subprocess.run(["import", "-window", hex(window), str(work / "terminal.png")], check=True)
    type_text("exit\n")
    assert process.wait(timeout=5) == 0
    print("PASS: shell exit closes window and reaps process", flush=True)
finally:
    print(f"Artifacts: {work}", flush=True)
    if pointer_original is not None:
        xtest.XTestFakeMotionEvent(display, -1, *pointer_original, 0)
        x.XFlush(display)
    if process.poll() is None:
        type_text("\x03")
        type_text("exit\n")
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.terminate()
            process.wait(timeout=2)
    x.XCloseDisplay(display)
    log.close()
