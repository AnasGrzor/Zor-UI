"""end4-pC for Windows - AI agent status for the Dynamic Island.

Any coding agent reports to end4 by piping JSON into this script; the island shows when one
needs you (a permission prompt, a question) and the panel under it shows usage limits.

    end4_agent.py <agent>                 reads one JSON object on stdin, records it, prints nothing
    end4_agent.py <agent> --then <cmd>    status line mode: records it, then runs <cmd> with the
                                          same stdin and prints its output (wraps an existing one)
    end4_agent.py --install               adds the hooks and status line to Claude Code and the
                                          hooks to Codex (backs the files up first)

Understood input, whatever the agent or model:
  * hook payloads with "hook_event_name" (Claude Code, Codex and Gemini CLI use the same shape)
  * Codex's notify payload ({"type": "agent-turn-complete"}), also accepted as the last argument
  * Claude Code's status line JSON ("rate_limits": five_hour / seven_day / spend_limit)
  * end4's own: {"session": "id", "state": "waiting|working|done|ended", "message": "...",
                 "usage": {"<window>": {"used_percentage": 0-100, "resets_at": epoch seconds}}}

Writes %LOCALAPPDATA%\\end4-pC\\agents\\<agent>\\<session>.json (and usage.json); the bridge's
agents / agents-watch commands read them. One file per session, so parallel hooks never race.
Standard library only, because hooks run on every tool call.
"""
import json
import os
import re
import sys
import time
from pathlib import Path

ROOT = Path(os.environ.get("END4_ROOT") or Path(os.environ.get("LOCALAPPDATA", Path.home())) / "end4-pC")
AGENTS = ROOT / "agents"

WAITING = {"PermissionRequest", "Elicitation"}
WORKING = {"UserPromptSubmit", "PreToolUse", "PostToolUse", "PostToolUseFailure", "SubagentStart", "SubagentStop",
           "PreCompact", "PostCompact", "ElicitationResult", "BeforeAgent", "BeforeTool", "AfterTool", "BeforeModel",
           "AfterModel", "PreCompress"}
DONE = {"Stop", "StopFailure", "AfterAgent", "agent-turn-complete"}
ENDED = {"SessionEnd"}


def safe(name):
    return re.sub(r"[^\w.-]", "_", str(name))[:80] or "default"


def write(path, data):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(f".{os.getpid()}.tmp")
    tmp.write_text(json.dumps(data), encoding="utf-8")
    os.replace(tmp, path)


def short(v, n=90):
    v = " ".join(str(v).split())
    return v if len(v) <= n else v[: n - 1] + "…"


def ask_text(p):
    """What the agent is asking for, in one line."""
    if p.get("message"):
        return short(p["message"])
    tool, inp = p.get("tool_name") or "", p.get("tool_input") or {}
    if isinstance(inp, dict):
        detail = inp.get("command") or inp.get("file_path") or inp.get("path") or inp.get("url") or inp.get("description") or ""
        if tool == "AskUserQuestion" and inp.get("questions"):
            return short(inp["questions"][0].get("question", "Has a question"))
    else:
        detail = ""
    return short(f"Allow {tool}: {detail}" if detail else f"Allow {tool}?" if tool else "Needs your input")


def state_of(p):
    """(state, message) from a payload, or (None, '') if it carries no state."""
    if p.get("state") in ("waiting", "working", "done", "ended"):
        return p["state"], short(p.get("message") or "")
    ev = p.get("hook_event_name") or p.get("type") or ""
    if ev == "Notification":
        # Claude Code: idle_prompt is the "waiting for your next prompt" reminder, not a question.
        if p.get("notification_type") == "idle_prompt":
            return "done", ""
        return "waiting", ask_text(p)
    if ev in WAITING:
        return "waiting", ask_text(p)
    if ev in WORKING:
        return "working", ""
    if ev in DONE:
        return "done", ""
    if ev in ENDED:
        return "ended", ""
    return None, ""


def usage_of(p):
    lim = p.get("usage") if isinstance(p.get("usage"), dict) else p.get("rate_limits")
    if not isinstance(lim, dict):
        return None
    windows = {k: {"used_percentage": float(v["used_percentage"]), "resets_at": v.get("resets_at")}
               for k, v in lim.items() if isinstance(v, dict) and isinstance(v.get("used_percentage"), (int, float))}
    return windows or None


