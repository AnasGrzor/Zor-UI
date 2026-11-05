#!/usr/bin/env python3
"""
Zor UI - Material You theme generator.

Generates a Material 3 color scheme from a wallpaper (or a hex color) and
applies it to every part of the Windows setup:

  * Zebar widgets      -> ~/.glzr/zebar/zor-ui/theme.json (widgets poll it)
  * GlazeWM borders    -> ~/.glzr/glazewm/config.yaml (marked block) + reload
  * Flow Launcher      -> %APPDATA%/FlowLauncher/Themes/zor-ui.xaml
  * Windows Terminal   -> fragment with an "zor-ui" color scheme

Color math is adapted from end4-pC's scripts/colors/generate_colors_material.py
(GPL-3.0, (c) end-4 / pctrade) so the palettes match end4-pC exactly.
"""
import argparse
import json
import math
import os
import re
import subprocess
import sys
from pathlib import Path

from PIL import Image
from materialyoucolor.quantize import QuantizeCelebi
from materialyoucolor.score.score import Score
from materialyoucolor.hct import Hct
from materialyoucolor.dynamiccolor.material_dynamic_colors import MaterialDynamicColors
from materialyoucolor.utils.color_utils import rgba_from_argb, argb_from_rgb
from materialyoucolor.utils.math_utils import (
    sanitize_degrees_double, difference_degrees, rotation_direction)

HERE = Path(__file__).resolve().parent
HOME = Path.home()
APPDATA = Path(os.environ.get("APPDATA", HOME / "AppData" / "Roaming"))
LOCALAPPDATA = Path(os.environ.get("LOCALAPPDATA", HOME / "AppData" / "Local"))

SCHEMES = {
    "content": "scheme_content.SchemeContent",
    "expressive": "scheme_expressive.SchemeExpressive",
    "fidelity": "scheme_fidelity.SchemeFidelity",
    "fruit-salad": "scheme_fruit_salad.SchemeFruitSalad",
    "monochrome": "scheme_monochrome.SchemeMonochrome",
    "neutral": "scheme_neutral.SchemeNeutral",
    "rainbow": "scheme_rainbow.SchemeRainbow",
    "tonal-spot": "scheme_tonal_spot.SchemeTonalSpot",
    "vibrant": "scheme_vibrant.SchemeVibrant",
}


# ----------------------------------------------------------------- color math
def argb_to_hex(argb):
    return "#{:02X}{:02X}{:02X}".format(*map(round, rgba_from_argb(argb)))


def hex_to_argb(h):
    h = h.lstrip("#")
    return argb_from_rgb(int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16))


def harmonize(design, source, threshold=35, harmony=0.5):
    a, b = Hct.from_int(design), Hct.from_int(source)
    rot = min(difference_degrees(a.hue, b.hue) * harmony, threshold)
    hue = sanitize_degrees_double(a.hue + rot * rotation_direction(a.hue, b.hue))
    return Hct.from_hct(hue, a.chroma, a.tone).to_int()


def boost(argb, chroma=1.0, tone=1.0):
    h = Hct.from_int(argb)
    return Hct.from_hct(h.hue, h.chroma * chroma, h.tone * tone).to_int()


def source_from_image(path, size=128):
    img = Image.open(path)
    if getattr(img, "is_animated", False):
        img.seek(1 if img.n_frames > 1 else 0)
    img = img.convert("RGB")
    w, h = img.size
    if w * h > size * size:
        s = math.sqrt(size * size / (w * h))
        img = img.resize((max(1, round(w * s)), max(1, round(h * s))), Image.Resampling.BICUBIC)
    pixels = img.get_flattened_data() if hasattr(img, "get_flattened_data") else img.getdata()
    return Score.score(QuantizeCelebi(list(pixels), 128))[0]


def load_scheme_class(name):
    mod, cls = SCHEMES.get(name, SCHEMES["tonal-spot"]).split(".")
    module = __import__(f"materialyoucolor.scheme.{mod}", fromlist=[cls])
    return getattr(module, cls)


