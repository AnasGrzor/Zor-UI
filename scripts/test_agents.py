"""Self-check for end4_agent.py + the bridge's agents summary: python scripts/test_agents.py"""
import json, os, subprocess, sys, tempfile, time
from pathlib import Path

HERE = Path(__file__).parent
tmp = Path(tempfile.mkdtemp())
env = {**os.environ, "END4_ROOT": str(tmp)}


def send(agent, payload, *extra):
    r = subprocess.run([sys.executable, str(HERE / "end4_agent.py"), agent, *extra], input=json.dumps(payload).encode(),
                       capture_output=True, env=env)
    assert r.returncode == 0, r.stderr
    return r.stdout.decode()


sys.path.insert(0, str(HERE))
import end4_bridge as b
b.AGENTS_DIR = tmp / "agents"
by = lambda: {a["id"]: a for a in b.agents_summary()}

send("claude", {"hook_event_name": "PreToolUse", "session_id": "s1", "tool_name": "Bash"})
assert by()["claude"]["state"] == "working"
send("claude", {"hook_event_name": "PermissionRequest", "session_id": "s1", "tool_name": "Bash", "tool_input": {"command": "npm test"}})
a = by()["claude"]
assert a["state"] == "waiting" and a["message"] == "Allow Bash: npm test", a
send("claude", {"hook_event_name": "Notification", "session_id": "s2", "notification_type": "idle_prompt", "message": "waiting"})
assert by()["claude"]["waiting"] == 1
send("claude", {"hook_event_name": "PostToolUse", "session_id": "s1"})
assert by()["claude"]["state"] == "working"
send("claude", {"hook_event_name": "Stop", "session_id": "s1"})
assert by()["claude"]["state"] == "idle"
# status line: usage recorded, wrapped command's output passed through
later = time.time() + 3600
o = send("claude", {"model": {"display_name": "Opus"}, "rate_limits": {"five_hour": {"used_percentage": 23.5, "resets_at": later},
                                                                       "seven_day": {"used_percentage": 41, "resets_at": time.time() - 5}}},
         "--then", sys.executable, "-c", "import sys; print('wrapped ' + str(len(sys.stdin.read()) > 0))")
assert o.strip() == "wrapped True", o
a = by()["claude"]
assert a["usage"] == {"five_hour": {"used_percentage": 23.5, "resets_at": later}} and a["model"] == "Opus", a   # past reset dropped
# any other agent, end4's own format; Codex notify as an argument
send("my-bot", {"session": "x", "state": "waiting", "message": "Approve deploy?"})
assert by()["my-bot"]["label"] == "My-Bot" and by()["my-bot"]["message"] == "Approve deploy?"
subprocess.run([sys.executable, str(HERE / "end4_agent.py"), "codex", '{"type": "agent-turn-complete", "thread-id": "t"}'], env=env, stdin=subprocess.DEVNULL, check=True)
assert by()["codex"]["state"] == "idle"
send("claude", {"hook_event_name": "SessionEnd", "session_id": "s1"})
send("claude", {"hook_event_name": "SessionEnd", "session_id": "s2"})
assert by()["claude"]["waiting"] == 0 and by()["claude"]["state"] == "idle"   # usage keeps it listed
send("broken", {}); subprocess.run([sys.executable, str(HERE / "end4_agent.py"), "broken"], input=b"not json", env=env, check=True)
print("ok")
