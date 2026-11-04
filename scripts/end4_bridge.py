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
    end4_bridge.py notifications-watch     (streams new notifications)
    end4_bridge.py nightlight on|off|status [kelvin]
    end4_bridge.py color-pick              (full-screen picker; copies the hex colour)
    end4_bridge.py mixer [volume <app> <0-100> | mute <app> <0|1>]
    end4_bridge.py record start|stop|status  (primary monitor, saved in Videos/Recordings)
    end4_bridge.py app-icons <process names...>  /  launch <pinned app name>
    end4_bridge.py pick-image image1|image2|image3   (file dialog; copies the picture into the pack)
    end4_bridge.py screen-translate        (full-screen: drag over text, OCR + translate)
    end4_bridge.py config-set <key> <json>
    end4_bridge.py presets-online          /  preset-apply-online <name>
    end4_bridge.py presets-local           /  preset-save <name>  /  preset-apply-local <name>  /  preset-delete <name>
    end4_bridge.py visualizer [bars]       (streams one line of bar heights per frame)
    end4_bridge.py video-frame <video> <out.png>
    end4_bridge.py we-status / we-list / we-apply <id|path> [monitor] / we-close
    end4_bridge.py we-sync                 (streams; re-themes when Wallpaper Engine changes wallpaper)
    end4_bridge.py agents / agents-watch   AI agents' state and usage, from end4_agent.py (watch streams)

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
    if key.startswith("visualizer") or key in ("desktopWidgets", "dock.enabled"):
        sync_zebar_startup(cfg)
    out({"ok": True})


def sync_zebar_startup(cfg):
    """Keeps Zebar's startup list in line with the widgets enabled in config."""
    path = PACK.parent / "settings.json"
    s = load_json(path, {})
    keep = [c for c in s.get("startupConfigs", []) if c.get("pack") != "end4-pC"]
    native = cfg.get("native", {})  # parts native/end4 draws instead of Zebar
    mine = [] if native.get("bar") else [{"pack": "end4-pC", "widget": "bar", "preset": "default"}]
    if cfg.get("desktopWidgets", True):
        mine.append({"pack": "end4-pC", "widget": "desktop", "preset": "default"})
    if cfg.get("visualizer", {}).get("desktop", True):
        mine.append({"pack": "end4-pC", "widget": "visualizer", "preset": "default"})
    if cfg.get("dock", {}).get("enabled", False):
        mine.append({"pack": "end4-pC", "widget": "dock", "preset": "default"})
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
def translate_text(text, target, source="auto"):
    """Google Translate (the endpoint end4's translator uses). Returns (translation, detected language)."""
    q = urllib.parse.urlencode({"client": "gtx", "sl": source or "auto", "tl": target, "dt": "t", "q": text})
    data = get_json("https://translate.googleapis.com/translate_a/single?" + q)
    return "".join(seg[0] for seg in data[0] if seg and seg[0]), (data[2] if len(data) > 2 else source)


def cmd_translate(target, source, *text):
    text = " ".join(text).strip()
    if not text:
        return out({"text": "", "detected": source})
    try:
        result, detected = translate_text(text, target, source)
        out({"text": result, "detected": detected})
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
            bases = ["https://yande.re"] if source == "yandere" else (["https://konachan.com", "https://konachan.net"] if nsfw else ["https://konachan.net"])
            q = "/post.json?" + urllib.parse.urlencode({"tags": _booru_tags(tags, nsfw, "rating:safe"), "limit": limit, "page": page})
            for base in bases:   # konachan.com is blocked on some networks; .net is the same site, safe posts only
                try:
                    posts = get_json(base + q)
                    break
                except (OSError, ValueError):
                    if base == bases[-1]:
                        raise
            for it in posts:
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
            # api.waifu.im no longer exists and waifu.im now serves an unrelated spam page (Nov 2026).
            return out({"error": "waifu.im has shut down. Pick another source."})
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
        if re.search(r"curl: \(35\)|Connection was reset|ConnectionReset|WinError 10054", str(e)):
            return out({"error": f"{source}: the connection was reset. Your network or ISP is probably blocking this site; it should work over a VPN."})
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


def _wpndb():
    return Path(os.environ.get("END4_WPNDB", Path(os.environ.get("LOCALAPPDATA", "")) / "Microsoft" / "Windows" / "Notifications" / "wpndatabase.db"))