def generate(source_argb, dark, scheme_name, smart):
    hct = Hct.from_int(source_argb)
    if smart and hct.chroma < 20:
        scheme_name = "neutral"
    scheme = load_scheme_class(scheme_name)(hct, dark, 0.0)

    colors = {}
    for attr in vars(MaterialDynamicColors).keys():
        dyn = getattr(MaterialDynamicColors, attr)
        if hasattr(dyn, "get_hct"):
            r, g, b, _ = dyn.get_hct(scheme).to_rgba()
            colors[attr] = "#{:02X}{:02X}{:02X}".format(r, g, b)

    # Normalise the key-color name across materialyoucolor 2.x / 3.x.
    if "primary_paletteKeyColor" in colors:
        colors.setdefault("primaryPaletteKeyColor", colors["primary_paletteKeyColor"])

    # end4's extended "success" palette.
    colors.update({
        "success": "#B5CCBA", "onSuccess": "#213528",
        "successContainer": "#374B3E", "onSuccessContainer": "#D1E9D6",
    } if dark else {
        "success": "#4F6354", "onSuccess": "#FFFFFF",
        "successContainer": "#D1E8D5", "onSuccessContainer": "#0C1F13",
    })

    # Terminal colors, harmonised toward the primary key color (end4 defaults).
    base = json.loads((HERE / "terminal" / "scheme-base.json").read_text())["dark" if dark else "light"]
    key = hex_to_argb(colors.get("primaryPaletteKeyColor", colors["primary"]))
    term = {}
    for name, val in base.items():
        if scheme_name == "monochrome":
            term[name] = val
            continue
        c = harmonize(hex_to_argb(val), key, 100, 0.8)
        c = boost(c, 1, 1 + 0.35 * (1 if dark else -1))
        term[name] = argb_to_hex(c)
    return colors, term, scheme_name


# -------------------------------------------------------------------- targets
def write_thumbnail(pack_dir, image):
    """Small copy of the wallpaper for the Settings widget (webviews can't read arbitrary files)."""
    try:
        img = Image.open(image)
        if getattr(img, "is_animated", False):
            img.seek(0)
        img = img.convert("RGB")
        img.thumbnail((960, 960))
        img.save(pack_dir / "wallpaper.jpg", quality=85)
    except OSError:
        pass


def write_zebar(pack_dir, payload):
    pack_dir.mkdir(parents=True, exist_ok=True)
    tmp = pack_dir / "theme.json.tmp"
    tmp.write_text(json.dumps(payload, indent=2))
    os.replace(tmp, pack_dir / "theme.json")


BEGIN, END = "# >>> zor-ui colors (managed by end4_theme.py)", "# <<< zor-ui colors"


def write_glazewm(cfg, c):
    if not cfg.is_file():
        return False
    text = cfg.read_text(encoding="utf-8")
    block = (
        f"{BEGIN}\n"
        "window_effects:\n"
        "  focused_window:\n"
        f"    border: {{ enabled: true, color: '{c['primary']}' }}\n"
        "    hide_title_bar: { enabled: false }\n"
        "    corner_style: { enabled: true, style: 'rounded' }\n"
        "    transparency: { enabled: false, opacity: '95%' }\n"
        "  other_windows:\n"
        f"    border: {{ enabled: true, color: '{c['outlineVariant']}' }}\n"
        "    hide_title_bar: { enabled: false }\n"
        "    corner_style: { enabled: true, style: 'rounded' }\n"
        "    transparency: { enabled: false, opacity: '92%' }\n"
        f"{END}"
    )
    pattern = re.compile(re.escape(BEGIN) + r".*?" + re.escape(END), re.S)
    if not pattern.search(text):
        return False
    cfg.write_text(pattern.sub(lambda _: block, text), encoding="utf-8")
    return True


def write_flow(template, themes_dir, c, dark):
    if not template.is_file():
        return False
    themes_dir.mkdir(parents=True, exist_ok=True)
    out = template.read_text(encoding="utf-8")
    for k, v in {
        "BG": c["surfaceContainerLow"], "BORDER": c["outlineVariant"],
        "FG": c["onSurface"], "SUBFG": c["onSurfaceVariant"],
        "SELBG": c["secondaryContainer"], "SELFG": c["onSecondaryContainer"],
        "PRIMARY": c["primary"], "OUTLINE": c["outline"],
        "COLORSCHEME": "Dark" if dark else "Light",
    }.items():
        out = out.replace("{{" + k + "}}", v)
    (themes_dir / "zor-ui.xaml").write_text(out, encoding="utf-8")
    return True


