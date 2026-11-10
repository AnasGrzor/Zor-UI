# Buddy: how it works

The buddy is the voice and chat assistant in Zor UI. This page covers what is built, how the parts fit together, the settings
that control them, and how to measure them. The longer plan is in `BuddyIntelPlan.md`.

## Requests

A request comes in by voice or text. `assist::parse` handles plain commands ("open Chrome", "turn up the volume") directly,
without the model. Anything else goes to `agent::ask`, which runs one of two loops:

- **JSON protocol** (default, works with every provider and with the Claude subscription). The model replies with one JSON
  object per turn: `{"do": "tool", ...}` to act, or `{"say": "..."}` to answer. `parse_step` validates each reply.
  Up to 12 steps and 90 seconds.
- **Native tool calling** (Claude API only). The model gets the tools as API tool definitions and replies with `tool_use`
  blocks. Each result goes back as a `tool_result` block with its id. A plain text reply ends the request. Turned on by
  `ai.nativeTools`, and only when the model is served by Anthropic and the browser is off.

Both loops run the same tools through `exec`, so the same confirmations and switches apply to each.

## Tools

21 tools, the same in both loops: `search`, `web_search`, `read_url`, `open`, `find_app`, `open_url`, `play`, `media`,
`message`, `volume`, `brightness`, `radio`, `power`, `clipboard`, `find_file`, `open_file`, `wallpaper`, `see_screen`,
`take_over`, `remember`, `forget`. The browser tools are only in the JSON loop.

Risk levels (`agent/policy.rs`, one `match` over every tool): read-only (search, find, see the screen), reversible (open,
volume, play, remember, forget, message), consequential (take over, browser), destructive (sleep, restart, shut down, sign
out). Only consequential and destructive calls need approval. The island confirmations still do the asking today; the log
records each call's level, not its arguments.

Safety:
- `power` asks in the island before anything except lock. A refusal does nothing.
- `take_over` asks in the island before it moves the mouse or keyboard.
- `message` opens a chat with the text ready. The user presses Enter to send.
- `message` text is the user's own words. The prompt says so, and the live eval checks it.
- `remember` and `forget` do nothing while `ai.memory` is off.

## Play a song

`playsong::play_song` tries routes in order and reports the first that starts a new track:
1. the Spotify app, when installed or running;
2. a Spotify web tab: an open one (your login is in it), or a new one opened to the Spotify search page;
3. a YouTube tab that is already open;
4. a new YouTube search.

"Started" means the Windows media session shows a new track playing within 10 seconds. The play button's label is not trusted.
A failed route falls through to the next one, and the answer lists what was tried.

## Memory

Facts the user asks Buddy to keep, in `%LOCALAPPDATA%\zor-ui\buddy.db` (SQLite, FTS5 full-text search).
- `remember` saves a fact. A fact with at least 60% of its words in common with a stored one replaces it.
- Text that looks like a password, key, PIN, card or account number is refused, and so is a run of 12 or more digits.
- `forget` deletes the stored fact that best matches the request.
- Recall adds up to five matching facts to the user's turn. Common words ("the", "what") don't count.
- Settings > AI lists the 30 newest facts with a delete button. The list reads a cache; every write clears it.
- A JSON store from the earlier build is imported once, then renamed `buddy-memory.json.migrated`.

## Verifier

A tool's "ok" is not proof that the request worked. `agent/verifier.rs` checks each tool result against what the tool
reports:
- **Verified:** a play that `playsong` confirmed ("Playing ..."), a saved or forgotten fact, a search with results, a file found.
- **Unverified:** the device or app did something, but nothing reports what it did (volume, opening an app, take-over).
- **Failed:** the tool errored, or found nothing.

The checks give one verdict for the request: completed, unverified, partial, or failed. The verdict is logged as
`buddy verdict`. A partial or failed request also gets a short note added to the answer ("Only part of that was confirmed.").
Unverified actions are not called out, so a volume change doesn't come with a complaint.

## Tasks and states

Each request is a task, saved in the `tasks` table with its request text, state, verdict and tool-call count. A request
that looks like a secret is saved as "[private request]". The task moves through the states in `agent/state.rs`: Planning,
Running, WaitingApproval (while the island asks), Verifying, then Completed, Failed or Cancelled. A move that skips a state
is refused and logged. A task still open when the app closed is marked Interrupted on the next start. The Settings > AI
page lists the 10 newest requests with their state, verdict and tool calls.

## Retries and fallbacks

