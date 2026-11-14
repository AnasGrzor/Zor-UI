# Zor-UI — Buddy Intelligence Core v2

## Codebase Audit &amp; Implementation Brief

### Mission

Upgrade Zor-UI Buddy from a tool-using desktop assistant into a context-aware, highly autonomous AI agent.

The goal is simple:

**The user should describe the desired outcome. Buddy should infer the intent, gather relevant context, plan the work, execute it, adapt when things go wrong, and verify the result.**

The user should not need to specify every tool, file, intermediate step, or implementation detail.

This is an incremental upgrade to the existing Rust architecture, not a rewrite.

---

## 1. Architectural decisions

These decisions are established:

- Agent runtime: Existing native Rust agent.
- Hermes: Do not integrate.
- Autonomy: Balanced and context-adaptive.
- Model routing: Automatic and capability-aware.
- Memory: Persistent.
- User modeling: Honcho integration, optional and replaceable.
- Operational memory: Existing SQLite implementation.
- Supermemory: Do not integrate yet; keep the architecture extensible.
- Model providers: Provider-agnostic.
- Voice: Preserve the existing provider-independent voice architecture.
- UI: Preserve the existing Buddy interface and personalities.
- Performance: Minimize RAM, CPU, context size, and unnecessary model calls.

Honcho must not become a mandatory dependency. Buddy must continue working without it.

## 2. Existing implementation to preserve

Inspect and reuse the existing components before creating alternatives:

- `native/zor-ui/src/[agent.rs](http://agent.rs)`
- `native/zor-ui/src/agent/[memory.rs](http://memory.rs)`
- `native/zor-ui/src/agent/[history.rs](http://history.rs)`
- `native/zor-ui/src/agent/[routing.rs](http://routing.rs)`
- `native/zor-ui/src/agent/[policy.rs](http://policy.rs)`
- `native/zor-ui/src/agent/[tasks.rs](http://tasks.rs)`
- `native/zor-ui/src/agent/[verifier.rs](http://verifier.rs)`
- `native/zor-ui/src/agent/[state.rs](http://state.rs)`
- `native/zor-ui/src/agent/[skills.rs](http://skills.rs)`
- `native/zor-ui/src/agent/[native.rs](http://native.rs)`
- `native/zor-ui/src/[assist.rs](http://assist.rs)`
- `native/zor-ui/src/bridge/[web.rs](http://web.rs)`
- `native/zor-ui/src/voice/`

Preserve existing provider integrations, SQLite data, Windows capabilities, settings, and working features.

Do not replace working functionality just to introduce a cleaner abstraction.

---

## 3. Priority 0 — Reliability and safety

Implement these before expanding autonomy.

### 3.1 Make verification authoritative

Current issue identified during the codebase audit:

`state::status_of()` can mark a task Completed simply because the agent returned `Ok`, even when `verifier::judge()` determines that the outcome is Partial or Failed.

Fix this inconsistency.

Requirements:

- Persist the verifier's verdict as part of the task result.
- Distinguish Completed, Unverified, Partial, Failed, and Cancelled.
- Require evidence for successful completion.
- Verify actual outcomes rather than trusting tool return messages.
- Prevent unsafe retries after actions with side effects.
- Ensure the final response accurately reflects what was accomplished.

A successful model response must never automatically imply a successful task.

### 3.2 Centralize permission enforcement

The existing risk classifications are:

- ReadOnly
- Reversible
- Consequential
- Destructive

Make the policy layer authoritative.

Rules:

- Read-only actions: automatic.
- Reversible actions: automatic within approved scope.
- Consequential actions: require approval.
- Destructive actions: explicit confirmation.
- Unknown or unclassified tools: fail closed.

All execution paths, including native tools, browser tools, developer tools, and future integrations, must pass through the same policy boundary.

Do not allow individual tools to bypass it.

---

## 4. Priority 1 — Context-aware intent engine

This is the most important intelligence upgrade.

Buddy currently relies heavily on keyword-based context selection.

Replace that approach with progressive context gathering.

### Context sources

Where available and permitted, gather:

- Active application.
- Foreground window.
- Current project and working directory.
- Relevant open files.
- Recent file activity.
- Current screen or selected content when necessary.
- Recent conversation.
- Current and previous tasks.
- Relevant user preferences.
- Relevant project memories.

Do not capture everything indiscriminately.

### Intent inference

Buddy must understand incomplete and vague requests such as:

- "Fix this."
- "Make it better."
- "Continue where we left off."
- "Get this ready for tomorrow."
- "Build something like that."
- "Clean up my project."

For each request:

1. Identify the likely desired outcome.
2. Resolve references using conversation and environment context.
3. Retrieve relevant memories.
4. Determine whether additional context is needed.
5. Estimate confidence in the inferred intent.
6. Decide whether to proceed, inspect further, or ask one concise question.

Do not ask the user to explain details Buddy can reasonably discover itself.

When several interpretations are possible, prefer the most likely reversible path and explain the assumption.

Never infer permission for consequential or destructive actions.

---

## 5. Priority 1 — Structured planning and execution

Upgrade the current sequential tool loop into a lightweight planning system.

Each task should contain:

- User's original request.
- Inferred objective.
- Relevant context.
- Execution steps.
- Dependencies.
- Expected results.
- Completion criteria.
- Required permissions.
- Current progress.
- Verification evidence.

Requirements:

- Support multi-step tasks.
- Persist plans and progress.
- Recover interrupted tasks safely.
- Adapt plans when tools fail.
- Re-plan when new information changes the approach.
- Avoid blindly replaying side effects.
- Preserve the existing fast path for simple requests.

Do not introduce a heavyweight agent framework.

Use model reasoning for planning, with Rust responsible for execution, state, permissions, and verification.

---

## 6. Priority 1 — Adaptive autonomy

Autonomy should adapt to the request and its risks.


| Situation                            | Behavior                              |
| ------------------------------------ | ------------------------------------- |
| Clear, harmless request              | Act immediately                       |
| Vague request with strong context    | Infer and act                         |
| Missing but discoverable information | Investigate first                     |
| Multiple reasonable interpretations  | Choose the likely safe interpretation |
| Consequential action                 | Request approval                      |
| Destructive action                   | Require explicit confirmation         |
| Failed execution                     | Diagnose and adapt                    |


Buddy should not ask unnecessary questions.

The objective is to minimize user involvement without sacrificing control or safety.

---

## 7. Priority 1 — Provider-agnostic model architecture

Create a normalized Rust model-provider interface.

It should support:

- Text generation.
- Streaming.
- Tool calling.
- Cancellation.
- Model capability information.
- Context limits.
- Usage and token information.
- Latency and error reporting.
- Provider fallback.

Retain existing integrations and authentication.

### Automatic routing

Routing should consider:

- Task complexity.
- Coding requirements.
- Vision requirements.
- Tool-calling requirements.
- Context size.
- Provider availability.
- Latency.
- Cost.

Use lightweight heuristics initially.

Do not require an additional LLM classification call for every request.

Keep primary-model and strong-model compatibility with existing settings.

---

## 8. Priority 2 — Honcho memory integration

Integrate Honcho as an optional user-modeling backend.

Its responsibilities:

- Learn user preferences.
- Understand communication style.
- Identify recurring patterns.
- Maintain evolving user profiles.
- Understand goals and preferences over time.
- Provide relevant user context during intent inference.

Keep SQLite as the source of truth for:

- Task state.
- Execution history.
- Local settings.
- Operational memory.
- Recent conversation history.

### Memory architecture

Separate:

1. Semantic memory — stable user facts.
2. Preference memory — how the user likes things done.
3. Episodic memory — previous interactions and events.
4. Project memory — decisions, architecture, and ongoing work.
5. Working memory — current task context.

Requirements:

- Selective memory extraction.
- Duplicate detection.
- Confidence and provenance.
- Memory consolidation.
- Expiration of outdated information.
- User-controlled forgetting.
- Sensitive-data filtering.
- Configurable external transmission.
- Bounded retrieval.

Do not send all local memory to Honcho or to model providers.

Create a replaceable memory-provider interface.

Do not introduce Supermemory in this milestone.

---

## 9. Priority 2 — Developer capabilities

Buddy currently lacks a complete developer toolset.

Introduce a permission-controlled Developer skill.

Potential capabilities:

- Inspect project directories.
- Read and edit files.
- Search code.
- Understand project structure.
- Execute terminal commands.
- Run tests.
- Build projects.
- Diagnose errors.
- Review diffs.
- Verify changes.

Requirements:

- Workspace-scoped access.
- Command risk classification.
- Explicit approval for risky operations.
- Safe handling of secrets.
- No unrestricted shell execution.
- Verification after changes.

Buddy should be able to understand and work on its own Zor-UI codebase when the user authorizes access.

---

## 10. Priority 2 — Skills and lightweight delegation

Expand the existing skills system.

Preserve Research, Desktop, and Personal skills.

Introduce:

- Developer.
- Project management.
- Browser research.
- Workflow automation.

Skills should have:

- Clear capability descriptions.
- Required tools.
- Permissions.
- Context requirements.
- Reusable workflows.
- Versioning.
- Evaluation criteria.

Support lightweight task delegation when it improves performance.

Do not spawn unnecessary subagents for simple requests.

---

## 11. Performance requirements

Minimal resource consumption is a core requirement.

- No unnecessary background polling.
- No always-running heavyweight agent runtime.
- No unnecessary model calls.
- Bounded conversation context.
- Lazy-load skills and providers.
- Reuse existing SQLite infrastructure.
- Keep idle memory consumption low.
- Use asynchronous operations where appropriate.
- Avoid duplicate context gathering.
- Measure memory and CPU before and after changes.

Do not add a vector database by default.

Only consider embeddings or external memory infrastructure when measured retrieval improvements justify their cost.

---

## 12. Implementation order

Work in small, independently testable milestones.


| Milestone | Deliverable                                |
| --------- | ------------------------------------------ |
| M0        | Fresh codebase audit and baseline          |
| M1        | Verification and task-state reliability    |
| M2        | Centralized permission enforcement         |
| M3        | Progressive context engine                 |
| M4        | Intent inference                           |
| M5        | Structured planning and recovery           |
| M6        | Adaptive autonomy                          |
| M7        | Provider abstraction and automatic routing |
| M8        | Optional Honcho integration                |
| M9        | Developer skill                            |
| M10       | Skills, delegation, and evaluation         |
| M11       | Performance optimization                   |