def read_notifications():
    """Reads Windows' notification history (the same database Action Center uses)."""
    import sqlite3
    import tempfile
    src = _wpndb()
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


def cmd_notifications_watch():
    """Streams {"new": notification} as notifications arrive, for the bar's Dynamic Island. Re-reads
    the database only when it (or its write-ahead log) changes. One copy runs for all bars."""
    if not _first_instance("notif-watch"):
        return out({"skip": "already running"})
    ppid = _owner_pid()
    _exit_with_owner(ppid)
    db = _wpndb()
    seen = {n["id"] for n in read_notifications().get("items", [])}
    out({"started": True})
    stamp = None
    while _parent_alive(ppid):
        now = tuple(f.stat().st_mtime_ns for f in (db, db.with_name(db.name + "-wal")) if f.exists())
        if now != stamp:
            stamp = now
            for n in reversed(read_notifications().get("items", [])):   # oldest first
                if n["id"] not in seen:
                    seen.add(n["id"])
                    out({"new": n})
        time.sleep(2)


# ===================================================================== AI agents (end4_agent.py writes, the island reads)
AGENTS_DIR = ROOT / "agents"
AGENT_LABELS = {"claude": "Claude", "codex": "Codex", "gemini": "Gemini", "opencode": "OpenCode", "cursor": "Cursor"}


def agents_summary():
    """One entry per agent: the session that needs you (if any), what's running, and usage windows."""
    now, res = time.time(), []
    if not AGENTS_DIR.is_dir():
        return res
    for d in sorted(p for p in AGENTS_DIR.iterdir() if p.is_dir()):
        sessions = []
        for f in d.glob("*.json"):
            if f.name == "usage.json":
                continue
            s = load_json(f, {})
            age = now - float(s.get("at") or 0)
            if age > 12 * 3600:   # the agent quit without a SessionEnd
                f.unlink(missing_ok=True)
                continue
            if s.get("state") == "working" and age > 15 * 60:
                s["state"] = "done"   # crashed or killed mid-turn: don't show it working forever
            sessions.append(s)
        u = load_json(d / "usage.json", {})
        windows = {k: v for k, v in (u.get("windows") or {}).items() if not v.get("resets_at") or float(v["resets_at"]) > now}
        if not sessions and not windows:
            continue
        waiting = sorted((s for s in sessions if s.get("state") == "waiting"), key=lambda s: -float(s.get("at") or 0))
        working = [s for s in sessions if s.get("state") == "working"]
        res.append({"id": d.name, "label": AGENT_LABELS.get(d.name, d.name.title()),
                    "state": "waiting" if waiting else "working" if working else "idle",
                    "message": waiting[0].get("message", "") if waiting else "",
                    "cwd": (waiting or working or [{}])[0].get("cwd", ""),
                    "waiting": len(waiting), "working": len(working),
                    "usage": windows, "model": u.get("model", ""), "usageAt": u.get("at")})
    return res


def cmd_agents():
    out({"agents": agents_summary()})


def cmd_agents_watch():
    """Streams {"agents": [...]} whenever an agent reports (and every 30 s, for resets and stale
    sessions). One copy runs for all bars; the bar shares it through localStorage."""
    if not _first_instance("agents-watch"):
        return out({"skip": "already running"})
    ppid = _owner_pid()
    _exit_with_owner(ppid)
    stamp, last, sent = None, None, 0.0
    while _parent_alive(ppid):
        try:
            now = tuple(sorted((e.path, e.stat().st_mtime_ns) for d in os.scandir(AGENTS_DIR) if d.is_dir()
                               for e in os.scandir(d.path) if e.name.endswith(".json")))
        except OSError:
            now = ()
        if now != stamp or time.time() - sent > 30:
            stamp, sent = now, time.time()
            cur = agents_summary()
            if cur != last:
                last = cur
                out({"agents": cur})
        time.sleep(0.5)


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


def _monitor_count():
    if not IS_WIN:
        return 1
    import ctypes
    return max(1, ctypes.windll.user32.GetSystemMetrics(80))   # SM_CMONITORS


