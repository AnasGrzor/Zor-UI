#!/usr/bin/env python3
"""
end4-pC for Windows - backend for the Zebar widgets.

Zebar widgets run in a webview, so anything that needs the network without
CORS limits, Windows APIs, or long-running processes goes through here:

    end4_bridge.py translate <target> <source|auto> <text>
    end4_bridge.py booru <source> <page> [tags...]
    end4_bridge.py download-wallpaper <url> [--apply]
    end4_bridge.py open <url>
    end4_bridge.py football [--standings ID | --live | --positions]
    end4_bridge.py ai-models
    end4_bridge.py ai-chat                 (reads one JSON line on stdin, streams JSON lines)
    end4_bridge.py secret-set <name> [value]   /  secret-status
    end4_bridge.py notifications           /  notifications-dismiss <id...|all>
    end4_bridge.py nightlight on|off|status [kelvin]
    end4_bridge.py config-set <key> <json>
    end4_bridge.py presets-online          /  preset-apply-online <name>
    end4_bridge.py presets-local           /  preset-save <name>  /  preset-apply-local <name>  /  preset-delete <name>
    end4_bridge.py visualizer [bars]       (streams one line of bar heights per frame)
    end4_bridge.py video-frame <video> <out.png>
    end4_bridge.py we-status / we-list / we-apply <id|path> [monitor] / we-close
    end4_bridge.py we-sync                 (streams; re-themes when Wallpaper Engine changes wallpaper)

Every command prints exactly one JSON line unless it says it streams.
Parts of the logic follow end4-pC's services (GPL-3.0, (c) end-4 / pctrade).
"""
import base64
import json
import os
import re
import shutil
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent                                     # %LOCALAPPDATA%\end4-pC
PACK = Path(os.environ.get("END4_PACK", Path.home() / ".glzr" / "zebar" / "end4-pC"))
CONFIG = PACK / "config.json"
SECRETS = ROOT / "secrets.json"
PRESETS = ROOT / "presets"
DISMISSED = ROOT / "notifications-dismissed.json"
WALLPAPERS = Path.home() / "Pictures" / "Wallpapers"
IS_WIN = os.name == "nt"
NO_WINDOW = getattr(subprocess, "CREATE_NO_WINDOW", 0)
UA = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/150.0 Safari/537.36 end4-pC"


def out(obj):
    sys.stdout.write(json.dumps(obj, ensure_ascii=False) + "\n")
    sys.stdout.flush()


def load_json(path, default):
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return default