Do not implement every milestone in one enormous change.

Start with M0 and M1. Report findings before proceeding to M2.

---

## 13. Testing and acceptance criteria

Every milestone must include tests.

### Intent tests

Buddy should correctly resolve vague instructions using supplied conversation and environment context.

### Planning tests

Buddy should produce executable plans, recover interrupted tasks, and adapt when steps fail.

### Safety tests

Unknown tools must fail closed. Consequential and destructive actions must not execute without the required approval.

### Verification tests

A task must never be marked Completed when its required outcome is not verified.

### Memory tests

Memory retrieval should be relevant, bounded, privacy-aware, and independent of Honcho availability.

### Provider tests

Switching model providers must not require changes to the agent orchestration code.

### Performance tests

Measure:

- Idle RAM.
- Active RAM.
- CPU usage.
- Context size.
- Token consumption.
- Model calls per task.
- Task latency.

Set concrete baselines before optimization.

---

## 14. Non-negotiable constraints

- No Hermes integration.
- No full agent rewrite.
- No replacement of existing SQLite memory.
- No mandatory Honcho dependency.
- No Supermemory integration yet.
- No removal of existing model providers.
- No breaking existing voice functionality.
- No breaking Buddy personalities or UI.
- No bypassing permission checks.
- No blind retries of side-effecting operations.
- No unnecessary heavyweight frameworks.
- No unnecessary background processes.
- No unrelated UI redesign.

---

## 15. Required deliverables

For every milestone, report:

1. What was inspected.
2. What was changed.
3. Files created or modified.
4. Architectural decisions.
5. Dependencies introduced or removed.
6. Tests added.
7. Tests executed and their actual results.
8. Build results.
9. Performance impact.
10. Remaining limitations.

Update the relevant architecture documentation and `docs/[BuddyIntelPlan.md](http://BuddyIntelPlan.md)`.

Do not claim tests or builds passed unless they were actually executed.

---

## Final product vision

Buddy should feel like an intelligent personal computer assistant, not a chatbot with a list of commands.

The user should be able to say:

"Make my project production-ready."

Buddy should understand the project, inspect it, identify the work required, plan the changes, execute safe actions, ask only when necessary, recover from failures, and verify the final result.

The user describes the outcome.

**Buddy figures out how to achieve it.**
---

## Status

This brief (v2) is the source of truth. Milestone numbers are the ones in section 12. Every entry below is backed by code and tests on the branch named.

