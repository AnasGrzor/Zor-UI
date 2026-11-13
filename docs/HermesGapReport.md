# Hermes vs Buddy gap report

## Context

The user says Hermes Agent (Nous Research) has every feature they want in Buddy. `docs/BuddyIntelPlan.md` says "Hermes: Do not integrate", so this report keeps the native Rust agent and lists what to port.

Baseline: the `integration-all` branch (`D:/zor-ui/zor-ui-all`, paths under `native/zor-ui/src/`).
Hermes sources: the GitHub README and the docs pages on messaging, skills, memory, tools and cron.

## 1. Messaging gateway

| Hermes | Buddy (integration-all) | Gap |
|---|---|---|
| One gateway process for Telegram, Discord, Slack, WhatsApp, Signal, Email, SMS, Matrix, Teams and about 15 more | Outbound only. The `message` tool opens a `whatsapp://` or `discord://` link with the text filled in. The user presses Enter (`assist.rs:516`). | No inbound channel. A Discord bot exists on the unmerged branch `worktree-discord-buddy` (Allow/Deny buttons, background sync), planned in `docs/MessageIntegrationPlan.md`. It is not in integration-all. |
| Each chat keeps a persistent session, with `/new`, `/resume`, `/sessions`, `/bg` | One global `TURNS` store: 6 turns, dropped after 600 s (`agent.rs:43-48`) | Needs a session store per channel |
| Allowlist plus DM pairing codes, `/approve` and `/deny` | Allow/Deny buttons on the Discord branch | Pairing and allowlist not checked |
| Voice memo transcription and TTS replies | Voice module exists (`voice/`). Chat voice not checked. | Voice over chat unverified |
| Delivery ledger resends replies after a crash | None found | Missing |

## 2. Skills

| Hermes | Buddy | Gap |
|---|---|---|
| The agent writes skills itself with `skill_manage` (create, patch, delete). A background review runs after tasks. | 7 fixed skills compiled into the binary (`agent/skills.rs:41-143`) | No self-written skills, no learning loop |
| `SKILL.md` with YAML frontmatter, `references/` and `scripts/` folders, follows the agentskills.io standard | Rust struct with trigger words and an eval (`skills.rs:23-39`) | No skills on disk, no standard format |
| Progressive loading: `skills_list`, then `skill_view` | Whole-word trigger match adds guidance lines to the user message (`skills.rs:146-153`) | No skill-browsing tool |
| Write approval, `/skills approve`, curator lock | Permission per skill, checked by test | No staging for skills the agent writes |
| Skills Hub install with a security scan, `/learn` from URLs and folders | None | Missing |
| Each skill becomes a `/skill-name` command | None | Missing |

## 3. Memory and session search

| Hermes | Buddy | Gap |
|---|---|---|
| `MEMORY.md` (2,200 chars) and `USER.md` (1,375 chars), loaded at session start, edited with the `memory` tool | SQLite `memories` table with an FTS5 index. Kinds: preference, project, semantic. Near-duplicates replace each other, secrets are filtered, and at most 6 facts / 600 chars enter the prompt (`agent/memory.rs`). | Same idea, and Buddy is stricter. No separate user-profile document. |
| `session_search`: FTS5 over every past session in `state.db` | `turns` keeps only 6 rows (`agent/history.rs`). FTS covers facts only. | **Biggest gap.** Past conversations cannot be searched. |
| Honcho user modeling as a plugin. Mem0, Supermemory and others also plugins. | `Ranker` trait, keyword search only (`memory.rs:291`) | Honcho is planned in `BuddyIntelPlan.md` §8, not built |
| Nudge settings (`memory.nudge_interval`) and a background self-review, which can run on a cheaper model | None | Missing |
| `/journey` timeline to view or edit learned items | None (Settings page not checked) | Missing |
| Delegation summarizes long tool results | `delegate.rs` cuts results over 6,000 chars to about 150 words | Same feature |

## 4. Cron, subagents and tools

| Hermes | Buddy | Gap |
|---|---|---|
| Cron in plain language ("every 2h", "weekdays at 9am"), delivery to any platform, jobs in `jobs.json`, fresh session per run | Break reminder on a fixed timer only (`buddy.rs:452`). `tasks.rs` resumes tasks after a crash. | No scheduler. The user cannot set reminders. |
| `delegate_task` runs isolated subagents in parallel, each with its own model and limits | Planner with up to 8 steps and replanning (`agent/plan.rs`), plus a summarizer call | No subagents, no parallel runs |
| `execute_code`: Python scripts that call tools over RPC | None | Missing |
| Six terminal backends (local, Docker, SSH, Modal, Daytona, Vercel Sandbox), PTY, background `process` tool | `run_check` allows only cargo check, test, build and clippy (`agent/developer.rs`) | No general shell. This matches the policy design. |
| Connects any MCP server, `/reload-mcp` | stdio MCP client, used only for Playwright (`mcp.rs`, `browse.rs:24`) | Client code exists. No setting to add servers. Small gap. |
| CDP browser tools | Edge through Playwright MCP, 11 browser tools (`browse.rs`) | Same feature |
| Many providers through `hermes model` | 8 providers, strong-model routing, fallback chain (`agent/routing.rs`, `bridge/web.rs`) | Same feature |
| Approval for dangerous commands | Four-level policy that fails closed on unknown tools (`agent/policy.rs`) | Buddy is stricter |

## Where Buddy is already ahead or equal

- Windows desktop tools: media, volume, wallpaper, `see_screen`, `take_over`, app launching
- Policy with risk levels, and no retries after side effects
- Verifier and resumable tasks
- Secret filter on memory
- Island and desktop UI with personalities

## Gaps ranked by value for the cost

1. **Session search.** Keep all turns in SQLite with FTS5 and add a `session_search` tool. `memory.rs` already has the FTS code to reuse. Small change.
2. **Merge the Discord gateway** from `worktree-discord-buddy`, then add a session store per channel. Most of the code exists.
3. **Cron and reminders.** A job table in `buddy.db`, a timer thread, and a `schedule` tool at Reversible risk. Deliver to the island or to Discord.
4. **User-chosen MCP servers.** A setting that lists stdio servers, reusing `mcp.rs`. Every MCP tool gets Consequential risk unless the user marks it safe.
5. **Self-written skills.** Store SKILL.md files in `%LOCALAPPDATA%\zor-ui\skills`, add `skill_manage` with write approval, and load them next to the built-in skills.
6. **Subagents.** Run the existing agent loop in a thread with a smaller tool set and return results. Highest risk, because of the policy and approval flow.
7. **Later:** Honcho user model, background self-review, `/journey` view. These match `BuddyIntelPlan.md` §8.

## Verification

This is a report only, with no code changes. Before acting on it, check the cited `file:line` references in `D:/zor-ui/zor-ui-all`.

Not read: `agent/intent.rs`, `agent/verifier.rs`, and the Settings pages (for hidden bot settings or journey-style views).
