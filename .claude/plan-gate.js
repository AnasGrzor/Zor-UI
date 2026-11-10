// Plan-first gate. `node plan-gate.js prompt` runs on UserPromptSubmit,
// `node plan-gate.js gate` runs on PreToolUse (Edit|Write|NotebookEdit).
// The flag lasts one prompt: every new prompt clears it unless it contains a go-ahead.
// Background-task notices (a subagent finishing) also arrive as prompts; they leave the flag alone,
// so subagents and the work after them keep the go-ahead the user gave.
// shortcut: keyword match, so "how would you implement X?" also counts as a go-ahead; tighten GO if that bites.
const fs = require('fs');
const os = require('os');
const path = require('path');

const GO = /\b(go ahead|go for it|do it|implement|proceed|start coding|code it|build it|write (it|the|a)|make (it|the change|the changes)|apply (it|the plan|the changes))\b/i;
const NOTICE = /<task-notification>|\[SYSTEM NOTIFICATION/;

let input = {};
try { input = JSON.parse(fs.readFileSync(0, 'utf8') || '{}'); } catch {}
const flag = path.join(os.tmpdir(), `claude-go-${input.session_id || 'nosession'}`);

if (process.argv[2] === 'prompt') {
  if (NOTICE.test(input.prompt || '')) {}
  else if (GO.test(input.prompt || '')) fs.writeFileSync(flag, '1');
  else fs.rmSync(flag, { force: true });
} else if (!fs.existsSync(flag)) {
  console.log(JSON.stringify({
    hookSpecificOutput: {
      hookEventName: 'PreToolUse',
      permissionDecision: 'deny',
      permissionDecisionReason: 'Plan-first rule: no code changes until the user explicitly asks. Give a plan and wait for a go-ahead.',
    },
  }));
}