def record(agent, p):
    d = AGENTS / safe(agent)
    usage = usage_of(p)
    if usage:
        write(d / "usage.json", {"windows": usage, "model": (p.get("model") or {}).get("display_name", "") if isinstance(p.get("model"), dict) else "", "at": time.time()})
    state, message = state_of(p)
    if not state:
        return
    f = d / f"{safe(p.get('session_id') or p.get('session') or p.get('thread-id') or 'default')}.json"
    if state == "ended":
        f.unlink(missing_ok=True)
        return
    write(f, {"state": state, "message": message, "cwd": p.get("cwd") or "", "at": time.time()})


def main(argv):
    if argv[:1] == ["--install"]:
        return install()
    if not argv:
        print(__doc__)
        return 2
    agent, then = argv[0], None
    if "--then" in argv:
        then = argv[argv.index("--then") + 1:]
    raw = sys.stdin.buffer.read() if not sys.stdin.isatty() else b""
    text = raw.decode("utf-8-sig", "replace").strip()   # some shells' pipes add a BOM
    if not text and len(argv) > 1 and argv[-1].startswith("{"):
        text = argv[-1]   # Codex notify passes its JSON as an argument
    try:
        record(agent, json.loads(text) if text else {})
    except (ValueError, OSError, TypeError, KeyError):
        pass   # never break the agent over a status update
    if then:
        import subprocess
        r = subprocess.run(then, input=raw, capture_output=True)
        sys.stdout.buffer.write(r.stdout)
    return 0


# ------------------------------------------------------------------ install
CLAUDE_EVENTS = ["UserPromptSubmit", "PreToolUse", "PostToolUse", "PostToolUseFailure", "PermissionRequest",
                 "Notification", "Elicitation", "ElicitationResult", "Stop", "StopFailure", "SessionEnd"]
CODEX_EVENTS = ["UserPromptSubmit", "PreToolUse", "PermissionRequest", "PostToolUse", "Stop"]


def command(agent):
    # The venv's python.exe only relaunches the base one; hooks run on every tool call, so skip it.
    py = Path(getattr(sys, "_base_executable", "") or sys.executable).as_posix()
    return f'"{py}" -I -S "{Path(__file__).resolve().as_posix()}" {agent}'


def add_hooks(path, agent, events, extra=None):
    """Adds one hook per event to a Claude-style hooks file; returns True if it changed."""
    data = json.loads(path.read_text(encoding="utf-8-sig")) if path.is_file() else {}
    hooks, cmd, changed = data.setdefault("hooks", {}), command(agent), False
    for ev in events:
        groups = hooks.setdefault(ev, [])
        if any("end4_agent.py" in h.get("command", "") for g in groups for h in g.get("hooks", [])):
            continue
        groups.append({"hooks": [{"type": "command", "command": cmd, "timeout": 10, **(extra or {})}]})
        changed = True
    if extra is not None:   # Claude Code: also wrap the status line, which is where usage comes from
        sl = data.get("statusLine") or {}
        if "end4_agent.py" not in sl.get("command", ""):
            then = f" --then {sl['command']}" if sl.get("command") else ""
            data["statusLine"] = {**sl, "type": "command", "command": cmd + then}
            changed = True
    if changed:
        if path.is_file():
            path.with_name(path.name + f".bak-{time.strftime('%Y%m%d-%H%M%S')}").write_bytes(path.read_bytes())
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(data, indent=2, ensure_ascii=False), encoding="utf-8")
    return changed


def install():
    home = Path.home()
    claude_dirs = {home / ".claude"}
    if os.environ.get("CLAUDE_CONFIG_DIR"):
        claude_dirs.add(Path(os.environ["CLAUDE_CONFIG_DIR"]))
    for d in sorted(claude_dirs):
        if d.is_dir():
            print(f"Claude Code {d}: {'added' if add_hooks(d / 'settings.json', 'claude', CLAUDE_EVENTS, {'async': True}) else 'already set up'}")
    if (home / ".codex").is_dir():
        print(f"Codex: {'added' if add_hooks(home / '.codex' / 'hooks.json', 'codex', CODEX_EVENTS) else 'already set up'}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
