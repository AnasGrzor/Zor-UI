// Plan-first gate. `node plan-gate.js prompt` runs on UserPromptSubmit,
// `node plan-gate.js gate` runs on PreToolUse (Edit|Write|NotebookEdit).
// The flag lasts the session: once a go-ahead is given, later prompts keep it (relaxed by the user; see git history for the one-prompt rule).
// Background-task notices (a subagent finishing) also arrive as prompts; they leave the flag alone,
// so subagents and the work after them keep the go-ahead the user gave.
// shortcut: keyword match, so "how would you implement X?" also counts as a go-ahead; tighten GO if that bites.
const fs = require('fs');
const os = require('os');
const path = require('path');

const GO = /\b(go ahead|go for it|do it|implement|proceed|start coding|code it|build it|write (it|the|a)|make (it|the change|the changes)|apply (it|the plan|the changes))\b/i;
const NOTICE = /<task-notification>|\[SYSTEM NOTIFICATION/;
// Plan files (a .md file in a "plans" folder) are what planning writes, so they are never blocked.
const PLAN_FILE = /[\\/]plans[\\/][^\\/]+\.md$/i;

let input = {};
try { input = JSON.parse(fs.readFileSync(0, 'utf8') || '{}'); } catch {}
const flag = path.join(os.tmpdir(), `claude-go-${input.session_id || 'nosession'}`);
const isPlanFile = PLAN_FILE.test((input.tool_input && input.tool_input.file_path) || '');

if (process.argv[2] === 'prompt') {
  if (NOTICE.test(input.prompt || '')) {}
  else if (GO.test(input.prompt || '')) fs.writeFileSync(flag, '1');
  // no go-ahead in this prompt: the flag from an earlier go-ahead stays
} else if (!fs.existsSync(flag) && !isPlanFile) {
  console.log(JSON.stringify({
    hookSpecificOutput: {
      hookEventName: 'PreToolUse',
      permissionDecision: 'deny',
      permissionDecisionReason: 'Plan-first rule: no code changes until the user explicitly asks. Give a plan and wait for a go-ahead.',
    },
  }));
}
