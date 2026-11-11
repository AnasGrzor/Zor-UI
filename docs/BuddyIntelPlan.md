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
| M1 Verification and task state | Done | buddy-intel-m1b | Volume and opening apps have no read-back. The repeat block covers only the same call with the same arguments. |
| M2 Centralized permissions | Done | buddy-intel-m1b | The browser's per-action gate still lives in `browse::gate`. |
| M3 Progressive context | Done | buddy-intel-m1b | "It" is not a reference word. Memory lookup opens the database on every request. Intent rules are keyword-based. No preferences or project memories in context. |
| M4 Intent inference | Done | buddy-intel-m1b | Keyword-based. Not yet measured against real requests. |
| M5 Structured planning and recovery | Done | buddy-intel-m5 | Steps are recorded from what the model did, not from a declared plan. Interrupted tasks are marked and their side effects blocked, but not resumed. Recovery runs once per app start. Live check (release build, real `buddy.db` backed up and restored): an interrupted task was marked Interrupted at start, and a repeated clipboard step was refused before it ran. |
| M6 Adaptive autonomy | Done | buddy-intel-m6 | Clear requests proceed. Consequential actions ask (M2). Unclear requests take the first object found: project, then window in front, then last request; the readings are ranked in that fixed order. Asks only when nothing is found. Every question is logged with its request, and a test table covers phrasings that must not ask. A failed tool's error gets one hint for the model (`agent/diagnose.rs`), which the user never sees. Limits: the ranking is fixed, not learned; the hints match the tools' current wording; the question rate is read from the log only. |
| M7 Provider abstraction and routing | Partial | buddy-intel-m7 | Normalized facts in `agent/provider.rs`: capabilities per provider (native tools, vision), usability (a keyed provider needs its saved key), and the token usage each reply reports (from the adapters' stream events), summed per task and saved on its row (`tasks.tokens_in`, `tasks.tokens_out`). Routing never picks a strong or fallback model whose key is missing; the user's own model is never skipped. Streaming, cancellation and latency already existed (the bridge's event stream, `stop_check`, `state::Timing`). Each provider's wire format is an `Adapter` (`bridge/web/adapters.rs`), chosen by `for_provider`; the chat code never names a provider. Native tools (the tool list and reading tool calls) are part of the Anthropic adapter, not named in `ai_chat`. Screen questions now pick the first configured model that can be sent pictures (`assist::screen_model`), so a picked subscription model is passed over. Model ids saved before the Gemini CLI rename map to their new ids when the chat reads them (`sub_cli::current_id`); config.json is not rewritten. Limits: no context-size limits (no checked per-model numbers in the code); native tools are not chosen by capability (only Anthropic has them, and routing does not yet pick it for a request); providers that report no counts show zero, not an estimate. The Settings dropdown may still show an old id until it is picked again. The zero-token request in the live check was the Claude subscription path, which dropped the counts in its final result line. Fixed: the result line's counts are read, and cached prompt tokens are counted as input on both the subscription and the API paths. The fix is covered by a unit test on the real result shape, not by a second live run. The Gemini CLI was discontinued; its subscription option now uses Antigravity's `agy` in print mode (`--output-format json`, `--mode plan`), which prints one JSON reply with `usage`. The error reply was captured on a quota-exhausted account; a success reply has not been captured, so its shape and cache-token meaning are unverified. Codex runs with `--json`: the agent's message is read from `item.completed` and the token counts from `turn.completed`, checked against captured lines from a real one-word run. Codex's error events are not captured yet, so an error event shows as "stopped without an answer". |
| M8 Honcho integration | Not started | — | Only a `Recall` trait for a future backend. |
| M9 Developer skill | Partial (M9a read-only) | buddy-intel-m9 | Four read-only tools inside one workspace folder (`buddies.developerWorkspace`, set by hand in config until Settings has a field): `list_tree`, `read_file`, `search_code`, `git_status`. Paths are resolved before use, so junctions cannot reach outside the workspace; secret files are never read; key-like text is redacted. Not yet: Settings field, edits, checks and their approvals (M9b, M9c). Scope: `docs/M9-developer-scope.md`. |
| M10 Skills, delegation, evaluation | Partial | — | Research, Desktop and Personal skills exist. Missing: Developer, Project management, Browser research, Workflow skills; versioning and evaluation criteria. |
| M11 Performance | Not started | — | Only the M0 numbers. No optimization and no before-and-after measurements. |

Tests on `main` at `741abef` (M9a): `zorui` 1011 passed, 0 failed, 10 ignored; `zorui-svc` 13 passed. Warnings: the existing unused `run_loop` in `agent.rs`, and no others from the M-series files.
The work of every milestone listed above is on `main`. `buddy-intel-m1b`, `buddy-intel-m5` and `buddy-intel-m6` are ancestors of it. `buddy-intel-m7` and `buddy-intel-m9` hold the same changes under different commit hashes, from a parallel history. The branch names stay as a record of where each milestone was built.
M5 migration was run on a copy of a real `buddy.db` (21 tasks): all rows kept, the column added, a new row written.
Corrections: the active-project source (`agent/project.rs`) is part of the M3 commit `8c4a892`. An earlier note said there was none.

The earlier "Buddy Intelligence Upgrade" plan (milestones 1–5) is no longer in this file. It is still in the history of `main`.
