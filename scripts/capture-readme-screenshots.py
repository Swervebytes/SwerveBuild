"""Capture README screenshots from a running Swerve Build window."""
from __future__ import annotations

import subprocess
import sys
import time
from ctypes import windll
from pathlib import Path

import win32con
import win32gui
import win32ui
from PIL import Image

ROOT = Path(__file__).resolve().parents[1]
DOCS = ROOT / "docs"
NAV = ROOT / "scripts" / "nav-swerve.ps1"
TITLE = "Swerve Build"
SETTLE_S = 0.9
OUTPUT_WIDTH = 1100


def nav(page: str) -> None:
    subprocess.run(
        [
            "powershell.exe",
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            str(NAV),
            "-Page",
            page,
        ],
        check=True,
    )


def capture(path: Path) -> None:
    hwnd = win32gui.FindWindow(None, TITLE)
    if not hwnd:
        raise RuntimeError(f"Window '{TITLE}' not found")

    win32gui.ShowWindow(hwnd, win32con.SW_RESTORE)
    win32gui.SetForegroundWindow(hwnd)
    time.sleep(SETTLE_S)

    left, top, right, bottom = win32gui.GetWindowRect(hwnd)
    width = right - left
    height = bottom - top

    hwnd_dc = win32gui.GetWindowDC(hwnd)
    mfc_dc = win32ui.CreateDCFromHandle(hwnd_dc)
    save_dc = mfc_dc.CreateCompatibleDC()
    bitmap = win32ui.CreateBitmap()
    bitmap.CreateCompatibleBitmap(mfc_dc, width, height)
    save_dc.SelectObject(bitmap)

    # PW_RENDERFULLCONTENT = 2
    if not windll.user32.PrintWindow(hwnd, save_dc.GetSafeHdc(), 2):
        raise RuntimeError("PrintWindow failed")

    bmpinfo = bitmap.GetInfo()
    bmpstr = bitmap.GetBitmapBits(True)
    image = Image.frombuffer(
        "RGB",
        (bmpinfo["bmWidth"], bmpinfo["bmHeight"]),
        bmpstr,
        "raw",
        "BGRX",
        0,
        1,
    )

    if image.width != OUTPUT_WIDTH:
        ratio = OUTPUT_WIDTH / image.width
        size = (OUTPUT_WIDTH, max(1, round(image.height * ratio)))
        image = image.resize(size, Image.Resampling.LANCZOS)

    path.parent.mkdir(parents=True, exist_ok=True)
    image.save(path, "PNG", optimize=True)
    print(f"Saved {path} ({image.width}x{image.height})")

    win32gui.DeleteObject(bitmap.GetHandle())
    save_dc.DeleteDC()
    mfc_dc.DeleteDC()
    win32gui.ReleaseDC(hwnd, hwnd_dc)


def main() -> int:
    for page, filename in (("Home", "screenshot-home.png"), ("Settings", "screenshot-settings.png")):
        nav(page)
        time.sleep(0.5)
        capture(DOCS / filename)
    print("README screenshots updated in docs/")
    return 0


if __name__ == "__main__":
    sys.exit(main())