def save_json(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    tmp.write_text(json.dumps(data, indent=2, ensure_ascii=False), encoding="utf-8")
    os.replace(tmp, path)


def config():
    return load_json(CONFIG, {})


def http(url, data=None, headers=None, timeout=20, method=None):
    h = {"User-Agent": UA, "Accept": "application/json"}
    h.update(headers or {})
    if isinstance(data, (dict, list)):
        data = json.dumps(data).encode()
        h.setdefault("Content-Type", "application/json")
    req = urllib.request.Request(url, data=data, headers=h, method=method)
    return urllib.request.urlopen(req, timeout=timeout)


CURL = Path(os.environ.get("SystemRoot", r"C:\Windows")) / "System32" / "curl.exe"


def _curl_can_retry(e):
    """Some hosts (Google Translate, Danbooru) answer 429 or reset Python's TLS client
    but accept Windows' own TLS stack, which the built-in curl.exe uses."""
    if not (IS_WIN and CURL.is_file()):
        return False
    if isinstance(e, urllib.error.HTTPError):
        return e.code == 429
    return isinstance(getattr(e, "reason", e), ConnectionResetError)


def curl_get(url, headers=None, timeout=20, dest=None):
    """GET through curl.exe. Returns the body, or writes it to dest."""
    args = [str(CURL), "-sSfL", "--max-time", str(timeout)]
    for k, v in {"User-Agent": UA, "Accept": "application/json", **(headers or {})}.items():
        args += ["-H", f"{k}: {v}"]
    if dest:
        args += ["-o", str(dest)]
    r = subprocess.run([*args, url], capture_output=True, timeout=timeout + 5, creationflags=NO_WINDOW)
    if r.returncode:
        raise OSError(f"curl: {r.stderr.decode('utf-8', 'replace').strip()[:200]}")
    return r.stdout


def get_json(url, **kw):
    try:
        with http(url, **kw) as r:
            return json.loads(r.read().decode("utf-8"))
    except (urllib.error.URLError, ConnectionResetError) as e:
        if kw.get("data") is not None or not _curl_can_retry(e):
            raise
        return json.loads(curl_get(url, kw.get("headers"), kw.get("timeout", 20)).decode("utf-8"))


# ===================================================================== secrets
# API keys are encrypted with Windows DPAPI (tied to your Windows account), so
# they are never stored in plain text or in the webview's storage.
def _dpapi(data: bytes, protect: bool) -> bytes:
    if not IS_WIN:
        return data
    import ctypes
    from ctypes import wintypes

    class BLOB(ctypes.Structure):
        _fields_ = [("cbData", wintypes.DWORD), ("pbData", ctypes.POINTER(ctypes.c_char))]

    buf = ctypes.create_string_buffer(data, len(data))
    blob_in, blob_out = BLOB(len(data), buf), BLOB()
    fn = ctypes.windll.crypt32.CryptProtectData if protect else ctypes.windll.crypt32.CryptUnprotectData
    if protect:
        ok = fn(ctypes.byref(blob_in), "end4-pC", None, None, None, 0, ctypes.byref(blob_out))
    else:
        ok = fn(ctypes.byref(blob_in), None, None, None, None, 0, ctypes.byref(blob_out))
    if not ok:
        raise OSError("DPAPI failed")
    try:
        return ctypes.string_at(blob_out.pbData, blob_out.cbData)
    finally:
        ctypes.windll.kernel32.LocalFree(blob_out.pbData)


def secret_get(name):
    enc = load_json(SECRETS, {}).get(name)
    if not enc:
        return os.environ.get(f"END4_{name.upper()}_KEY", "")
    try:
        return _dpapi(base64.b64decode(enc), False).decode()
    except (OSError, ValueError):
        return ""


def cmd_secret_set(name, value=""):
    data = load_json(SECRETS, {})
    if value:
        data[name] = base64.b64encode(_dpapi(value.encode(), True)).decode()
    else:
        data.pop(name, None)
    save_json(SECRETS, data)
    out({"ok": True, "name": name, "set": bool(value)})


def cmd_secret_status():
    out({k: bool(secret_get(k)) for k in ("gemini", "openai", "anthropic", "mistral", "openrouter", "wallhaven")})


# ===================================================================== config
def cmd_config_set(key, value_json):
    """Sets a dotted key in config.json, e.g. ai.model "gemini-2.5-flash"."""
    if not re.fullmatch(r"[A-Za-z0-9_.]+", key):
        return out({"ok": False, "error": "bad key"})
    cfg = config()
    node = cfg
    parts = key.split(".")
    for p in parts[:-1]:
        if not isinstance(node.get(p), dict):
            node[p] = {}
        node = node[p]
    node[parts[-1]] = json.loads(value_json)
    save_json(CONFIG, cfg)
    if key.startswith("visualizer") or key == "desktopWidgets":
        sync_zebar_startup(cfg)
    out({"ok": True})


def sync_zebar_startup(cfg):
    """Keeps Zebar's startup list in line with the widgets enabled in config."""
    path = PACK.parent / "settings.json"
    s = load_json(path, {})
    keep = [c for c in s.get("startupConfigs", []) if c.get("pack") != "end4-pC"]
    mine = [{"pack": "end4-pC", "widget": "bar", "preset": "default"}]
    if cfg.get("desktopWidgets", True):
        mine.append({"pack": "end4-pC", "widget": "desktop", "preset": "default"})
    if cfg.get("visualizer", {}).get("desktop", True):
        mine.append({"pack": "end4-pC", "widget": "visualizer", "preset": "default"})
    s["startupConfigs"] = mine + keep
    save_json(path, s)


# ===================================================================== open / downloads
def cmd_open(url):
    if not re.match(r"^https?://", url):
        return out({"ok": False, "error": "only http(s) links"})
    if IS_WIN:
        os.startfile(url)  # noqa: S606 - default browser
    out({"ok": True})


def run_switchwall(*args):
    ps = ["powershell", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
          "-File", str(HERE / "switchwall.ps1"), *args]
    r = subprocess.run(ps, capture_output=True, text=True, timeout=180, creationflags=NO_WINDOW)
    lines = [l for l in r.stdout.splitlines() if l.strip()]
    try:
        return json.loads(lines[-1]) if lines else {"ok": False, "error": r.stderr.strip()[:300]}
    except ValueError:
        return {"ok": False, "error": lines[-1][:300]}


def download(url, dest_dir=WALLPAPERS):
    if not re.match(r"^https?://", url):
        raise ValueError("only http(s) links")
    dest_dir.mkdir(parents=True, exist_ok=True)
    name = re.sub(r"[^\w.\-]+", "_", urllib.parse.unquote(urllib.parse.urlparse(url).path.rsplit("/", 1)[-1]))[-120:] or "wallpaper.jpg"
    dest = dest_dir / name
    if not dest.exists():
        host = urllib.parse.urlparse(url).netloc
        headers = {"Accept": "*/*", "Referer": f"https://{host}/"}
        try:
            with http(url, headers=headers, timeout=60) as r, open(str(dest) + ".part", "wb") as f:
                shutil.copyfileobj(r, f)
        except (urllib.error.URLError, ConnectionResetError) as e:
            if not _curl_can_retry(e):
                raise
            curl_get(url, headers, 60, dest=str(dest) + ".part")
        os.replace(str(dest) + ".part", dest)
    return dest


def cmd_download_wallpaper(url, *flags):
    try:
        path = download(url)
    except Exception as e:  # noqa: BLE001
        return out({"ok": False, "error": str(e)})
    res = {"ok": True, "path": str(path)}
    if "--apply" in flags:
        res["theme"] = run_switchwall("-Image", str(path))
    out(res)


# ===================================================================== translate
def cmd_translate(target, source, *text):
    text = " ".join(text).strip()
    if not text:
        return out({"text": "", "detected": source})
    q = urllib.parse.urlencode({"client": "gtx", "sl": source or "auto", "tl": target, "dt": "t", "q": text})
    try:
        data = get_json("https://translate.googleapis.com/translate_a/single?" + q)
        result = "".join(seg[0] for seg in data[0] if seg and seg[0])
        out({"text": result, "detected": data[2] if len(data) > 2 else source})
    except Exception as e:  # noqa: BLE001
        out({"error": f"Translation failed: {e}"})


# ===================================================================== booru / online wallpapers
def _booru_tags(tags, nsfw, safe_tag):
    t = " ".join(tags).strip()
    return (t + ("" if nsfw else f" {safe_tag}")).strip()


def cmd_booru(source, page="1", *tags):
    cfg = config().get("booru", {})
    nsfw = bool(cfg.get("allowNsfw", False))
    page = max(1, int(page or 1))
    limit = 30
    items = []
    try:
        if source in ("yandere", "konachan"):
            base = "https://yande.re" if source == "yandere" else ("https://konachan.com" if nsfw else "https://konachan.net")
            url = f"{base}/post.json?" + urllib.parse.urlencode({"tags": _booru_tags(tags, nsfw, "rating:safe"), "limit": limit, "page": page})
            for it in get_json(url):
                if not it.get("file_url"):
                    continue
                items.append({"id": it["id"], "preview": it.get("preview_url"), "sample": it.get("sample_url") or it["file_url"],
                              "file": it["file_url"], "w": it.get("width"), "h": it.get("height"), "tags": it.get("tags", ""),
                              "nsfw": it.get("rating") != "s", "page": f"{base}/post/show/{it['id']}", "source": it.get("source") or ""})
        elif source == "danbooru":
            url = "https://danbooru.donmai.us/posts.json?" + urllib.parse.urlencode({"tags": _booru_tags(tags[:1] if not nsfw else tags[:2], nsfw, "rating:general"), "limit": limit, "page": page})
            for it in get_json(url):
                if not it.get("file_url") or not it.get("preview_file_url"):
                    continue
                items.append({"id": it["id"], "preview": it["preview_file_url"], "sample": it.get("large_file_url") or it["file_url"],
                              "file": it["file_url"], "w": it.get("image_width"), "h": it.get("image_height"), "tags": it.get("tag_string", ""),
                              "nsfw": it.get("rating") not in ("g", "s"), "page": f"https://danbooru.donmai.us/posts/{it['id']}", "source": it.get("source") or ""})
        elif source == "waifuim":
            params = [("PageSize", limit), ("IsNsfw", "All" if nsfw else "False"), ("Page", page)]
            params += [("IncludedTags", t.lower()) for t in tags if t]
            data = get_json("https://api.waifu.im/images?" + urllib.parse.urlencode(params))
            for it in data.get("items", data.get("images", [])):
                u = it.get("url")
                items.append({"id": it.get("id") or it.get("image_id"), "preview": it.get("sample_url") or u, "sample": u, "file": u,
                              "w": it.get("width"), "h": it.get("height"), "tags": " ".join(t.get("name", "") for t in it.get("tags", [])),
                              "nsfw": bool(it.get("isNsfw", it.get("is_nsfw"))), "page": it.get("source") or u, "source": it.get("source") or ""})
        elif source == "wallhaven":
            key = secret_get("wallhaven")
            params = {"q": " ".join(tags), "page": page, "purity": "110" if (nsfw and key) else "100",
                      "categories": "111", "sorting": "relevance" if tags else "toplist", "atleast": "1920x1080"}
            if key:
                params["apikey"] = key
            data = get_json("https://wallhaven.cc/api/v1/search?" + urllib.parse.urlencode(params))
            for it in data.get("data", []):
                items.append({"id": it["id"], "preview": it["thumbs"]["small"], "sample": it["thumbs"]["large"], "file": it["path"],
                              "w": it.get("dimension_x"), "h": it.get("dimension_y"), "tags": it.get("category", ""),
                              "nsfw": it.get("purity") != "sfw", "page": it.get("url"), "source": it.get("source") or ""})
        else:
            return out({"error": f"unknown source {source}"})
    except Exception as e:  # noqa: BLE001
        return out({"error": f"{source}: {e}"})
    if not nsfw:
        items = [i for i in items if not i["nsfw"]]
    out({"source": source, "page": page, "items": items})


# ===================================================================== football (end4's script)
def cmd_football(*args):
    import runpy
    sys.argv = [str(HERE / "football.py"), *args]
    runpy.run_path(str(HERE / "football.py"), run_name="__main__")
    sys.stdout.write("\n")


# ===================================================================== AI chat
DEFAULT_MODELS = [
    {"id": "gemini-2.5-flash", "name": "Gemini 2.5 Flash", "provider": "gemini"},
    {"id": "gemini-3-flash-preview", "name": "Gemini 3 Flash", "provider": "gemini"},
    {"id": "mistral-medium-latest", "name": "Mistral Medium", "provider": "mistral"},
    {"id": "claude-sonnet-5-5", "name": "Claude Sonnet", "provider": "anthropic"},
    {"id": "claude-haiku-5-5", "name": "Claude Haiku", "provider": "anthropic"},
    {"id": "gpt-4o-mini", "name": "GPT-4o mini", "provider": "openai"},
    {"id": "openrouter/auto", "name": "OpenRouter Auto", "provider": "openrouter"},
]
OPENAI_LIKE = {
    "openai": "https://api.openai.com/v1/chat/completions",
    "mistral": "https://api.mistral.ai/v1/chat/completions",
    "openrouter": "https://openrouter.ai/api/v1/chat/completions",
    "ollama": "http://localhost:11434/v1/chat/completions",
}


def ollama_models():
    try:
        tags = get_json("http://localhost:11434/api/tags", timeout=2)
        return [{"id": m["name"], "name": f"Ollama: {m['name']}", "provider": "ollama"} for m in tags.get("models", [])]
    except Exception:  # noqa: BLE001
        return []


def all_models():
    custom = config().get("ai", {}).get("customModels", [])
    return DEFAULT_MODELS + [m for m in custom if isinstance(m, dict) and m.get("id")] + ollama_models()


def cmd_ai_models():
    out({"models": all_models(), "keys": {k: bool(secret_get(k)) for k in ("gemini", "openai", "anthropic", "mistral", "openrouter")}})


def _sse(resp):
    """Yields parsed JSON payloads from a server-sent-events stream."""
    for raw in resp:
        line = raw.decode("utf-8", "replace").strip()
        if not line.startswith("data:"):
            continue
        data = line[5:].strip()
        if data == "[DONE]":
            return
        try:
            yield json.loads(data)
        except ValueError:
            continue


def cmd_ai_chat():
    """stdin: {"model": id, "messages": [{"role": "user"|"assistant", "content": str}], "system": str, "temperature": float}"""
    try:
        req = json.loads(sys.stdin.readline())
    except ValueError:
        return out({"error": "bad request"})
    model = next((m for m in all_models() if m["id"] == req.get("model")), None) or {"id": req.get("model"), "provider": "openai"}
    provider = model.get("provider", "openai")
    msgs = [m for m in req.get("messages", []) if m.get("content")]
    system = req.get("system") or "You are a helpful assistant inside a Windows desktop sidebar. Use Markdown. Be concise."
    temp = float(req.get("temperature", 0.5))
    key = secret_get(provider) if provider != "ollama" else ""
    if provider not in ("ollama", "custom") and not key:
        return out({"error": f"No API key for {provider}. Type /key {provider} YOUR_KEY"})
    try:
        if provider == "gemini":
            url = f"https://generativelanguage.googleapis.com/v1beta/models/{model['id']}:streamGenerateContent?alt=sse"
            body = {"contents": [{"role": "model" if m["role"] == "assistant" else "user", "parts": [{"text": m["content"]}]} for m in msgs],
                    "systemInstruction": {"parts": [{"text": system}]}, "generationConfig": {"temperature": temp}}
            with http(url, body, headers={"x-goog-api-key": key, "Accept": "text/event-stream"}, timeout=120) as r:
                for ev in _sse(r):
                    for part in (ev.get("candidates") or [{}])[0].get("content", {}).get("parts", []):
                        if part.get("text"):
                            out({"delta": part["text"]})
        elif provider == "anthropic":
            body = {"model": model["id"], "max_tokens": 4096, "system": system, "temperature": temp, "stream": True,
                    "messages": [{"role": m["role"], "content": m["content"]} for m in msgs]}
            with http("https://api.anthropic.com/v1/messages", body, timeout=120,
                      headers={"x-api-key": key, "anthropic-version": "2023-06-01", "Accept": "text/event-stream"}) as r:
                for ev in _sse(r):
                    if ev.get("type") == "content_block_delta" and ev.get("delta", {}).get("text"):
                        out({"delta": ev["delta"]["text"]})
                    elif ev.get("type") == "error":
                        out({"error": ev.get("error", {}).get("message", "error")})
        else:
            url = model.get("endpoint") or OPENAI_LIKE.get(provider, OPENAI_LIKE["openai"])
            headers = {"Accept": "text/event-stream"}
            if key:
                headers["Authorization"] = f"Bearer {key}"
            if provider == "openrouter":
                headers.update({"HTTP-Referer": "https://github.com/pctrade/end4-pC", "X-Title": "end4-pC for Windows"})
            body = {"model": model["id"], "stream": True, "temperature": temp,
                    "messages": [{"role": "system", "content": system}] + [{"role": m["role"], "content": m["content"]} for m in msgs]}
            with http(url, body, headers=headers, timeout=180) as r:
                for ev in _sse(r):
                    delta = (ev.get("choices") or [{}])[0].get("delta", {}).get("content")
                    if delta:
                        out({"delta": delta})
        out({"done": True})
    except urllib.error.HTTPError as e:
        detail = e.read().decode("utf-8", "replace")[:400]
        out({"error": f"{provider} returned {e.code}: {detail}"})
    except Exception as e:  # noqa: BLE001
        out({"error": f"{provider}: {e}"})


# ===================================================================== notifications
def _filetime_to_unix(ft):
    try:
        return int(ft) / 10_000_000 - 11_644_473_600
    except (TypeError, ValueError):
        return 0


def _pretty_app(aumid):
    a = (aumid or "").split("!")[0]
    a = re.sub(r"_[a-z0-9]{13}$", "", a)       # package family hash
    known = {"MSEdge": "Microsoft Edge", "Chrome": "Google Chrome", "Microsoft.Windows.Explorer": "File Explorer",
             "windows.immersivecontrolpanel": "Settings", "Microsoft.WindowsTerminal": "Terminal"}
    if a in known:
        return known[a]
    tail = re.split(r"[.\\]", a)[-1] if "." in a or "\\" in a else a
    tail = tail.replace(".exe", "")
    return re.sub(r"(?<=[a-z])(?=[A-Z])", " ", tail).strip() or "App"


def _toast_text(payload):
    if isinstance(payload, bytes):
        payload = payload.decode("utf-8", "replace")
    texts = [re.sub(r"<[^>]+>", "", t).strip() for t in re.findall(r"<text[^>]*>(.*?)</text>", payload or "", re.S)]
    import html
    texts = [html.unescape(t) for t in texts if t]
    return (texts[0] if texts else ""), "\n".join(texts[1:])


def read_notifications():
    """Reads Windows' notification history (the same database Action Center uses)."""
    import sqlite3
    import tempfile
    src = Path(os.environ.get("END4_WPNDB", Path(os.environ.get("LOCALAPPDATA", "")) / "Microsoft" / "Windows" / "Notifications" / "wpndatabase.db"))
    if not src.exists():
        return {"error": "Notification history not found", "items": []}
    dismissed = set(load_json(DISMISSED, []))
    with tempfile.TemporaryDirectory() as tmp:
        db = Path(tmp) / "wpn.db"
        for suffix in ("", "-wal", "-shm"):     # copy so we never lock Windows' live database
            s = Path(str(src) + suffix)
            if s.exists():
                shutil.copyfile(s, str(db) + suffix)
        con = sqlite3.connect(str(db))
        try:
            rows = con.execute(
                "SELECT n.Id, n.Payload, n.ArrivalTime, h.PrimaryId FROM Notification n "
                "LEFT JOIN NotificationHandler h ON h.RecordId = n.HandlerId "
                "WHERE n.Type = 'toast' ORDER BY n.ArrivalTime DESC LIMIT 80").fetchall()
        except sqlite3.Error as e:
            return {"error": f"Couldn't read notifications: {e}", "items": []}
        finally:
            con.close()
    items = []
    for nid, payload, arrival, aumid in rows:
        if str(nid) in dismissed:
            continue
        title, body = _toast_text(payload)
        if not title and not body:
            continue
        items.append({"id": str(nid), "app": _pretty_app(aumid), "title": title, "body": body,
                      "time": _filetime_to_unix(arrival)})
    return {"items": items}


def cmd_notifications():
    out(read_notifications())


def cmd_notifications_dismiss(*ids):
    d = set(load_json(DISMISSED, []))
    if ids == ("all",):
        d |= {i["id"] for i in read_notifications().get("items", [])}
    else:
        d |= set(ids)
    save_json(DISMISSED, sorted(d, key=lambda x: int(x) if x.isdigit() else 0)[-2000:])
    out({"ok": True})


# ===================================================================== night light (gamma ramp, like f.lux)
NIGHT_STATE = ROOT / "nightlight.json"


def _kelvin_rgb(k):
    """Tanner Helland's colour-temperature approximation, as 0..1 multipliers."""
    import math
    t = k / 100
    r = 1.0 if t <= 66 else min(1, max(0, 329.698727446 * ((t - 60) ** -0.1332047592) / 255))
    g = (99.4708025861 * math.log(t) - 161.1195681661) / 255 if t <= 66 else 288.1221695283 * ((t - 60) ** -0.0755148492) / 255
    b = 1.0 if t >= 66 else (0 if t <= 19 else (138.5177312231 * math.log(t - 10) - 305.0447927307) / 255)
    return r, min(1, max(0, g)), min(1, max(0, b))


def _set_gamma(kelvin):
    if not IS_WIN:
        return 0, 0
    import ctypes
    from ctypes import wintypes
    user32, gdi32 = ctypes.windll.user32, ctypes.windll.gdi32
    r, g, b = _kelvin_rgb(kelvin) if kelvin else (1, 1, 1)
    ramp = (wintypes.WORD * 768)()
    for i in range(256):
        for c, f in enumerate((r, g, b)):
            ramp[c * 256 + i] = min(65535, int(i * 257 * f))

    class MONITORINFOEXW(ctypes.Structure):
        _fields_ = [("cbSize", wintypes.DWORD), ("rcMonitor", wintypes.RECT), ("rcWork", wintypes.RECT),
                    ("dwFlags", wintypes.DWORD), ("szDevice", wintypes.WCHAR * 32)]

    devices = []
    proc_t = ctypes.WINFUNCTYPE(ctypes.c_int, wintypes.HMONITOR, wintypes.HDC, ctypes.POINTER(wintypes.RECT), wintypes.LPARAM)

    def cb(hmon, _hdc, _rc, _lp):
        mi = MONITORINFOEXW()
        mi.cbSize = ctypes.sizeof(mi)
        if user32.GetMonitorInfoW(hmon, ctypes.byref(mi)):
            devices.append(mi.szDevice)
        return 1

    user32.EnumDisplayMonitors(None, None, proc_t(cb), 0)
    ok = 0
    for dev in devices:
        hdc = gdi32.CreateDCW(dev, None, None, None)
        if hdc:
            ok += 1 if gdi32.SetDeviceGammaRamp(hdc, ctypes.byref(ramp)) else 0
            gdi32.DeleteDC(hdc)
    return ok, len(devices)


def cmd_nightlight(action="status", kelvin=None):
    st = load_json(NIGHT_STATE, {"on": False, "kelvin": 4500})
    k = int(kelvin or config().get("nightLight", {}).get("kelvin", st.get("kelvin", 4500)))
    if action == "toggle":
        action = "off" if st.get("on") else "on"
    if action in ("on", "off"):
        ok, total = _set_gamma(k if action == "on" else None)
        if total and not ok:
            return out({"ok": False, "on": st.get("on", False),
                        "error": "Windows refused the colour change on this display. Use Windows Night light instead."})
        st = {"on": action == "on", "kelvin": k}
        save_json(NIGHT_STATE, st)
    out({"ok": True, **st})


# ===================================================================== presets
PRESET_REPO = "pctrade/end4-pCpresets"
RAW = f"https://raw.githubusercontent.com/{PRESET_REPO}"


def cmd_presets_online():
    try:
        tree = get_json(f"https://api.github.com/repos/{PRESET_REPO}/git/trees/main?recursive=1",
                        headers={"Accept": "application/vnd.github+json"})
    except Exception as e:  # noqa: BLE001
        return out({"error": f"GitHub: {e}", "presets": []})
    try:
        index = get_json(f"{RAW}/gallery-data/index.json").get("presets", {})
    except Exception:  # noqa: BLE001
        index = {}
    names = sorted({m.group(1) for t in tree.get("tree", []) if (m := re.match(r"presets/([^/]+)/\1\.json$", t["path"]))}, key=str.lower)
    files = {t["path"] for t in tree.get("tree", [])}
    q = urllib.parse.quote
    out({"presets": [{
        "name": n, "author": index.get(n, {}).get("author", ""), "updated": index.get(n, {}).get("updated", ""),
        "preview": f"{RAW}/main/presets/{q(n)}/preview.png" if f"presets/{n}/preview.png" in files else ""} for n in names]})


def windows_settings_from_end4(preset):
    """Maps the parts of an end4-pC preset that mean something on Windows."""
    a, bar = preset.get("appearance", {}), preset.get("bar", {})
    palette = a.get("palette", {}).get("type", "auto") or "auto"
    scheme = palette.replace("scheme-", "") if palette.startswith("scheme-") else "auto"
    ws = bar.get("workspaces", {})
    tr = a.get("transparency", {})
    viz = preset.get("background", {}).get("widgets", {}).get("visualizer", {})
    return {
        "scheme": scheme if scheme in ("content", "expressive", "fidelity", "fruit-salad", "monochrome", "neutral",
                                       "rainbow", "tonal-spot", "vibrant") else "auto",
        "config": {
            "workspaces": int(ws.get("shown", 10) or 10),
            "workspaceLabels": ws.get("numberMap") or [],
            "alwaysShowNumbers": bool(ws.get("alwaysShowNumbers", False)),
            "transparency": round(float(tr.get("backgroundTransparency", 0)), 3) if tr.get("enable") else 0,
            "visualizer": {**config().get("visualizer", {}), **({"desktop": bool(viz.get("enable"))} if "enable" in viz else {})},
        },
        "wallpaper": preset.get("background", {}).get("wallpaperPath", ""),
    }


def apply_settings(mapped, wallpaper_path, mode=None):
    cfg = config()
    cfg.update(mapped["config"])
    save_json(CONFIG, cfg)
    sync_zebar_startup(cfg)
    args = ["-Scheme", mapped["scheme"]]
    if wallpaper_path:
        args = ["-Image", str(wallpaper_path)] + args
    if mode in ("dark", "light"):
        args += ["-Mode", mode]
    return run_switchwall(*args)


def cmd_preset_apply_online(name):
    if not re.fullmatch(r"[^/\\]+", name):
        return out({"ok": False, "error": "bad name"})
    q = urllib.parse.quote(name)
    try:
        preset = get_json(f"{RAW}/main/presets/{q}/{q}.json")
        mapped = windows_settings_from_end4(preset)
        wall = None
        if mapped["wallpaper"]:
            wall = download(f"{RAW}/main/presets/{q}/{urllib.parse.quote(Path(mapped['wallpaper']).name)}",
                            WALLPAPERS / "end4-presets" / re.sub(r"[^\w.-]", "_", name))
        local = PRESETS / re.sub(r"[^\w.-]", "_", name)
        save_json(local / "preset.json", {"name": name, "from": "online", **mapped, "wallpaper": str(wall) if wall else ""})
        out({"ok": True, "name": name, "theme": apply_settings(mapped, wall)})
    except Exception as e:  # noqa: BLE001
        out({"ok": False, "error": str(e)})


def cmd_presets_local():
    items = []
    if PRESETS.exists():
        for d in sorted(PRESETS.iterdir(), key=lambda p: p.name.lower()):
            p = load_json(d / "preset.json", None)
            if p:
                items.append({"id": d.name, "name": p.get("name", d.name), "scheme": p.get("scheme"),
                              "mode": p.get("mode"), "wallpaper": p.get("wallpaper", ""), "from": p.get("from", "local")})
    out({"presets": items})


def cmd_preset_save(name):
    state = load_json(ROOT / "state.json", {})
    cfg = config()
    pid = re.sub(r"[^\w.-]", "_", name)[:60] or "preset"
    keep = {k: cfg[k] for k in ("workspaces", "workspaceLabels", "alwaysShowNumbers", "transparency", "visualizer",
                                "clockFormat", "tempUnit", "desktopWidgets") if k in cfg}
    save_json(PRESETS / pid / "preset.json", {"name": name, "from": "local", "scheme": state.get("scheme", "auto"),
                                             "mode": state.get("mode", "dark"), "wallpaper": state.get("image") or "",
                                             "color": state.get("color"), "config": keep})
    out({"ok": True, "id": pid})


def cmd_preset_apply_local(pid):
    p = load_json(PRESETS / re.sub(r"[^\w.-]", "_", pid) / "preset.json", None)
    if not p:
        return out({"ok": False, "error": "preset not found"})
    wall = p.get("wallpaper") if p.get("wallpaper") and Path(p["wallpaper"]).exists() else None
    mapped = {"scheme": p.get("scheme", "auto"), "config": p.get("config", {})}
    if not wall and p.get("color"):
        cfg = config(); cfg.update(mapped["config"]); save_json(CONFIG, cfg)
        return out({"ok": True, "theme": run_switchwall("-Color", p["color"], "-Scheme", mapped["scheme"], "-Mode", p.get("mode", "dark"))})
    out({"ok": True, "theme": apply_settings(mapped, wall, p.get("mode"))})


def cmd_preset_delete(pid):
    d = PRESETS / re.sub(r"[^\w.-]", "_", pid)
    if d.exists() and d.parent == PRESETS:
        shutil.rmtree(d)
    out({"ok": True})


# ===================================================================== live wallpaper helper
def cmd_video_frame(video, dest):
    try:
        import imageio_ffmpeg
        ff = imageio_ffmpeg.get_ffmpeg_exe()
    except ImportError:
        ff = shutil.which("ffmpeg")
    if not ff:
        return out({"ok": False, "error": "ffmpeg not available"})
    r = subprocess.run([ff, "-y", "-loglevel", "error", "-ss", "1", "-i", video, "-frames:v", "1", dest],
                       capture_output=True, creationflags=NO_WINDOW, timeout=60)
    if r.returncode or not Path(dest).exists():   # very short clips: grab the first frame
        subprocess.run([ff, "-y", "-loglevel", "error", "-i", video, "-frames:v", "1", dest],
                       capture_output=True, creationflags=NO_WINDOW, timeout=60)
    out({"ok": Path(dest).exists(), "path": dest})


# ===================================================================== Wallpaper Engine
# Wallpaper Engine (Steam) is controlled through its documented command line:
# wallpaper64.exe -control openWallpaper / closeWallpaper / getWallpaper.
WE_APP = "431960"
WE_THUMBS = PACK / "we-thumbs"
VIDEO_EXT = {".mp4", ".webm", ".mkv", ".mov", ".avi", ".wmv", ".m4v"}


def steam_libraries():
    if os.environ.get("END4_STEAM_LIBS"):
        return [Path(p) for p in os.environ["END4_STEAM_LIBS"].split(os.pathsep) if p]
    roots = []
    if IS_WIN:
        import winreg
        for hive, key in ((winreg.HKEY_CURRENT_USER, r"Software\Valve\Steam"),
                          (winreg.HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Valve\Steam")):
            try:
                with winreg.OpenKey(hive, key) as k:
                    for name in ("SteamPath", "InstallPath"):
                        try:
                            roots.append(Path(winreg.QueryValueEx(k, name)[0]))
                        except OSError:
                            pass
            except OSError:
                pass
    roots.append(Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / "Steam")
    libs = []
    for root in roots:
        vdf = root / "steamapps" / "libraryfolders.vdf"
        if vdf.is_file():
            for m in re.finditer(r'"path"\s+"([^"]+)"', vdf.read_text(encoding="utf-8", errors="replace")):
                libs.append(Path(m.group(1).replace("\\\\", "\\")))
        libs.append(root)
    seen, out_ = set(), []
    for lib in libs:
        key = str(lib).lower().rstrip("\\/")
        if key not in seen and lib.exists():
            seen.add(key)
            out_.append(lib)
    return out_


def we_dir():
    custom = config().get("wallpaperEngine", {}).get("path")
    if custom and Path(custom).is_dir():
        return Path(custom)
    for lib in steam_libraries():
        d = lib / "steamapps" / "common" / "wallpaper_engine"
        if d.is_dir():
            return d
    return None


def we_exe():
    d = we_dir()
    for name in ("wallpaper64.exe", "wallpaper32.exe"):
        if d and (d / name).is_file():
            return d / name
    return None


def we_running():
    try:
        import psutil
        return any((p.info.get("name") or "").lower() in ("wallpaper64.exe", "wallpaper32.exe")
                   for p in psutil.process_iter(["name"]))
    except ImportError:
        if not IS_WIN:
            return bool(os.environ.get("END4_WE_RUNNING"))
        r = subprocess.run(["tasklist", "/FO", "CSV", "/NH"], capture_output=True, text=True, creationflags=NO_WINDOW)
        return "wallpaper64.exe" in r.stdout.lower() or "wallpaper32.exe" in r.stdout.lower()


def we_control(*args, timeout=20):
    exe = we_exe()
    if not exe:
        raise FileNotFoundError("Wallpaper Engine not found in your Steam libraries")
    return subprocess.run([str(exe), "-control", *map(str, args)], capture_output=True, text=True,
                          timeout=timeout, creationflags=NO_WINDOW)


def we_project_dirs():
    d = we_dir()
    dirs = []
    for lib in steam_libraries():
        ws = lib / "steamapps" / "workshop" / "content" / WE_APP
        if ws.is_dir():
            dirs += [(p, "workshop") for p in ws.iterdir() if (p / "project.json").is_file()]
    if d:
        for sub in ("myprojects", "defaultprojects"):
            pd = d / "projects" / sub
            if pd.is_dir():
                dirs += [(p, sub) for p in pd.iterdir() if (p / "project.json").is_file()]
    return dirs


def _project(path):
    """project.json -> dict (Wallpaper Engine writes it with a BOM sometimes)."""
    try:
        return json.loads(Path(path).read_text(encoding="utf-8-sig"))
    except (OSError, ValueError):
        return {}


def _thumb(src, dest, size=360):
    from PIL import Image
    if dest.exists() and dest.stat().st_mtime >= src.stat().st_mtime:
        return True
    try:
        img = Image.open(src)
        img.seek(0)
        img = img.convert("RGB")
        img.thumbnail((size, size))
        dest.parent.mkdir(parents=True, exist_ok=True)
        img.save(dest, quality=82)
        return True
    except (OSError, ValueError):
        return False


def cmd_we_status():
    exe = we_exe()
    st = load_json(ROOT / "state.json", {})
    out({"installed": bool(exe), "exe": str(exe or ""), "running": bool(exe) and we_running(),
         "current": (st.get("live") or "")[3:] if str(st.get("live") or "").startswith("we:") else "",
         "follow": config().get("wallpaperEngine", {}).get("follow", True)})


def cmd_we_list():
    if not we_exe():
        return out({"installed": False, "items": []})
    items = []
    for d, origin in we_project_dirs():
        p = _project(d / "project.json")
        pid = re.sub(r"[^\w.-]", "_", d.name)
        thumb = ""
        prev = d / str(p.get("preview") or "")
        if p.get("preview") and prev.is_file() and _thumb(prev, WE_THUMBS / f"{pid}.jpg"):
            thumb = f"we-thumbs/{pid}.jpg"
        items.append({"id": pid, "title": p.get("title") or d.name, "type": str(p.get("type") or "").lower(),
                      "origin": origin, "path": str(d / "project.json"), "thumb": thumb,
                      "rating": p.get("contentrating", ""), "tags": p.get("tags", [])})
    items.sort(key=lambda i: i["title"].lower())
    out({"installed": True, "items": items})


def _we_theme(target):
    """Themes end4 from a Wallpaper Engine wallpaper (project.json or media file)."""
    target = Path(target)
    proj_file = target if target.name.lower() == "project.json" else (target.parent / "project.json")
    p = _project(proj_file) if proj_file.is_file() else {}
    frame = ROOT / "live-frame.png"
    ROOT.mkdir(parents=True, exist_ok=True)
    media = None
    if target.suffix.lower() in VIDEO_EXT:
        media = target
    elif str(p.get("type", "")).lower() == "video" and p.get("file"):
        media = proj_file.parent / p["file"]
    if media and media.is_file():
        buf = __import__("io").StringIO()
        old, sys.stdout = sys.stdout, buf
        try:
            cmd_video_frame(str(media), str(frame))
        finally:
            sys.stdout = old
        if frame.is_file():
            # A full-resolution frame also becomes the Windows wallpaper (lock screen, apps that read it).
            return run_switchwall("-Image", str(frame), "-Live", "we:" + str(target))
    prev = proj_file.parent / str(p.get("preview") or "")
    if p.get("preview") and prev.is_file():
        from PIL import Image
        img = Image.open(prev)
        img.seek(0)
        img.convert("RGB").save(frame)
        # Previews are small, so only the colors change; the Windows wallpaper stays as it is.
        return run_switchwall("-Image", str(frame), "-Live", "we:" + str(target), "-ThemeOnly")
    return {"ok": False, "error": "No preview or video to take colors from"}


def _find_project(ident):
    if Path(ident).exists():
        return Path(ident)
    for d, _ in we_project_dirs():
        if re.sub(r"[^\w.-]", "_", d.name) == ident:
            return d / "project.json"
    return None


def cmd_we_apply(ident, monitor=""):
    target = _find_project(ident)
    if not target:
        return out({"ok": False, "error": f"Wallpaper {ident} not found"})
    try:
        if not ensure_we_running():
            return out({"ok": False, "error": "Wallpaper Engine didn't start"})
        args = ["openWallpaper", "-file", str(target)]
        if str(monitor).isdigit():
            args += ["-monitor", monitor]
        we_control(*args)
    except (OSError, subprocess.SubprocessError) as e:
        return out({"ok": False, "error": str(e)})
    out({"ok": True, "theme": _we_theme(target)})


def cmd_we_close():
    if we_exe() and we_running():
        try:
            we_control("closeWallpaper")
        except (OSError, subprocess.SubprocessError):
            pass
    out({"ok": True})


def we_key(path):
    """Same wallpaper -> same key, whether WE reports project.json, scene.pkg or the folder."""
    p = Path(str(path).strip().strip('"'))
    if p.is_dir():
        return str(p).lower()
    return str(p.parent if (p.parent / "project.json").is_file() else p).lower()


def ensure_we_running(wait=20):
    if we_running():
        return True
    subprocess.Popen([str(we_exe())], creationflags=NO_WINDOW, close_fds=True)
    for _ in range(wait * 2):
        time.sleep(0.5)
        if we_running():
            time.sleep(3)     # give it a moment to accept commands
            return True
    return False


def we_current():
    # `-control getWallpaper` prints nothing on current Wallpaper Engine builds, so read the
    # wallpaper it saved for this Windows user (first monitor) from its config.json first.
    cfg = _project(we_dir() / "config.json") if we_dir() else {}   # BOM-safe JSON read
    users = [cfg.get(os.environ.get("USERNAME", ""))] + list(cfg.values())
    for u in users:
        sel = (u or {}).get("general", {}).get("wallpaperconfig", {}).get("selectedwallpapers") if isinstance(u, dict) else None
        if isinstance(sel, dict) and sel:
            f = (sel[sorted(sel)[0]] or {}).get("file", "")
            if f and Path(f).exists():
                return f
    r = we_control("getWallpaper", timeout=10)
    line = next((l.strip().strip('"') for l in r.stdout.splitlines() if l.strip()), "")
    return line if line and Path(line).exists() else ""


def cmd_we_sync():
    """Streams: re-themes end4 whenever Wallpaper Engine's wallpaper changes (its UI, playlists...).
    Started by the bar; only one copy runs even with a bar on every monitor."""
    if not we_exe():
        return out({"skip": "Wallpaper Engine not installed"})
    lock = ROOT / "we-sync.pid"
    ROOT.mkdir(parents=True, exist_ok=True)
    try:
        other = int(lock.read_text())
        if other != os.getpid() and _parent_alive(other):
            return out({"skip": "already running"})
    except (OSError, ValueError):
        pass
    lock.write_text(str(os.getpid()))
    ppid = _owner_pid()
    out({"started": True})
    last = None
    interval = float(os.environ.get("END4_WE_INTERVAL", "5"))
    try:
        while _parent_alive(ppid):
            follow = config().get("wallpaperEngine", {}).get("follow", True)
            if follow and we_exe() and we_running():
                try:
                    cur = we_current()
                except (OSError, subprocess.SubprocessError):
                    cur = ""
                if cur and we_key(cur) != last:
                    st = load_json(ROOT / "state.json", {})
                    live = str(st.get("live") or "")
                    if not live.startswith("we:") or we_key(live[3:]) != we_key(cur):   # changed outside end4
                        res = _we_theme(cur)
                        out({"changed": cur, "ok": bool(res.get("ok", True))})
                    last = we_key(cur)
            time.sleep(interval)
    finally:
        try:
            if lock.read_text() == str(os.getpid()):
                lock.unlink()
        except OSError:
            pass


# ===================================================================== visualizer (cava-style)
def _parent_alive(ppid):
    if not IS_WIN:
        try:
            os.kill(ppid, 0)
            return True
        except OSError:
            return False
    import ctypes
    h = ctypes.windll.kernel32.OpenProcess(0x00100000, False, ppid)  # SYNCHRONIZE
    if not h:
        return False
    alive = ctypes.windll.kernel32.WaitForSingleObject(h, 0) == 0x102  # WAIT_TIMEOUT
    ctypes.windll.kernel32.CloseHandle(h)
    return alive


def _owner_pid():
    """PID of the process that started this stream (Zebar). On Windows a venv's python.exe
    is a launcher that runs the real interpreter as its child and outlives Zebar, so watching
    it would keep streams (and Zebar's inherited port) alive forever. Skip over it."""
    ppid = os.getppid()
    if not IS_WIN or sys.prefix == sys.base_prefix:
        return ppid
    import ctypes
    from ctypes import wintypes

    class PE(ctypes.Structure):
        _fields_ = [("dwSize", wintypes.DWORD), ("cntUsage", wintypes.DWORD), ("th32ProcessID", wintypes.DWORD),
                    ("th32DefaultHeapID", ctypes.c_void_p), ("th32ModuleID", wintypes.DWORD),
                    ("cntThreads", wintypes.DWORD), ("th32ParentProcessID", wintypes.DWORD),
                    ("pcPriClassBase", ctypes.c_long), ("dwFlags", wintypes.DWORD), ("szExeFile", ctypes.c_wchar * 260)]

    k = ctypes.windll.kernel32
    k.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    snap = k.CreateToolhelp32Snapshot(0x2, 0)  # TH32CS_SNAPPROCESS
    e = PE()
    e.dwSize = ctypes.sizeof(PE)
    try:
        ok = k.Process32FirstW(wintypes.HANDLE(snap), ctypes.byref(e))
        while ok:
            if e.th32ProcessID == ppid:
                return e.th32ParentProcessID if e.szExeFile.lower() in ("python.exe", "pythonw.exe") else ppid
            ok = k.Process32NextW(wintypes.HANDLE(snap), ctypes.byref(e))
    finally:
        k.CloseHandle(wintypes.HANDLE(snap))
    return ppid


class Spectrum:
    """FFT -> log-spaced bars with cava-like auto-sensitivity and gravity falloff."""

    def __init__(self, bars, rate=48000, size=2048, lo=50, hi=12000):
        import numpy as np
        self.np, self.bars, self.size = np, bars, size
        freqs = np.fft.rfftfreq(size, 1 / rate)
        edges = np.geomspace(lo, hi, bars + 1)
        self.idx = [np.where((freqs >= edges[i]) & (freqs < max(edges[i + 1], edges[i] + freqs[1])))[0] for i in range(bars)]
        self.window = np.hanning(size)
        self.buf = np.zeros(size)
        self.level = np.zeros(bars)
        self.peak = 1e-3
        self.weights = np.linspace(0.8, 1.6, bars)  # lift the treble a little, as cava does

    def push(self, mono):
        np = self.np
        n = len(mono)
        self.buf = np.concatenate([self.buf[n:], mono])[-self.size:]

    def frame(self, dt):
        np = self.np
        mag = np.abs(np.fft.rfft(self.buf * self.window))
        raw = np.array([mag[i].mean() if len(i) else 0 for i in self.idx]) * self.weights
        raw = np.log1p(raw)
        self.peak = max(self.peak * (1 - 0.5 * dt), raw.max(), 1e-3)   # auto-sensitivity
        target = np.clip(raw / self.peak, 0, 1)
        fall = 2.8 * dt                                                   # gravity
        self.level = np.where(target > self.level, target * 0.6 + self.level * 0.4, np.maximum(target, self.level - fall))
        return self.level


def cmd_visualizer(bars="48"):
    import warnings
    warnings.filterwarnings("ignore")
    import numpy as np
    bars = max(8, min(256, int(bars)))
    ppid = _owner_pid()
    spec = Spectrum(bars)
    lock = threading.Lock()
    last_audio = [0.0]

    def capture():
        import soundcard as sc
        while True:
            try:
                spk = sc.default_speaker()
                mic = sc.get_microphone(id=str(spk.name), include_loopback=True)
                with mic.recorder(samplerate=48000, channels=2, blocksize=512) as rec:
                    next_check = time.time() + 2
                    while True:
                        data = rec.record(numframes=512)
                        if data is not None and len(data):
                            with lock:
                                spec.push(data.mean(axis=1))
                                last_audio[0] = time.time()
                        if time.time() > next_check:      # follow default-device changes
                            next_check = time.time() + 2
                            if sc.default_speaker().name != spk.name:
                                break
            except Exception:  # noqa: BLE001 - device changes, exclusive mode, etc.
                time.sleep(1)

    threading.Thread(target=capture, daemon=True).start()
    fps = 30
    prev = time.time()
    silent_frames = 0
    next_parent_check = time.time() + 2
    while True:
        time.sleep(1 / fps)
        now = time.time()
        dt, prev = now - prev, now
        with lock:
            if now - last_audio[0] > 0.25:
                spec.buf *= 0.5     # nothing playing: let the bars fall
            level = spec.frame(dt)
        if level.max() < 0.01:
            silent_frames += 1
            if silent_frames > fps and silent_frames % fps:   # idle: one line per second
                continue
        else:
            silent_frames = 0
        try:
            sys.stdout.write(" ".join(f"{v:.2f}" for v in level) + "\n")
            sys.stdout.flush()
        except (BrokenPipeError, OSError):
            return
        if now > next_parent_check:              # exit if Zebar went away
            next_parent_check = now + 2
            if not _parent_alive(ppid):
                return


# ===================================================================== main
COMMANDS = {
    "translate": cmd_translate, "booru": cmd_booru, "download-wallpaper": cmd_download_wallpaper, "open": cmd_open,
    "football": cmd_football, "ai-models": cmd_ai_models, "ai-chat": cmd_ai_chat,
    "secret-set": cmd_secret_set, "secret-status": cmd_secret_status,
    "notifications": cmd_notifications, "notifications-dismiss": cmd_notifications_dismiss,
    "nightlight": cmd_nightlight, "config-set": cmd_config_set,
    "presets-online": cmd_presets_online, "preset-apply-online": cmd_preset_apply_online,
    "presets-local": cmd_presets_local, "preset-save": cmd_preset_save,
    "preset-apply-local": cmd_preset_apply_local, "preset-delete": cmd_preset_delete,
    "visualizer": cmd_visualizer, "video-frame": cmd_video_frame,
    "we-status": cmd_we_status, "we-list": cmd_we_list, "we-apply": cmd_we_apply, "we-close": cmd_we_close,
    "we-sync": cmd_we_sync,
}

if __name__ == "__main__":
    if len(sys.argv) < 2 or sys.argv[1] not in COMMANDS:
        out({"error": "usage: end4_bridge.py <" + "|".join(COMMANDS) + "> ..."})
        sys.exit(2)
    try:
        COMMANDS[sys.argv[1]](*sys.argv[2:])
    except TypeError as e:
        out({"error": f"bad arguments: {e}"})
    except KeyboardInterrupt:
        pass