- **Retries.** A read-only tool that fails (a search, a page read) is tried once more after 400 ms. Anything that acts is
  never repeated (`policy::may_retry`).
- **Replanning.** A failed tool's error goes back to the model, which picks another route. The loop itself does not plan.
- **Model fallback.** If the model fails before any tool has run, the next model in the fallback list (`ai.fallback1`, `ai.fallback2`) tries the same
  request. After a tool has run, the request is never retried on another model.
- **Routing.** A request with words like "then", "compare" or "research" goes to `ai.strongModel` when that is set.

## Context and skills

`AgentContext` (in `agent.rs`) holds what a request needs: the clock, the window in front, the player, and matching facts.
Each part is filled only when the request asks for it. `skills.rs` has three skills with their tools and short guidance:
research (search, read pages), desktop (apps, volume, brightness, media, screen, take-over) and personal (remember, forget,
files, messages, music). A skill's guidance goes into the request only when its words appear, so most requests carry none.

## Conversation turns

The last six answered turns are saved to the `turns` table and restored on the first request after a restart. Turns older
than ten minutes are dropped, and turns that look like secrets are never saved.

## Prompt layout and caching

The system text is fixed: the base rules, the user's own instructions (`ai.systemPrompt`), and the browser rules when the
browser is on. Providers can reuse a cached copy of it between requests.

Per-request parts go in the user turn (`request_text`):
- the clock, the front window and the player, each only when the request's words ask for it (`wants_clock`, `wants_window`,
  `wants_media`). A plain request like "turn the volume up" gets none of them;
- the recalled facts, only when they match the request.

Measured on haiku: when the system text stayed the same, the second call read 1,590 tokens from the cache. When the time was
in the system text, nothing was read.

## Settings

| Key | Default | What it does |
|---|---|---|
| `ai.model` | `gemini-2.5-flash` | model for chat and tools |
| `ai.temperature` | 0.5 | sampling temperature for the agent loop and screen questions (take-over stays at 0.2) |
| `ai.systemPrompt` | empty | your own instructions; they never override the reply format |
| `ai.nativeTools` | false | native tool calling, Claude API only |
| `ai.memory` | true | remember and recall facts, on this PC only |
| `ai.strongModel` | empty (Off) | model for multi-step requests; picked in Settings > AI |
| `ai.fallback1`, `ai.fallback2` | Off | two fallback models, from dropdowns in Settings > AI; tried in order when the chosen model fails before any tool runs |
| `ai.fallbackModels` | empty | optional extra list in `config.json` only, tried after the two above |

## Logs

Each request logs `buddy request`, `buddy native turn` or `buddy AI reply`, each tool call, `buddy timing: ... total, ... in the
model, ... in N tool call(s)`, `buddy answer` or `buddy failed`, and `buddy status` (Completed, Failed or Cancelled). The log
is `%LOCALAPPDATA%\zor-ui\zorui.log`.

## Evals

- **Protocol cases** (`eval/protocol-cases.json`), run with `cargo test`. Replies the parser must read correctly. Offline.
- **Live cases** (`eval/live-cases.json`). Real requests sent through your Claude subscription, no API key:
  `cargo test live_protocol_eval -- --ignored --nocapture`. Set `ZORUI_EVAL_MODEL` to choose the model (default haiku).
  A case can require a word in the step with `contains`.
- **Cache check.** `cargo test live_cache_layout_eval -- --ignored --nocapture` sends five requests in the old and new layouts
  and prints the cached tokens for each call.

Live evals spend subscription usage, so they are ignored by default.

## Subscription and API keys

The subscription path runs the `claude` command with no tools, so it gives text replies only. It works with the JSON
protocol. Native tool calling needs an API key, because the tool definitions go through the API.

## Not built yet

- **A separate planner.** The model plans each step itself, one tool at a time, and the loop carries it out. A second planning
  call would cost a model round trip on every request, so it is not built until the eval shows the loop losing track.
- **Developer skill.** It needs code tools (read a project, run tests). The buddy has none yet.
- **Semantic memory.** The `Recall` trait is in place, but an embedding model would have to be downloaded, and the privacy
  rule keeps facts off providers, so it needs your decision on a local model first.
- **Browser tools and other providers in native mode.** Native mode is Claude-only with the browser off.
- **Active project in the context.** The program in front is read; no project is detected.
- **Metrics beyond the logs and the tasks table.** The eval scripts are the benchmarks; no dashboard.
- **Manage tasks by voice.** The Settings list shows them and can cancel one. A cancel stops the request between model calls;
  a tool that is already running finishes first.
