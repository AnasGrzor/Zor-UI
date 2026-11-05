"""Chat through the `claude` command (Claude Code), so the sidebar and the buddies can use your Claude
subscription instead of an API key. Models are "claude-cli:haiku", "claude-cli:sonnet", "claude-cli:opus".

The CLI runs with no tools, no hooks, no MCP servers and no saved session, from a scratch folder, so a
chat message can't touch your files or pick up your Claude Code setup.
"""
import json
import os
import shutil
import subprocess
import tempfile

ALIASES = ("haiku", "sonnet", "opus")
MODELS = [{"id": f"claude-cli:{a}", "name": f"Claude subscription: {a.capitalize()}", "provider": "claude-cli"} for a in ALIASES]


def find():
    """Path of the claude command, or "" when Claude Code isn't installed."""
    return shutil.which("claude") or ""


def alias(model_id):
    a = model_id.split(":", 1)[-1]
    return a if a in ALIASES else "haiku"


def prompt(messages):
    """The conversation as one prompt: just the text for a single message, else a labelled transcript."""
    turns = [m for m in messages if m.get("content")]
    if len(turns) == 1:
        return turns[0]["content"]
    return "\n\n".join(f"{'Assistant' if m['role'] == 'assistant' else 'User'}: {m['content']}" for m in turns) + "\n\nAssistant:"


def read_line(line):
    """One stream-json line from the CLI: ("delta", text), ("done", text), ("error", message) or None."""
    try:
        ev = json.loads(line)
    except ValueError:
        return None
    if ev.get("type") == "stream_event":
        d = (ev.get("event") or {}).get("delta") or {}
        if d.get("type") == "text_delta" and d.get("text"):
            return ("delta", d["text"])
    if ev.get("type") == "result":
        text = str(ev.get("result") or "")
        return ("error", text or "Claude Code returned an error. Run `claude` once to log in.") if ev.get("is_error") else ("done", text)
    return None


def chat(model_id, messages, system, emit):
    """Streams the reply through emit({"delta": ...}) / emit({"error": ...})."""
    exe = find()
    if not exe:
        return emit({"error": "Claude Code isn't installed, so the subscription can't be used. Install it and run `claude` once to log in."})
    cmd = [exe, "-p", "--output-format", "stream-json", "--verbose", "--include-partial-messages", "--model", alias(model_id),
           "--tools", "", "--system-prompt", system, "--setting-sources", "", "--no-session-persistence",
           "--disable-slash-commands", "--strict-mcp-config"]
    p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, encoding="utf-8",
                         cwd=tempfile.gettempdir(), creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0))
    try:
        p.stdin.write(prompt(messages))
        p.stdin.close()
        got = False
        for line in p.stdout:
            r = read_line(line)
            if not r:
                continue
            kind, text = r
            if kind == "delta":
                got = True
                emit({"delta": text})
            elif kind == "error":
                return emit({"error": text})
            elif not got and text:  # no partial chunks: send the whole answer
                emit({"delta": text})
        if p.wait(timeout=5) != 0 and not got:
            emit({"error": "Claude Code stopped without an answer. Run `claude` once to log in."})
    finally:
        if p.poll() is None:
            p.kill()