def cmd_we_apply(ident, monitor=""):
    target = _find_project(ident)
    if not target:
        return out({"ok": False, "error": f"Wallpaper {ident} not found"})
    try:
        if not ensure_we_running():
            return out({"ok": False, "error": "Wallpaper Engine didn't start"})
        # Without -monitor, Wallpaper Engine sets only one monitor; end4 has one wallpaper for all.
        monitors = [monitor] if str(monitor).isdigit() else range(_monitor_count())
        for m in monitors:
            we_control("openWallpaper", "-file", str(target), "-monitor", m)
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
    if not _first_instance("we-sync"):
        return out({"skip": "already running"})
    ppid = _owner_pid()
    _exit_with_owner(ppid)
    out({"started": True})
    last = None
    interval = float(os.environ.get("END4_WE_INTERVAL", "5"))
    # Closing a wallpaper through WE's CLI doesn't clear its saved one, so WE reopens it when it
    # starts with Windows and hides the picture end4 set. Close it again for a while after start-up
    # whenever end4's last choice was a still picture (state.live empty).
    started, closes, last_close = time.time(), 0, 0.0
    while _parent_alive(ppid):
        follow = config().get("wallpaperEngine", {}).get("follow", True)
        if (follow and closes < 3 and time.time() - started < 90 and time.time() - last_close > 10
                and not load_json(ROOT / "state.json", {}).get("live") and we_exe() and we_running()):
            try:
                we_control("closeWallpaper")
                out({"closed": "restored end4's picture"})
            except (OSError, subprocess.SubprocessError):
                pass
            closes, last_close = closes + 1, time.time()
            time.sleep(interval)
            continue
        if follow and we_exe() and we_running():
            try:
                cur = we_current()
            except (OSError, subprocess.SubprocessError):
                cur = ""
            if cur and we_key(cur) != last:
                live = str(load_json(ROOT / "state.json", {}).get("live") or "")
                following = live.startswith("we:")
                # On start, Wallpaper Engine's saved wallpaper is often stale (end4 may have closed it
                # for a picture), so only catch up if end4 was following WE and WE has moved on since.
                if (last is not None or following) and not (following and we_key(live[3:]) == we_key(cur)):
                    res = _we_theme(cur)
                    out({"changed": cur, "ok": bool(res.get("ok", True))})
                last = we_key(cur)
        time.sleep(interval)


def _first_instance(name):
    """True for the first process to claim `name`. A named mutex is atomic (two bars starting at
    the same moment can't both win) and Windows releases it when the process exits."""
    if not IS_WIN:
        return True
    import ctypes
    k = ctypes.WinDLL("kernel32", use_last_error=True)
    k.CreateMutexW.restype = ctypes.c_void_p
    _first_instance.handle = k.CreateMutexW(None, False, f"Local\\end4-pC-{name}")   # kept open until exit
    return ctypes.get_last_error() != 183   # ERROR_ALREADY_EXISTS


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


def _exit_with_owner(pid):
    """Exits the moment Zebar does. Zebar's children inherit its listening socket (port 6124),
    so a stream that outlives Zebar even for a few seconds stops the next Zebar (Mod+Ctrl+R)
    from starting its server, and every widget shows 'refused to connect'."""
    if not IS_WIN:
        return
    import ctypes
    k = ctypes.windll.kernel32
    k.OpenProcess.restype = ctypes.c_void_p
    h = k.OpenProcess(0x00100000, False, pid)  # SYNCHRONIZE
    if not h:
        os._exit(0)

    def wait():
        k.WaitForSingleObject(ctypes.c_void_p(h), 0xFFFFFFFF)  # INFINITE
        os._exit(0)
    threading.Thread(target=wait, daemon=True).start()


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
    _exit_with_owner(ppid)
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
def cmd_click_through(widget):
    """Lets mouse clicks pass through a widget's window to the desktop. Tauri's setIgnoreCursorEvents
    sets the same two styles, but Zebar doesn't grant widgets permission to call it."""
    if not (IS_WIN and re.fullmatch(r"[a-z-]+", widget)):
        return out({"ok": False})
    import ctypes
    u = ctypes.windll.user32
    u.FindWindowW.restype = ctypes.c_void_p
    h = ctypes.c_void_p(u.FindWindowW(None, f"Zebar - end4-pC / {widget}"))
    if not h.value:
        return out({"ok": False, "error": "window not found"})
    ex = u.GetWindowLongW(h, -20)                      # GWL_EXSTYLE
    u.SetWindowLongW(h, -20, ex | 0x80000 | 0x20)      # WS_EX_LAYERED | WS_EX_TRANSPARENT
    out({"ok": True})


