// Verify gate: an agent that changed the app's code can't end its turn until it has run the real build and read its log.
// `node verify-gate.js prompt` runs on UserPromptSubmit: it notes what the code looked like when the turn started.
// `node verify-gate.js tool` runs on PostToolUse (Bash|PowerShell|Read): it notes when the real app ran and when zorui.log was read.
// `node verify-gate.js stop` runs on Stop: if the code changed this turn, it blocks the stop until
//   1. the release build is newer than every source change and is the installed copy (%LOCALAPPDATA%\zor-ui\zorui.exe),
//   2. the real program ran after the last change (zorui.exe --bridge/--script/..., or the installed app started), and
//   3. zorui.log was read after that run.
// "cargo test" and "cargo build" don't count as running it. See "Verify the work in the real app" in CLAUDE.md.
// shortcut: it checks that the agent ran the app and read the log, not that the feature works; the report must say what it saw.
// After MAX_BLOCKS refusals in one turn it lets the stop through and tells the user what was left unverified, so an agent that
// truly can't verify (it needs the user's voice, say) doesn't loop forever.
const fs = require('fs');
const os = require('os');
const path = require('path');
const crypto = require('crypto');
const { execFileSync } = require('child_process');

const MAX_BLOCKS = 4;
const NOTICE = /<task-notification>|\[SYSTEM NOTIFICATION/;
// running the real program: the bridge or other CLI entry points, or starting the installed app
const RUN = /zorui(\.exe)?["']?\s+--(bridge|script|render|buddy|open)\b|Start-Process\b[^\n|;]*zorui|start\s+[^\n|;]*zorui\.exe|&\s*["'][^"'\n]*zorui\.exe["']/i;
const LOG = /zorui\.log/i;

let input = {};
try { input = JSON.parse(fs.readFileSync(0, 'utf8') || '{}'); } catch {}
const root = process.env.CLAUDE_PROJECT_DIR || input.cwd || process.cwd();
const stateFile = path.join(os.tmpdir(), `claude-verify-${input.session_id || 'nosession'}.json`);
const load = () => { try { return JSON.parse(fs.readFileSync(stateFile, 'utf8')); } catch { return null; } };
const save = (s) => fs.writeFileSync(stateFile, JSON.stringify(s));

// The app's source as it is now: every tracked or new file under native/ (not ignored ones like target/), by size and time.
function source() {
  let files = [];
  try {
    files = execFileSync('git', ['-C', root, 'ls-files', '-co', '--exclude-standard', '--', 'native'], { encoding: 'utf8', windowsHide: true })
      .split('\n').filter(Boolean);
  } catch { return { fp: 'nogit', newest: 0 }; }
  const h = crypto.createHash('sha1');
  let newest = 0;
  for (const f of files.sort()) {
    let st;
    try { st = fs.statSync(path.join(root, f)); } catch { h.update(`${f}:gone\n`); continue; }
    h.update(`${f}:${st.size}:${st.mtimeMs}\n`);
    newest = Math.max(newest, st.mtimeMs);
  }
  return { fp: h.digest('hex'), newest };
}

const fileHash = (p) => crypto.createHash('sha1').update(fs.readFileSync(p)).digest('hex');

const mode = process.argv[2];
if (mode === 'prompt') {
  // a subagent's notice isn't a new request: the work from the user's turn still has to be verified
  if (!NOTICE.test(input.prompt || '')) save({ start: source().fp, ranAt: 0, ranFp: null, logAt: 0, logFp: null, blocks: 0 });
} else if (mode === 'tool') {
  const s = load();
  const t = input.tool_input || {};
  const cmd = t.command || '';
  const ran = RUN.test(cmd);
  const log = LOG.test(cmd) || LOG.test(t.file_path || '');
  if (s && (ran || log)) {
    const fp = source().fp;
    if (ran) { s.ranAt = Date.now(); s.ranFp = fp; }
    if (log) { s.logAt = Date.now(); s.logFp = fp; }
    save(s);
  }
} else if (mode === 'stop') {
  const s = load();
  if (!s) process.exit(0); // no turn start seen (the hook was just added): nothing to compare against
  const now = source();
  if (now.fp === s.start) process.exit(0); // the app's code didn't change this turn
  const missing = [];
  const target = path.join(root, 'native', 'zor-ui', 'target', 'release', 'zorui.exe');
  const installed = path.join(process.env.LOCALAPPDATA || '', 'zor-ui', 'zorui.exe');
  try {
    if (fs.statSync(target).mtimeMs < now.newest) missing.push('The release build is older than your last source change. Run `cargo build --release` in native/zor-ui.');
    else if (!fs.existsSync(installed) || fileHash(target) !== fileHash(installed)) missing.push('The installed app is not this build. Stop the main installed zorui, rename %LOCALAPPDATA%\\zor-ui\\zorui.exe to zorui.exe.old, copy target\\release\\zorui.exe over it and start it.');
  } catch { missing.push('There is no release build. Run `cargo build --release` in native/zor-ui.'); }
  if (s.ranFp !== now.fp) missing.push('You have not run the real program since your last change. Use the feature in the installed app, or run `zorui.exe --bridge <stream> ...`, feeding it a stand-in input (a text-to-speech WAV, a sample file) when the real input needs the user.');
  if (s.logFp !== now.fp || s.logAt < s.ranAt) missing.push("You have not read zorui.log (%LOCALAPPDATA%\\zor-ui\\zorui.log) since that run. Read it and check the feature's lines say what you expect.");
  if (!missing.length) process.exit(0);
  s.blocks = (s.blocks || 0) + 1;
  save(s);
  if (s.blocks > MAX_BLOCKS) {
    console.log(JSON.stringify({ systemMessage: `Verify gate: Claude stopped without verifying its code change. Still missing: ${missing.join(' ')}` }));
  } else {
    console.log(JSON.stringify({
      decision: 'block',
      reason: `Verify gate: you changed the app's code this turn but have not verified it in the real app ("tests pass" and "it compiles" don't count). ${missing.join(' ')} If it doesn't work, fix it and repeat. Then report what you ran and what you saw. If something truly can't be checked from here, say exactly what and why in your report.`,
    }));
  }
}
