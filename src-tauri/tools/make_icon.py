"""Generate the application icon without any imaging dependency.

PIL is not installable here (npm and PyPI are blocked on this network), so the ICO is
written by hand: an ICONDIR, one ICONDIRENTRY per size, and a 32-bit BGRA DIB for each.
Rendering is a small analytic rasteriser with 3x3 supersampling, which is plenty for a
flat-shaded mark and keeps the edges clean at 16px.
"""

import struct
from pathlib import Path

SIZES = [16, 24, 32, 48, 64, 128, 256]

BG_TOP = (0x1B, 0x22, 0x2E)     # deep slate
BG_BOTTOM = (0x12, 0x17, 0x20)
ACCENT = (0x3D, 0xD6, 0xB0)     # teal — the drive platter
ACCENT_DIM = (0x2A, 0x93, 0x7A)
SPINDLE = (0x0E, 0x13, 0x1A)


def lerp(a, b, t):
    return tuple(round(x + (y - x) * t) for x, y in zip(a, b))


def rounded_box(x, y, w, h, r):
    """Signed coverage test for a rounded rectangle, in unit coordinates."""
    cx = min(max(x, r), w - r)
    cy = min(max(y, r), h - r)
    dx, dy = x - cx, y - cy
    return (dx * dx + dy * dy) <= r * r


def sample(px, py, n):
    """Colour at a point, or None for transparent. Coordinates are 0..n."""
    u, v = px / n, py / n

    # Card with rounded corners.
    pad = n * 0.055
    r = n * 0.20
    if not rounded_box(px - pad, py - pad, n - 2 * pad, n - 2 * pad, r):
        return None

    base = lerp(BG_TOP, BG_BOTTOM, v)

    # Disc platter, centred slightly low.
    ccx, ccy = n * 0.5, n * 0.54
    d = ((px - ccx) ** 2 + (py - ccy) ** 2) ** 0.5
    outer = n * 0.30
    inner = n * 0.205
    hub = n * 0.072

    if d <= hub:
        return SPINDLE
    if inner <= d <= outer:
        # Shade the ring so it reads as a disc rather than a flat annulus.
        t = (py - (ccy - outer)) / (2 * outer)
        return lerp(ACCENT, ACCENT_DIM, max(0.0, min(1.0, t)))

    # A bar above the platter, suggesting a drive slot / shelf.
    bx0, bx1 = n * 0.30, n * 0.70
    by0, by1 = n * 0.155, n * 0.215
    if bx0 <= px <= bx1 and by0 <= py <= by1:
        return ACCENT_DIM

    return base


def render(n, ss=3):
    """Render size n with ss x ss supersampling. Returns BGRA rows, bottom-up."""
    rows = []
    for y in range(n - 1, -1, -1):
        row = bytearray()
        for x in range(n):
            acc_r = acc_g = acc_b = acc_a = 0
            for sy in range(ss):
                for sx in range(ss):
                    px = x + (sx + 0.5) / ss
                    py = y + (sy + 0.5) / ss
                    c = sample(px, py, n)
                    if c is not None:
                        acc_r += c[0]
                        acc_g += c[1]
                        acc_b += c[2]
                        acc_a += 255
            total = ss * ss
            a = acc_a // total
            if a == 0:
                row += b"\x00\x00\x00\x00"
            else:
                # Un-weight colour by the covered samples only, so edges do not
                # darken towards black.
                covered = max(1, acc_a // 255)
                row += bytes(
                    (acc_b // covered, acc_g // covered, acc_r // covered, a)
                )
        rows.append(bytes(row))
    return b"".join(rows)


def dib(n):
    """A 32-bit BITMAPINFOHEADER DIB, as ICO requires (doubled height for the mask)."""
    header = struct.pack(
        "<IiiHHIIiiII",
        40,        # biSize
        n,         # biWidth
        n * 2,     # biHeight: XOR bitmap + AND mask
        1,         # biPlanes
        32,        # biBitCount
        0,         # BI_RGB
        n * n * 4,
        2835, 2835, 0, 0,
    )
    # The AND mask is unused for 32-bit icons but must still be present and padded to
    # 32-bit row boundaries.
    mask_stride = ((n + 31) // 32) * 4
    return header + render(n) + b"\x00" * (mask_stride * n)


def main():
    images = [dib(n) for n in SIZES]

    out = struct.pack("<HHH", 0, 1, len(SIZES))
    offset = 6 + 16 * len(SIZES)
    for n, data in zip(SIZES, images):
        out += struct.pack(
            "<BBBBHHII",
            0 if n >= 256 else n,   # 0 means 256 in the ICO header
            0 if n >= 256 else n,
            0, 0, 1, 32,
            len(data), offset,
        )
        offset += len(data)
    out += b"".join(images)

    dest = Path(__file__).resolve().parent.parent / "icons"
    dest.mkdir(exist_ok=True)
    (dest / "icon.ico").write_bytes(out)

    # Tauri also wants a PNG for non-Windows bundles and the 32x32 tray size.
    (dest / "icon.png").write_bytes(png(256))
    (dest / "32x32.png").write_bytes(png(32))
    (dest / "128x128.png").write_bytes(png(128))
    (dest / "128x128@2x.png").write_bytes(png(256))
    print(f"wrote {dest / 'icon.ico'} ({len(out)} bytes) and 4 PNGs")


def png(n):
    """Minimal PNG encoder: RGBA, no filtering, stored deflate blocks."""
    import zlib

    bgra = render(n)
    stride = n * 4
    raw = bytearray()
    # render() returns bottom-up rows; PNG is top-down.
    for y in range(n - 1, -1, -1):
        raw.append(0)  # filter: none
        row = bgra[y * stride:(y + 1) * stride]
        for i in range(0, stride, 4):
            b, g, r, a = row[i:i + 4]
            raw += bytes((r, g, b, a))

    def chunk(tag, data):
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", n, n, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


if __name__ == "__main__":
    main()