def cmd_focus_widget(widget, x="0", y="0"):
    """Gives a popup keyboard focus (Esc, typing). Windows only lets the process that received the
    last input take the foreground; a shortcut's key went to GlazeWM, so Zebar's own setFocus is
    refused. A no-op key event makes this process eligible (the usual workaround). x, y: the
    popup's screen position, to pick it over a copy on another monitor that is still closing."""
    if not (IS_WIN and re.fullmatch(r"[a-z-]+", widget)):
        return out({"ok": False})
    import ctypes
    from ctypes import wintypes
    u = ctypes.windll.user32
    title, found = f"Zebar - end4-pC / {widget}", []

    @ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)
    def each(h, _):
        buf = ctypes.create_unicode_buffer(256)
        u.GetWindowTextW(h, buf, 256)
        if buf.value == title and u.IsWindowVisible(h):
            r = wintypes.RECT()
            u.GetWindowRect(h, ctypes.byref(r))
            found.append((abs(r.left - int(float(x))) + abs(r.top - int(float(y))), h))
        return True
    u.EnumWindows(each, 0)
    if not found:
        return out({"ok": False, "error": "window not found"})
    h = min(found)[1]
    u.keybd_event(0x87, 0, 0, 0)   # F24 down/up: no app uses it
    u.keybd_event(0x87, 0, 2, 0)
    u.SetForegroundWindow(h)
    out({"ok": u.GetForegroundWindow() == h})