def write_terminal(frag_dir, c, t):
    frag_dir.mkdir(parents=True, exist_ok=True)
    names = ["black", "red", "green", "yellow", "blue", "purple", "cyan", "white"]
    scheme = {"name": "zor-ui", "background": c["surface"], "foreground": c["onSurface"],
              "cursorColor": c["primary"], "selectionBackground": c["secondaryContainer"]}
    for i, n in enumerate(names):
        scheme[n] = t[f"term{i}"]
        scheme["bright" + n.capitalize()] = t[f"term{i + 8}"]
    (frag_dir / "zor-ui.json").write_text(json.dumps({"schemes": [scheme]}, indent=2))


def reload_glazewm():
    import shutil
    pf = Path(os.environ.get("ProgramFiles", r"C:\Program Files")) / "glzr.io" / "GlazeWM"
    exe = shutil.which("glazewm") or next(
        (str(p) for p in (pf / "cli" / "glazewm.exe", pf / "glazewm.exe") if p.exists()), None)
    if not exe:
        return
    try:
        subprocess.run([exe, "command", "wm-reload-config"], timeout=10, capture_output=True,
                       creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    except (OSError, subprocess.SubprocessError):
        pass


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    src = ap.add_mutually_exclusive_group()
    src.add_argument("--image", help="wallpaper to extract colors from")
    src.add_argument("--color", help="seed color, e.g. #6750A4")
    ap.add_argument("--mode", choices=["dark", "light"], help="default: last used, else dark")
    ap.add_argument("--scheme", choices=["auto", *SCHEMES], help="default: last used, else auto")
    ap.add_argument("--state", default=str(LOCALAPPDATA / "zor-ui" / "state.json"))
    ap.add_argument("--pack-dir", default=str(HOME / ".glzr" / "zebar" / "zor-ui"))
    ap.add_argument("--glazewm-config", default=str(HOME / ".glzr" / "glazewm" / "config.yaml"))
    ap.add_argument("--flow-themes", default=str(APPDATA / "FlowLauncher" / "Themes"))
    ap.add_argument("--terminal-fragments", default=str(
        LOCALAPPDATA / "Microsoft" / "Windows Terminal" / "Fragments" / "zor-ui"))
    ap.add_argument("--no-reload", action="store_true", help="don't ask GlazeWM to reload")
    ap.add_argument("--live", help="video used as a live wallpaper (--image is a frame from it)")
    ap.add_argument("--thumb-from", help=argparse.SUPPRESS)
    args = ap.parse_args()

    state_path = Path(args.state)
    state = json.loads(state_path.read_text()) if state_path.exists() else {}
    mode = args.mode or state.get("mode", "dark")
    scheme = args.scheme or state.get("scheme", "auto")
    image, color = args.image, args.color
    if not image and not color:  # re-theme from the last source (e.g. dark/light toggle)
        image, color = state.get("image"), state.get("color")
    if image and not Path(image).exists():
        image = None
    if not image and not color:
        color = "#6750A4"

    seed = source_from_image(image) if image else hex_to_argb(color)
    smart = scheme == "auto"
    colors, term, used = generate(seed, mode == "dark", "tonal-spot" if smart else scheme, smart)

    live = args.live if args.live else (state.get("live") if not args.image and not args.color else None)
    payload = {"mode": mode, "scheme": scheme, "resolvedScheme": used,
               "seed": argb_to_hex(seed), "wallpaper": image, "live": live, "colors": colors, "term": term}
    write_zebar(Path(args.pack_dir), payload)
    if image:
        write_thumbnail(Path(args.pack_dir), args.thumb_from or image)
    glaze_ok = write_glazewm(Path(args.glazewm_config), colors)
    flow_ok = write_flow(HERE.parent / "flowlauncher" / "zor-ui.xaml.template", Path(args.flow_themes), colors, mode == "dark")
    write_terminal(Path(args.terminal_fragments), colors, term)

    state.update({"mode": mode, "scheme": scheme, "image": image if image else None,
                  "color": None if image else color, "live": live})
    state_path.parent.mkdir(parents=True, exist_ok=True)
    state_path.write_text(json.dumps(state, indent=2))

    if glaze_ok and not args.no_reload:
        reload_glazewm()

    print(json.dumps({"ok": True, "mode": mode, "scheme": used, "seed": payload["seed"],
                      "primary": colors["primary"], "glazewm": glaze_ok, "flowLauncher": flow_ok}))


if __name__ == "__main__":
    sys.exit(main())