| Milestone | Status | Where | Limits |
| --------- | ------ | ----- | ------ |
| M0 Fresh audit and baseline | Done | main | Baseline: `zorui` 938 passed, 10 ignored; `zorui-svc` 13 passed; release build 5m 00s; `zorui.exe` 8,103,424 bytes; 59 warnings. |
| M1 Verification and task state | Done | buddy-intel-m7-finish | Every side-effecting `Call` now reads its outcome back or is classed Unverified with a reason in code (`verifier::kind_of` has no wildcard arm, so a new variant does not compile until it is decided; `verifier::unverified_reason` holds the reasons). Read back: Volume (output device), Open (process list), Brightness (status after the set), Radio (state after the switch must match an asked-for on or off, else Failed), Clip (clipboard text read back, mismatch Failed, unreadable Not confirmed), Wallpaper (`SPI_GETDESKWALLPAPER` file name; a different name or no answer is Not confirmed, not Failed, because the theme tool may hand Windows a converted copy), Remember and Forget (row checked in buddy.db after the write), Play (media session, in `playsong`), RunCheck (exit code). Unverified with a reason: Search, Url, Media, OpenFile, Message, Power, TakeOver, and the browser tools other than navigate, snapshot and find. The read-back functions in `agent::readback` take read and sleep closures, so tests use fakes. Table test `no_side_effecting_call_completes_from_its_own_ok_message` runs every Call variant and each browser tool and checks none reaches TaskStatus::Completed from its own Ok. Limits: the new read-backs (clipboard, wallpaper, radio, memory rows) were tested with fakes only, not on the live desktop; the radio read-back trusts the state the toggles bridge reports after the switch; a Not confirmed read-back ends the task Unverified, never Completed. Tests: zorui 1317 passed, 1 failed, 11 ignored; zorui-svc 13 passed. The one failure is `tokens::tests::no_spacing_const_is_declared_outside_tokens`, caused by another person's uncommitted `SCROLL_GAP` in bar.rs. |
| M2 Centralized permissions | Done | buddy-intel-m7-finish | `policy::gate` is the one gate, and it returns a `policy::Cleared` permit that every executor requires: `agent::exec`, `browse::run`, `developer::run_check`, `assist::run_cleared` (search, open, play, message) and `drive::start_with_chat` (every take-over start). Skipping the gate does not compile. Rules: ReadOnly and Reversible pass, Consequential and Destructive ask, the assumed-intent refusal and the run_check once-per-workspace yes are kept, unknown tools fail closed (`parse_step` has no Call for them, an unknown browser tool is refused). Paths on the gate: the AI's tool calls (desktop, Discord and other chats all reach `run_task` and `run_exec`), the fixed phrases in `assist::run` (each acting `Intent` maps to its agent `Call` in `assist::call_of`; `Drive` goes through `agent::take_over_with`, and the take-over a message to an unknown contact starts goes through `policy::gate` too, so its island question is now asked before it returns and its reply says "Taking over now."), and the fixed-phrase take-over. Out of scope, with reasons: read-only phrases (`Intent::Screen` answers a question about the screen, `Intent::Chat` goes to the AI whose tool calls are gated one by one); delegation (a model call with no tools); `mcp::Server::call` (only `browse` uses it, behind its permit); the steps inside an approved take-over (`drive` Tool::Open and Tool::Launch run under the take-over's yes, and each risky click asks); and direct user input, which is the user's own action and not the buddy's: bar, island, desktop and sidebar buttons (media, volume, mute, brightness, power, Wi-Fi, wallpaper, clipboard copy), keymap shortcuts and Settings. No fixed phrase exists for volume, power, wallpaper, clipboard, radio or brightness; those only run through the AI and so through the gate. Tests in `agent::policy` and `assist`: unknown tool refused, Destructive and Consequential not cleared without a yes, a fixed-phrase take-over does not start on a no, a fixed phrase whose switch is off is refused before any question. Limits: the island question of a take-over for an unknown contact is not exercised in a test, because it needs the live island. |
| M3 Progressive context | Done | buddy-intel-m7-finish | Memory and turn history share one connection, opened once per process (`memory::with_app`). The `memories` table has two nullable columns, `kind` (preference, project or semantic) and `project` (the active project's folder name). `add_column` adds them on open when they are missing and leaves existing rows alone. A fact saved through `remember` gets its kind from `memory::kind_to_save` (plain facts are stored with no kind, and a project fact is found by its text when its window is not in front) and the project the window in front shows, when the fact has a project word or a word of the window title (`project::lookup`). `memory::for_context` reads a row by its kind: a preference when its words share something with the request, a project fact only for its project, a semantic fact never. A row saved before the columns has no kind and is read by the old text rules (a preference word, or the project folder name in the text). `for_context` and `recall` never return sensitive-looking text. The context list is capped at 6 items and 600 characters; `recall` returns up to 5 matching facts of any kind. Limits: the 13-fact fixture is hand-written, not real saved facts; the real buddy.db and a real window title were not checked live, best match first. Retrieval is checked on 13 realistic facts (inline fixture in `the_saved_facts_fixture_gives_the_expected_context_for_each_request`, plus one sensitive row stored directly) over eight requests. "It", "this", "that", "them", "those" and "the last one" are resolved against the newest turn (`intent::resolve_reference`), as a note; no model call. The "reference note fires on plain questions" limit was fixed in M4 and is covered by `intent_reference_table_passes_every_row` ("what time is it" is a reference row). Not checked live: the real buddy.db has no memory rows, and the project a remembered fact is saved against depends on the window title, which the tests do not set. Tests: zorui 1154 passed, 0 failed, 11 ignored (new in the gap fix: `a_project_fact_saved_with_no_project_in_front_is_retrieved_for_its_project`, `a_restated_project_fact_keeps_its_project_kind`, `a_restated_fact_with_a_new_explicit_kind_takes_it`, `a_sensitive_project_fact_is_refused_and_never_returned`, `project_facts_saved_with_no_window_still_cap_at_six_items`); zorui-svc 13 passed; release build 4m 58s, zorui.exe 8,608,768 bytes. Context sources (second pass, `agent/sources.rs`): `sources::select` is the one place that decides which sources a request gets, and a plain request gets none. The open file of the window in front is added when the request says this file, this document or the file, or points back with this or it: the name in the window title is resolved to a path under the active project or the user's folders, and only its name and path are sent, plus at most 500 characters from its start when the request also asks to read, summarize, explain or check it, read through `policy::gate`, with secrets redacted. Recent files come from the names and modified times of the .lnk files in the Windows Recent folder (a time word such as recent, last, earlier or yesterday together with a file word, or "was working on"), at most 5 files, 60 characters per name, 300 characters in all, with no file contents. The clipboard is read with `util::clipboard_get` only for this text, the selection, what I copied or the clipboard, capped at 400 characters. All sources together are capped at 1100 characters. Every source drops sensitive-looking text and secret file names, file names and paths need the Open switch, the clipboard needs the System switch, and each use is logged by kind only. The screen stays with `see_screen` and is not widened. Nothing is saved to buddy.db and nothing leaves the PC except inside the request to the chosen model. Tests: six in `sources::tests` (selection table of 20 requests, caps, sensitive drops, fake Recent folder, open file through the gate, switches off) and one render test in `agent.rs`. Limit: not checked against the real Recent folder or a real window title. |
| M4 Intent inference | Done | buddy-intel-m1b | Keyword rules in `agent/intent.rs`, no model call. Measured by the table in `agent/intent_table.rs`: 62 request rows (49 hand-written, 13 from logged requests) plus 16 reference checks. The 13 logged-request rows are from 11 distinct requests in buddy.db (21 `tasks` rows, 6 `turns` rows) and one `buddy request` line in `zorui.log.old`; they are live-check prompts on the developer PC, not everyday use. The log holds no `buddy asks one question` line, so no logged question was counted. Counts: before any rule change the table passed 57/62 (failing: "put the PC to sleep", "use the browser to go to example.com", "what files did I open recently", and "how many errors does this project have" with nothing in front and with the project in front). With the vague-object rule for "this project" in place, 60/62. After removing that rule, 62/62. Rule changes: "sleep" and "browser" are action words; a question that starts with what, how, which, who, where, when or why is an answer, even with an action word in it. "how many errors does this project have" is not vague: it proceeds as an answer with or without a project in front. No logged request needed a privacy rewrite: none has a name, address, number, message content or path. Keyword-era baseline (before the first M4 change): table 36/49, reference 8/16. Now: table 62/62, reference 16/16. A vague request resolves to the active project, the window in front, or the last request, and the note says which; with none of these it asks one question and does not act. "It" is a reference only as an object ("fix it", "turn it off"), not in questions or statements about now. An assumed request has its consequential and destructive steps refused in `agent::approve` (`intent::ASSUMED_REFUSAL`), not asked. Confidence (M4 step 5, `intent::Confidence`, no model call): High for a clear action word and object or a question form; Medium for a vague request or follow-up that has an object (the project in front, the window in front, or an earlier turn); Low for a vague request with nothing to point at, or a project request with no project in front. The level and the decision are logged with the request (`buddy intent: Kind Confidence proceed|assume|investigate|ask`), never the words. Decision (step 6): High proceeds, Medium assumes or investigates (see M6), Low asks one question and runs nothing. The table now has 64 rows (two new rows with a window and an earlier turn together) and each row is checked for its confidence and decision: 64/64 pass, reference 16/16; 35 rows proceed, 13 assume, 2 investigate, 14 ask, and no clear row investigates or asks. Limits: the logged requests are developer live-check prompts, 11 distinct, from one PC, so most of the table is still hand-written. Kind is only logged, so Act/Answer rows check a label, not behaviour. No live check. Release build after the merge of buddy-intel-m7-finish d6e1141: 3m 12s, `zorui.exe` 8,608,768 bytes. Tests: `zorui` 1138 passed, 0 failed, 11 ignored (includes `no_vague_row_of_the_table_proceeds_or_selects_a_consequential_or_destructive_action`, `a_vague_request_never_selects_a_consequential_or_destructive_action`, `vague_request_with_no_context_asks_one_question_and_does_not_act`, `intent_table_passes_every_row`, `intent_reference_table_passes_every_row`); `zorui-svc` 13 passed. |
| M5 Structured planning and recovery | Done | buddy-intel-m5 | All five parts are built and have unit tests. The wiring in `run_task` (the plan call, the re-plan and the resume thread) is now tested end to end with a fake model (`agent/e2e.rs`, seams `agent::chat` and `agent::run_exec` under `#[cfg(test)]`). Seven tests cover: one planning call before the first tool step, with the plan saved on the row; no planning call for a plain request; a planned read runs and a change against it is refused, with the refusal in the model's next turn; one re-plan on a failed step, recorded as a note, and a second failure ends the task without another re-plan; an interrupted planned task resumes on its own row once when its next step reads, and waits with the verdict when it changes something, running nothing; a repeated side-effect step from an interrupted run is blocked. Plan: a multi-step request (`routing::looks_multi_step`) gets one planning call before its first tool step (`plan::plan_if_wanted`). The reply is `{objective, done_when, steps:[{does, expect, side_effect}]}`, at most 8 steps, stored on `tasks.plan` (nullable JSON). Code fences are skipped; garbage or no steps means no plan, recorded as a `note:plan` step. Other requests make no planning call. Each tool call is matched to the next unfinished step: a change against a read-only step is refused with a reason the model sees. A failed tool step, or a step refused twice, re-plans once; a failure after that ends the task (Failed, or Partial if an action ran). Restart: the newest Interrupted task with a plan, not a private request and not resumed before (`tasks.resumes`), resumes on its own row (`agent::resume_on_start`). It resumes automatically when its next step only reads. When the next step changes something, it stays Interrupted with the verdict "waiting: the next step changes something; ask the user", and a resumed run refuses any change. Old rows with no plan resume as before (no candidate). Live check on 2026-11-14 (model claude-cli:haiku, run as `#[ignore]` tests `live_*` in `agent/e2e.rs` that call `run_task` and `resume_pending` with the real model and real read tools; the app was not driven, and the tests use the test database, not the real buddy.db, which was backed up and found unchanged). Planning passed: "First look up what example.com is, then tell me in one sentence what it is for" made one planning call first (model calls: plan, turn, turn), saved the plan, ran ReadUrl and ended Completed. Re-plan passed: two unreadable URLs on invalid hosts gave one `note:replanned` step after the first failure, and the second failure ended the task Failed with "I couldn't finish that as planned, so I stopped." and no third planning call (plan, turn, plan, turn). A find_file for a missing name returns Ok with a "no match" text, so it is not a failed step. Resume of a read passed: a seeded Interrupted task with a plan, first step done and a read next resumed once on its own row (resumes 1, no new row), ran the read and ended Completed. Resume of a change passed: the same seed with a clipboard step next stayed Interrupted with the verdict "waiting: the next step changes something; ask the user", resumes 0, and made no model call and no tool call. Limit: the interrupted rows were seeded in code, not made by killing a running app, so the kill and restart path of `resume_on_start` was not seen live. No bug was found, and production code is unchanged. Tests: `zorui` 1320 passed, 0 failed, 16 ignored (5 are the live checks); `zorui-svc` 13 passed; release build 4m 53s. Earlier live check (release build, real `buddy.db` backed up and restored): an interrupted task was marked Interrupted at start, and a repeated clipboard step was refused before it ran. |
| M6 Adaptive autonomy | Done | buddy-intel-m6 | Clear requests proceed. Consequential actions ask (M2), destructive ones ask with their own question. A vague request with one object (the project in front, or one window, or the last request) is assumed and the assumption is stated in the reply. A vague request with two candidate objects (a window in front and an earlier turn) investigates first: `intent::investigate` makes at most 2 reads (`MAX_READS`), each a ReadOnly call (`policy::risk_of`) run through `run_exec` and so `policy::gate`. Read one is `find_file` for the file named in the window title. If that finds nothing, read two is `session_search` for that file name, or for the app name when the title has no file, to see whether the window was discussed. Evidence for the window makes it the object. With none, the last request is. With none and no last request, the window, so the window-first order holds when the evidence is equal. It never asks, because the window wins on any evidence and the last request wins otherwise; an unrelated last request no longer makes it ask. Low confidence asks one concise question and runs no tool and no model call. An assumed request has its consequential and destructive steps refused, not asked (`intent::ASSUMED_REFUSAL`, unchanged). Clear requests get no extra read or model call (table check). A failed tool's error gets one hint for the model (`agent/diagnose.rs`), which the user never sees. Tests: table checks, `intent::investigate` unit tests, and four end-to-end tests in `agent/e2e.rs` with a fake tool that fails on any non-ReadOnly call (read-only only, cap held, Low asks with no tool, assumed request refused for risky steps), and unit tests for an existing document (window assumed), a discussed window, an unknown and undiscussed window (last request assumed) and the app-name read. Limits: "holds" means found, not meant, and the ranking is fixed, not learned; no live check; the hints match the tools' current wording; the question rate is read from the log only. |
| M7 Provider abstraction and routing | Done | buddy-intel-m7 | Normalized facts in `agent/provider.rs`: capabilities per provider (native tools, vision), usability (a keyed provider needs its saved key), and the token usage each reply reports (from the adapters' stream events), summed per task and saved on its row (`tasks.tokens_in`, `tasks.tokens_out`). Routing never picks a strong or fallback model whose key is missing; the user's own model is never skipped. Streaming, cancellation and latency already existed (the bridge's event stream, `stop_check`, `state::Timing`). Each provider's wire format is an `Adapter` (`bridge/web/adapters.rs`), chosen by `for_provider`; the chat code never names a provider. Native tools (the tool list and reading tool calls) are part of the Anthropic adapter, not named in `ai_chat`. Screen questions now pick the first configured model that can be sent pictures (`assist::screen_model`), so a picked subscription model is passed over. Model ids saved before the Gemini CLI rename map to their new ids when the chat reads them (`sub_cli::current_id`); config.json is not rewritten. Context windows (`agent/provider.rs` `context_tokens`, in this order): the user's `ai.customModels` `contextTokens`; then the published window of the default models: claude-sonnet-5-5 and claude-haiku-5-5 at 1M (https://platform.claude.com/docs/en/models/overview, checked 2026-11-12), gemini-2.5-flash and gemini-3-flash-preview at 1,048,576 input tokens (https://aistudio.google.com/docs/models/gemini-2.5-flash and .../gemini-3-flash-preview, checked 2026-11-12), gpt-4o-mini at 128,000 (https://developers.openai.com/api/docs/models/gpt-4o-mini, checked 2026-11-12); then Ollama's `/api/show` (asked once per model per session); else `UNKNOWN_CONTEXT_TOKENS` = 32,000 as a floor. Left out because no number was confirmed: mistral-medium-latest (the Mistral models page has no row for it), the subscription CLIs, openrouter/auto. Each request is fitted to its window less a 4,096-token reply reserve (`fit_to_budget`: cut old tool results, then drop the oldest turns; the request's own message is never cut). Each tool result is capped at 4,000 characters. Native tools run for the model the chain picks when its provider reports native tools (only Anthropic) and `ai.nativeTools` is on; the user's own model is never skipped. Providers that report no counts show zero, not an estimate. Routing inputs, all lightweight and with no extra model call (`agent/routing.rs` `decide`, facts from `agent/provider.rs`): complexity (multi-step or coding words, `looks_coding`) sends a request to the strong model; a simple request goes to another model in the user's chain only when that model has a known price strictly below the user's model, or the same price and a latency at least 20 percent lower; an unknown price never wins and, when the user's own model has no known price, cost routing is off; vision picks the user's model if it can see, else the first chain model that can (`assist::screen_model` logs it); tool calling needs no filter because every model speaks the JSON tool protocol, and native tools are used for the chosen model when its provider has them; context size leaves out a model whose known window is smaller than the request; availability skips a model with no saved key. Latency and failures: each model call records its time to first text and total time (`assist::stream_turn_live`), kept as a smoothed figure (weight 0.3) per model id in the `model_health` table of buddy.db. Three failures in a row rest a model for 120 seconds, then it is tried again; a resting strong model, cheaper pick or fallback is passed over, and a resting user model gives way to another usable model but stays in the chain after it. The user's own model is never removed: when a strong or cheaper pick goes first, the user's model is tried last, and a model with no saved key stays first as before. `ai.routeByCost` (config.json only) is off by default, so the user's own model answers every request it can and the fallback order stays as set; turned on, simple requests go to a strictly cheaper priced model and fallbacks are tried fastest first when every one has a figure. Cooldown, vision and the strong model for complex requests work either way. Each route is logged as `buddy route: <model> (<simple|complex|vision|cooldown|latency>)` with no request text. Prices per million tokens, input and output, text, standard tier, checked 2026-11-15: claude-sonnet-5-5 $2 and $10, claude-haiku-5-5 $0.10 and $0.50 for prompts up to 100,000 tokens (https://platform.claude.com/docs/en/about-claude/pricing); gemini-2.5-flash $0.30 and $2.50, gemini-3-flash-preview $0.50 and $3.00 (https://aistudio.google.com/docs/pricing); gpt-4o-mini $0.15 and $0.60 (https://developers.openai.com/api/docs/models/gpt-4o-mini). No price is listed for mistral-medium-latest (the Mistral pricing page has no row for it), openrouter/auto or the subscription CLIs, so they are unknown. Limits: the figures come from real calls only, so they were checked in unit tests and not against a live provider; time to first text is empty for a reply that streams only tool calls, in which case the whole-reply time is used. Tests: zorui 1349 passed, 0 failed, 17 ignored; zorui-svc 13 passed. The Settings dropdown may still show an old id until it is picked again. The zero-token request in the live check was the Claude subscription path, which dropped the counts in its final result line. Fixed: the result line's counts are read, and cached prompt tokens are counted as input on both the subscription and the API paths. The fix is covered by a unit test on the real result shape, not by a second live run. The Gemini CLI was discontinued; its subscription option now uses Antigravity's `agy` in print mode (`--output-format json`, `--mode plan`), which prints one JSON reply with `usage`. The error reply was captured on a quota-exhausted account; a success reply has not been captured, so its shape and cache-token meaning are unverified. Codex runs with `--json`: the agent's message is read from `item.completed` and the token counts from `turn.completed`, checked against captured lines from a real one-word run. Codex error events are read too (`error` with `message`, and `turn.failed` with `error.message`): a live `codex exec ... -m no-such-model -` run on 2026-11-12 printed them, and the chat shows Codex's own message instead of "stopped without an answer". Not verified live: the Ollama `/api/show` path (no Ollama server on the test PC; only its parsing is tested), and the agy success shape: the live `agy -p "Reply with the word ok"` call on 2026-11-12 was quota-blocked, so the success fixture is still assumed. `chat_agy` now sends the 12-turn `agy_prompt` (it sent the whole conversation before). |
| M8 Honcho integration | Not started | — | Only a `Recall` trait for a future backend. |
| M9 Developer skill | Done (M9a, M9b and M9c) | buddy-intel-m9 | M9c: `edit_file` replaces one exact text span in one file inside the developer workspace. Paths resolve like M9a (canonical, junctions and symlinks cannot leave the workspace, secret files refused); the old text must match exactly once, else the model is told why; the file is written through a temp file and a rename; files over 1 MB and tasks past 20 edits are refused. The first edit of a file in a task copies the original to `%LOCALAPPDATA%zor-uidev-backups<task id><relative path>`. It goes through `policy::gate` and takes the `policy::Cleared` permit like `run_check`: Consequential, asked once per task naming the workspace and the first file, the yes held for that task only, a no is Declined and writes nothing, and a refused path or text asks nothing. The verifier has an Edit kind: a task that edited a file is Completed only if a `run_check` passed after the last edit, so edit with no check, or pass then edit, ends Unverified and the answer says so, and edit then fail is not Completed. The reply gives the model a bounded, redacted diff. Settings has a Developer workspace field and a Create button that calls `create_test_workspace` and fills the field. `edit_file` is in the Developer skill with eval rows, the native tool list and the JSON protocol. Not built: a list of every file shown before the first edit (the question names the first file, and the yes covers later files in the task). M9b: `run_check` runs one of five fixed entries (`cargo check`, `cargo test`, `cargo build`, `cargo build --release`, `cargo clippy`) with no shell and an argv from code only. The first run in a workspace asks and names the command; a yes holds for the app session only (memory, never saved), a no is Declined. Limits: 10-minute wall clock (on timeout the whole process tree is killed with `taskkill /PID <pid> /T /F`, then `child.kill()`), output keeps its last 16 KB, redacted first, stdin closed, cwd is the canonical workspace. Verifier: a passing check is evidence; the latest check decides (a later pass clears an earlier failure; a failure after a pass is not Completed); a code claim with no check run is Unverified and the answer says "I did not run a check, so this is unverified." Workspace setting that is a junction or symlink is refused with a reason. Live timeout check (M9b review): a dependency-free temp project with 4000 generated functions, `cargo build` with a 1-second limit through `run_program`: it timed out and no `rustc.exe` it started was still running afterwards (observed, passed). A hello-world build finishes in under 1 s, so it does not reach the limit. The same live run with `taskkill` disabled also left no rustc (cargo's own kill was enough here); the `cmd` to `ping` grandchild test does fail without `taskkill` and passes with it. The test folder `%LOCALAPPDATA%\zor-ui\dev-workspace` is made by `create_test_workspace` when called (no caller yet). Tests: `zorui` 1080 passed, 0 failed, 11 ignored (the 11th is the live cargo check, run with `--ignored`); `zorui-svc` 13 passed; one live `cargo check` and `cargo test` run in the test folder through `run_program` (not through the app). M9a: four read-only tools inside one workspace folder (`buddies.developerWorkspace`, set by hand in config until Settings has a field): `list_tree`, `read_file`, `search_code`, `git_status`. Paths are resolved before use, so junctions cannot reach outside the workspace; secret files are never read; key-like text is redacted. Scope: `docs/M9-developer-scope.md`. |
| M10 Skills, delegation, evaluation | Partial | m10-skills | Seven skills in `agent/skills.rs`, each with version, capability, tools, permission, context and an eval. New: Developer (`list_tree`, `read_file`, `search_code`, `git_status`, and `run_check`, Consequential, asked once per workspace), Project (`list_tree`, `remember`, `forget`), Browser research (`browser_*`, `web_search`, `read_url`) and Workflow (guidance over existing tools). Tests check every skill tool against the native tools, the JSON protocol and `browse::TOOLS`, check each permission against `policy::risk_of`, and run an eval table per skill. Added in the finish: (1) Routing is measured on `native/zor-ui/eval/skill-routing.json`, 41 rows: 27 real (the distinct requests of the 39 `tasks` rows and 14 `turns` rows in buddy.db, rewritten to carry nothing personal; the real ones had none, and the `tasks` row that carried a retry note was cut to the request) and 14 hand-written (from the intent table). The logs `zorui.log` and `zorui.log.old` no longer hold any `buddy request` line, so none came from there. Each row names the skill it should select. Before the routing fixes: research 100% precision and 50% recall, desktop 88% and 80%, personal 75% and 100%, developer 50% and 50%, project 0% and 100% (no true row), browser_research 100% and 100%, workflow no rows. After: research 100% and 50%, desktop 90% and 90%, personal 100% and 100%, developer 100% and 75%, project 0% (two false picks, no true row), browser_research 100% and 100%. The test `routing_precision_and_recall_hold_on_the_measured_set` prints the table and fails if a skill drops under those numbers; no model call. (2) Routing fixes, each from a real row that failed before: "Take over" now selects desktop; "claude code" and "vs code" are taken out before matching, so a Claude Code sessions question no longer selects developer; "errors" selects developer, so "how many errors does this project have" does; "tell me" no longer selects personal. Versions: desktop and developer 2. (3) Each of research, developer, project and browser_research declares `steps`, a short ordered list of its tools, added to the prompt as one line ("Usual order: ..."); a test checks every step is one of the skill's tools. Versions of those four: 2. (4) Delegation: summary-only delegation (`agent/delegate.rs`, a856e14) is enough for now. The logged tasks used at most 3 tool calls (mean 1.0) and none was planned, so no measured case needs a subagent, and none was added. Delegation runs only on a tool result over 6000 characters and never on exact-text results; a test (`simple_requests_never_make_a_summary_call`) shows a chat reply and a short tool result make no summary call. Still left: project routing, since "project" alone is too loose a word (two false picks: "how many errors does this project have" and "open the project in VS Code"); "what files did I open recently" still selects desktop through "open"; "What's the current state of zor ui" and a garbled voice request ("Nu V N U V I O") are still missed; research recall stays at 50% because "what's the weather in Oslo" is not routed; the 27 real rows are developer live-check prompts from one PC, not everyday use. No live check. Tests: zorui 1304 passed, 0 failed, 11 ignored; zorui-svc 13 passed; release build 8m 09s, `zorui.exe` 9,033,728 bytes. |
| M11 Performance | Not started | — | Only the M0 numbers. No optimization and no before-and-after measurements. |

Tests on `main` at `741abef` (M9a): `zorui` 1011 passed, 0 failed, 10 ignored; `zorui-svc` 13 passed. Warnings: the existing unused `run_loop` in `agent.rs`, and no others from the M-series files.
The work of every milestone listed above is on `main`. `buddy-intel-m1b`, `buddy-intel-m5` and `buddy-intel-m6` are ancestors of it. `buddy-intel-m7` and `buddy-intel-m9` hold the same changes under different commit hashes, from a parallel history. The branch names stay as a record of where each milestone was built.
M5 migration was run on a copy of a real `buddy.db` (21 tasks): all rows kept, the column added, a new row written.
Corrections: the active-project source (`agent/project.rs`) is part of the M3 commit `8c4a892`. An earlier note said there was none.

The earlier "Buddy Intelligence Upgrade" plan (milestones 1–5) is no longer in this file. It is still in the history of `main`.