def cmd_native_open(name):
    """Opens a popup in native/end4 (`end4.exe --open <name>`): it toggles the popup, and starts Zebar's copy if end4 isn't running."""
    exe = ROOT / "end4.exe"
    if not (re.fullmatch(r"[a-z-]+", name) and exe.exists()):
        return out({"ok": False})
    subprocess.Popen([str(exe), "--open", name], creationflags=NO_WINDOW, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    out({"ok": True})


# ===================================================================== volume mixer (end4's VolumeDialog)
def _mixer_groups():
    """Audio sessions grouped by app (Discord, browsers... run several), without end4's own Python."""
    from pycaw.pycaw import AudioUtilities
    groups = {}
    for s in AudioUtilities.GetAllSessions():
        try:
            exe = s.Process.exe() if s.Process else ""
            if s.Process and any("end4_bridge" in a for a in s.Process.cmdline()):
                continue                               # the visualizer's loopback capture
        except Exception:  # noqa: BLE001  (process gone or access denied)
            exe = ""
        name = Path(exe).stem if exe else ("System sounds" if s.ProcessId == 0 else str(s.ProcessId))
        groups.setdefault(name, []).append(s)
    return groups


def cmd_mixer(action="list", app="", value=""):
    """mixer [list] | mixer volume <app> <0-100> | mixer mute <app> <0|1>"""
    groups = _mixer_groups()
    if action in ("volume", "mute"):
        if app not in groups:
            return out({"error": f"{app} isn't playing audio any more"})
        for s in groups[app]:
            v = s.SimpleAudioVolume
            if action == "volume":
                v.SetMasterVolume(max(0, min(100, float(value))) / 100, None)
            else:
                v.SetMute(1 if value == "1" else 0, None)
    out({"apps": [{"app": name, "volume": round(ss[0].SimpleAudioVolume.GetMasterVolume() * 100),
                   "muted": bool(ss[0].SimpleAudioVolume.GetMute())} for name, ss in sorted(groups.items(), key=lambda g: g[0].lower())]})


# ===================================================================== image cards (end4's image / sticker widgets)
IMAGE_EXT = {".png", ".jpg", ".jpeg", ".webp", ".gif", ".bmp"}


def cmd_pick_image(slot):
    """Asks for a picture and copies it into the pack's images folder (widgets can only show files
    inside the pack). Prints {"file": name} for config desktop.images.<slot>.file (greet: desktop.greet.file)."""
    if not re.fullmatch(r"image[1-3]|greet", slot):
        return out({"error": "bad slot"})
    import tkinter as tk
    from tkinter import filedialog
    root = tk.Tk()
    root.withdraw()
    root.attributes("-topmost", True)   # in front of Settings
    pics = Path.home() / "Pictures"
    path = filedialog.askopenfilename(parent=root, title="Choose a picture", initialdir=str(pics if pics.is_dir() else Path.home()),
                                      filetypes=[("Pictures", " ".join("*" + e for e in sorted(IMAGE_EXT)))])
    root.destroy()
    if not path:
        return out({"cancelled": True})
    src = Path(path)
    if src.suffix.lower() not in IMAGE_EXT:
        return out({"error": "Pick a PNG, JPG, WebP, GIF or BMP picture"})
    if src.stat().st_size > 50_000_000:
        return out({"error": "That picture is over 50 MB"})
    folder = PACK / "images"
    folder.mkdir(parents=True, exist_ok=True)
    for old in folder.glob(f"{slot}-*"):
        old.unlink(missing_ok=True)
    name = f"{slot}-{int(time.time())}{src.suffix.lower()}"   # new name each time, so the card doesn't show a cached copy
    shutil.copyfile(src, folder / name)
    out({"file": name})


# ===================================================================== dock (end4's Dock)
def _dock_pinned():
    return [p for p in config().get("dock", {}).get("pinned", []) if isinstance(p, dict) and p.get("name") and p.get("path")]


def cmd_app_icons(*names):
    """Saves a PNG icon per app into the pack's icons folder (shown by the dock) and returns each
    app's exe path. Names are GlazeWM process names; pinned apps are found even when not running."""
    import psutil
    want = {n.lower(): n for n in names if re.fullmatch(r"[\w .+-]{1,80}", n)}
    exes = {p["name"]: p["path"] for p in _dock_pinned() if p["name"].lower() in want}
    for proc in psutil.process_iter(["name", "exe"]):
        n = re.sub(r"\.exe$", "", proc.info["name"] or "", flags=re.I)
        if n.lower() in want and proc.info["exe"]:
            exes.setdefault(want[n.lower()], proc.info["exe"])
    folder = PACK / "icons"
    folder.mkdir(parents=True, exist_ok=True)
    todo = {n: e for n, e in exes.items() if not (folder / f"{n}.png").exists() and Path(e).is_file()}
    if todo:
        q = lambda s: "'" + str(s).replace("'", "''") + "'"   # PowerShell single-quoted string
        ps = "Add-Type -AssemblyName System.Drawing; " + "; ".join(
            f"try {{ [Drawing.Icon]::ExtractAssociatedIcon({q(e)}).ToBitmap().Save({q(folder / (n + '.png'))}) }} catch {{ }}" for n, e in todo.items())
        subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-Command", ps], capture_output=True, timeout=30, creationflags=NO_WINDOW)
    out({"exes": exes})


def cmd_launch(name):
    """Starts an app pinned to the dock. Only paths saved in config.json's dock.pinned can be started."""
    for p in _dock_pinned():
        if p["name"] == name and Path(p["path"]).is_file():
            os.startfile(p["path"])  # noqa: S606 - the user's own pinned app
            return out({"ok": True})
    out({"ok": False, "error": f"{name} isn't pinned"})


# ===================================================================== screen recorder (end4's record.sh)
REC_STATE = ROOT / "recording.json"


def _recording():
    """The running recording ({pid, file, started}) or None."""
    st = load_json(REC_STATE, None)
    if not st:
        return None
    try:
        import psutil
        if "ffmpeg" in psutil.Process(st["pid"]).name().lower():
            return st
    except Exception:  # noqa: BLE001  (gone: psutil.NoSuchProcess, or bad state)
        pass
    REC_STATE.unlink(missing_ok=True)
    return None


def cmd_record(action="status"):
    """record start | stop | status. Records the primary monitor (video only) to Videos\\Recordings."""
    st = _recording()
    if action == "start" and not st:
        import ctypes
        import imageio_ffmpeg
        u = ctypes.windll.user32
        u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
        w, h = u.GetSystemMetrics(0) // 2 * 2, u.GetSystemMetrics(1) // 2 * 2   # primary monitor; x264 needs even sizes
        folder = Path.home() / "Videos" / "Recordings"
        folder.mkdir(parents=True, exist_ok=True)
        dest = folder / time.strftime("end4-%Y%m%d-%H%M%S.mp4")
        # Fragmented MP4 (1 s fragments): stopping kills ffmpeg, and the file still plays.
        args = [imageio_ffmpeg.get_ffmpeg_exe(), "-nostdin", "-y", "-f", "gdigrab", "-framerate", "30", "-draw_mouse", "1",
                "-offset_x", "0", "-offset_y", "0", "-video_size", f"{w}x{h}", "-i", "desktop",
                "-c:v", "libx264", "-preset", "ultrafast", "-crf", "23", "-pix_fmt", "yuv420p", "-g", "30",
                "-movflags", "+frag_keyframe+empty_moov+default_base_moof", str(dest)]
        p = subprocess.Popen(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                             creationflags=0x00000008 | 0x00000200 | NO_WINDOW)   # detached, own group: outlives the widget
        st = {"pid": p.pid, "file": str(dest), "started": time.time()}
        save_json(REC_STATE, st)
    elif action == "stop" and st:
        import psutil
        psutil.Process(st["pid"]).kill()
        REC_STATE.unlink(missing_ok=True)
        subprocess.Popen(["explorer", "/select,", st["file"]])
        return out({"recording": False, "file": st["file"]})
    out({"recording": bool(st), **(st or {})})


# ===================================================================== screen translator (end4's ScreenTranslator)
# Windows' own OCR (Windows.Media.Ocr), in the languages installed in Windows Settings > Language.
OCR_PS = r'''
Add-Type -AssemblyName System.Runtime.WindowsRuntime
$asTask = [System.WindowsRuntimeSystemExtensions].GetMethods() | Where-Object { $_.Name -eq 'AsTask' -and $_.GetParameters().Count -eq 1 -and $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1' } | Select-Object -First 1
function Await($op, [Type]$t) { $task = $asTask.MakeGenericMethod($t).Invoke($null, @($op)); $task.Wait(-1) | Out-Null; $task.Result }
[Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics, ContentType = WindowsRuntime] | Out-Null
[Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType = WindowsRuntime] | Out-Null
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($Path)) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bmp = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])
$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
$res = Await ($engine.RecognizeAsync($bmp)) ([Windows.Media.Ocr.OcrResult])
$lines = foreach ($l in $res.Lines) {
  $r = $l.Words | ForEach-Object { $_.BoundingRect }
  $x = ($r | Measure-Object X -Minimum).Minimum; $y = ($r | Measure-Object Y -Minimum).Minimum
  [ordered]@{ text = $l.Text; x = $x; y = $y; w = ($r | ForEach-Object { $_.X + $_.Width } | Measure-Object -Maximum).Maximum - $x
              h = ($r | ForEach-Object { $_.Y + $_.Height } | Measure-Object -Maximum).Maximum - $y }
}
$json = @{ lines = @($lines); language = $engine.RecognizerLanguage.LanguageTag } | ConvertTo-Json -Compress -Depth 4
[regex]::Replace($json, '[^\x00-\x7F]', { param($m) '\u{0:x4}' -f [int][char]$m.Value })
'''


def ocr_image(png):
    """Lines of text in an image: [{text, x, y, w, h}] in its pixels."""
    script = f"$Path = '{str(png).replace(chr(39), chr(39) * 2)}'\n" + OCR_PS
    enc = base64.b64encode(script.encode("utf-16-le")).decode()
    r = subprocess.run(["powershell", "-NoProfile", "-NonInteractive", "-EncodedCommand", enc],
                       capture_output=True, text=True, timeout=60, creationflags=NO_WINDOW)
    line = (r.stdout.strip().splitlines() or [""])[-1]
    if not line.startswith("{"):
        raise OSError((r.stderr or "OCR failed").strip()[:300])
    return json.loads(line)["lines"]


def cmd_screen_translate():
    """Drag a box over text on any monitor: Windows OCR reads it, and the translation (into the
    translator's language from Settings) is drawn over it and copied. Click, Esc or right-click closes."""
    import ctypes
    import tkinter as tk
    from PIL import ImageGrab, ImageTk
    u = ctypes.windll.user32
    u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    vx, vy, vw, vh = (u.GetSystemMetrics(i) for i in (76, 77, 78, 79))
    shot = ImageGrab.grab(all_screens=True).convert("RGB")
    target = config().get("translator", {}).get("target", "en")
    root = tk.Tk()
    root.overrideredirect(True)
    root.attributes("-topmost", True)
    root.geometry(f"{vw}x{vh}{vx:+d}{vy:+d}")
    canvas = tk.Canvas(root, width=vw, height=vh, highlightthickness=0, cursor="crosshair")
    canvas.pack()
    bg = ImageTk.PhotoImage(shot)
    canvas.create_image(0, 0, image=bg, anchor="nw")
    box = canvas.create_rectangle(0, 0, 0, 0, outline="#ffffff", width=2, dash=(6, 4))
    status = canvas.create_text(20, 20, anchor="nw", fill="white", font=("Segoe UI", 14, "bold"),
                                text="Drag over the text to translate  (Esc to cancel)")
    st = {"start": None, "done": False, "text": ""}

    def say(msg):
        canvas.itemconfigure(status, text=msg)
        root.update()

    def press(e):
        if st["done"]:
            return root.destroy()
        st["start"] = (e.x, e.y)

    def drag(e):
        if st["start"] and not st["done"]:
            canvas.coords(box, *st["start"], e.x, e.y)

    def release(e):
        if not st["start"] or st["done"]:
            return
        (x0, y0), (x1, y1) = st["start"], (e.x, e.y)
        x0, x1, y0, y1 = min(x0, x1), max(x0, x1), min(y0, y1), max(y0, y1)
        if x1 - x0 < 8 or y1 - y0 < 8:
            return
        st["done"] = True
        say("Reading text…")
        crop = shot.crop((x0, y0, x1, y1))
        if crop.width < 400:   # Windows OCR reads small text better enlarged
            crop = crop.resize((crop.width * 2, crop.height * 2))
        scale = crop.width / (x1 - x0)
        png = ROOT / "ocr.png"
        crop.save(png)
        try:
            lines = ocr_image(png)
            if not lines:
                return say("No text found. Other scripts need their language added in Windows Settings > Language.  (Click to close)")
            say("Translating…")
            joined = "\n".join(l["text"] for l in lines)
            result, _ = translate_text(joined, target)
        except Exception as ex:  # noqa: BLE001
            return say(f"Failed: {ex}  (Click to close)"[:200])
        st["text"] = result
        parts = result.split("\n")
        if len(parts) != len(lines):   # lines merged or split: show it as one block over the box
            lines, parts = [{"x": 0, "y": 0, "w": (x1 - x0) * scale, "h": (y1 - y0) * scale}], [result]
        for l, t in zip(lines, parts):
            lx, ly = x0 + l["x"] / scale, y0 + l["y"] / scale
            lw, lh = l["w"] / scale, l["h"] / scale
            size = max(9, int(lh * 0.8)) if len(parts) > 1 else 14
            canvas.create_rectangle(lx - 3, ly - 2, lx + lw + 3, ly + lh + 2, fill="#1e1e1e", outline="")
            canvas.create_text(lx, ly, anchor="nw", fill="white", text=t, width=max(lw, 120) if len(parts) == 1 else 0,
                               font=("Segoe UI", -size))
        say(f"Translated to {target} and copied.  Click anywhere to close.")

    root.bind("<ButtonPress-1>", press)
    root.bind("<B1-Motion>", drag)
    root.bind("<ButtonRelease-1>", release)
    root.bind("<Button-3>", lambda e: root.destroy())
    root.bind("<Escape>", lambda e: root.destroy())
    root.after(50, root.focus_force)
    root.after(120000, root.destroy)   # never leave a frozen full-screen overlay behind
    root.mainloop()
    if st["text"]:
        subprocess.run(["clip"], input=st["text"].encode("utf-16-le"), creationflags=NO_WINDOW)
    out({"text": st["text"]} if st["text"] else {"cancelled": True})


# ===================================================================== color picker (end4's hyprpicker toggle)
def cmd_color_pick():
    """Freezes every monitor in a full-screen overlay with a magnifier; a click copies the pixel's
    hex colour to the clipboard, Esc or right-click cancels. Prints {"hex": "#rrggbb"} or {"cancelled": true}."""
    import ctypes
    import tkinter as tk
    from PIL import Image, ImageGrab, ImageTk
    u = ctypes.windll.user32
    u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))   # physical pixels, so the overlay lines up with the shot
    vx, vy, vw, vh = (u.GetSystemMetrics(i) for i in (76, 77, 78, 79))   # SM_[XY]VIRTUALSCREEN, SM_C[XY]VIRTUALSCREEN
    shot = ImageGrab.grab(all_screens=True).convert("RGB")
    root = tk.Tk()
    root.overrideredirect(True)
    root.attributes("-topmost", True)
    root.geometry(f"{vw}x{vh}{vx:+d}{vy:+d}")
    canvas = tk.Canvas(root, width=vw, height=vh, highlightthickness=0, cursor="crosshair")
    canvas.pack()
    bg = ImageTk.PhotoImage(shot)
    canvas.create_image(0, 0, image=bg, anchor="nw")
    zoom = canvas.create_image(0, 0, anchor="nw")
    frame = canvas.create_rectangle(0, 0, 0, 0, outline="white", width=2)
    label_bg = canvas.create_rectangle(0, 0, 0, 0, fill="#1e1e1e", outline="")
    label = canvas.create_text(0, 0, anchor="nw", fill="white", font=("Segoe UI", 12, "bold"))
    state = {"hex": None, "img": None}

    def hex_at(x, y):
        return "#%02x%02x%02x" % shot.getpixel((min(max(x, 0), shot.width - 1), min(max(y, 0), shot.height - 1)))

    def move(e):
        x, y, n, k = e.x, e.y, 11, 12                       # 11x11 pixels shown 12x
        crop = shot.crop((x - n // 2, y - n // 2, x + n // 2 + 1, y + n // 2 + 1)).resize((n * k, n * k), Image.NEAREST)
        state["img"] = ImageTk.PhotoImage(crop)
        side = n * k
        ox = x + 24 if x + 24 + side < vw else x - 24 - side
        oy = y + 24 if y + 24 + side + 30 < vh else y - 24 - side - 30
        canvas.itemconfigure(zoom, image=state["img"])
        canvas.coords(zoom, ox, oy)
        canvas.coords(frame, ox + n // 2 * k, oy + n // 2 * k, ox + (n // 2 + 1) * k, oy + (n // 2 + 1) * k)
        canvas.itemconfigure(label, text=hex_at(x, y))
        canvas.coords(label, ox + 6, oy + side + 6)
        canvas.coords(label_bg, ox, oy + side, ox + side, oy + side + 30)

    def pick(e):
        state["hex"] = hex_at(e.x, e.y)
        canvas.itemconfigure(label, text=f"Copied {state['hex']}")
        root.after(500, root.destroy)

    root.bind("<Motion>", move)
    root.bind("<Button-1>", pick)
    root.bind("<Button-3>", lambda e: root.destroy())
    root.bind("<Escape>", lambda e: root.destroy())
    root.after(50, root.focus_force)
    root.after(60000, root.destroy)   # never leave a frozen full-screen overlay behind (no focus = no Esc)
    root.mainloop()
    if not state["hex"]:
        return out({"cancelled": True})
    subprocess.run(["clip"], input=state["hex"].encode(), creationflags=0x08000000)   # CREATE_NO_WINDOW
    out({"hex": state["hex"]})


COMMANDS = {
    "color-pick": cmd_color_pick,
    "mixer": cmd_mixer,
    "record": cmd_record,
    "app-icons": cmd_app_icons, "launch": cmd_launch, "pick-image": cmd_pick_image,
    "screen-translate": cmd_screen_translate,
    "focus-widget": cmd_focus_widget, "native-open": cmd_native_open,
    "click-through": cmd_click_through,
    "translate": cmd_translate, "booru": cmd_booru, "download-wallpaper": cmd_download_wallpaper, "open": cmd_open,
    "football": cmd_football, "ai-models": cmd_ai_models, "ai-chat": cmd_ai_chat,
    "secret-set": cmd_secret_set, "secret-status": cmd_secret_status,
    "notifications": cmd_notifications, "notifications-dismiss": cmd_notifications_dismiss,
    "notifications-watch": cmd_notifications_watch,
    "agents": cmd_agents, "agents-watch": cmd_agents_watch,
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
