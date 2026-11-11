# Zor-UI — Buddy Intelligence Upgrade

## Master Engineering Implementation Plan

### 1. Project Objective

Upgrade Buddy, the AI assistant inside Zor-UI, into a highly capable, context-aware, persistent, semi-autonomous assistant.

The objective is not simply to give Buddy a better system prompt or connect a more powerful model. The objective is to build a proper intelligence architecture that enables Buddy to:

- Understand complex, multi-part instructions.
- Break goals into executable steps.
- Choose the right model for each task.
- Use existing application tools intelligently.
- Execute multi-step workflows reliably.
- Remember useful information between conversations.
- Understand relevant desktop and application context.
- Recover from errors and revise its approach.
- Verify whether tasks were actually completed.
- Operate autonomously within user-defined safety boundaries.
- Support different AI providers without coupling the application to one provider.

**Important:** Preserve Zor-UI's existing functionality. Upgrade its architecture rather than rebuilding the application from scratch.

---

# 2. Existing Architecture: Audit Before Modifying

Before writing code, inspect the complete repository.

The main project is located in:

`native/zor-ui`

Pay particular attention to:


| File                                       | Responsibility                                                       |
| ------------------------------------------ | -------------------------------------------------------------------- |
| `src/[agent.rs](http://agent.rs)`          | Buddy's agent loop, prompts, tool execution and conversation history |
| `src/[assist.rs](http://assist.rs)`        | Voice/text intent recognition and AI routing                         |
| `src/bridge/[web.rs](http://web.rs)`       | AI provider communication and streaming                              |
| `src/[conf.rs](http://conf.rs)`            | Configuration and application defaults                               |
| `src/[browse.rs](http://browse.rs)`        | Browser automation                                                   |
| `src/[drive.rs](http://drive.rs)`          | Desktop automation and UI interaction                                |
| `src/[mcp.rs](http://mcp.rs)`              | MCP integration                                                      |
| `src/[proc.rs](http://proc.rs)`            | Process and application integration                                  |
| `src/[webread.rs](http://webread.rs)`      | Web search and page reading                                          |
| `src/[media.rs](http://media.rs)`          | Media controls                                                       |
| `src/[screen.rs](http://screen.rs)`        | Screen-related functionality                                         |
| `src/settings/[pages.rs](http://pages.rs)` | Settings interface                                                   |
| `src/[main.rs](http://main.rs)`            | Application module declarations                                      |


Existing functionality must be reused wherever possible.

Do not replace working integrations with duplicate implementations.

Before modifying anything:

1. Map the existing architecture.
2. Identify all existing AI providers and their capabilities.
3. Document current tool interfaces and safety checks.
4. Identify all existing voice, browser and desktop workflows.
5. Check the Rust toolchain, dependency versions and build configuration.
6. Establish a clean baseline build and test result.

Create an implementation checklist and update it as work progresses.

---

# 3. Core Architecture

Build the intelligence system around six independent components:

1. Intelligence Core
2. Automatic Model Router
3. Agent Planner and Executor
4. Persistent Memory
5. Tool and Skill Registry
6. Safety and Permission Engine

The architecture should look approximately like this:

```
                  USER
                   |
                   v
             BUDDY INTERFACE
                   |
                   v
              AGENT CORE
                   |
          +--------+--------+
          |                 |
          v                 v
      CONTEXT            MEMORY
      ENGINE             ENGINE
          |                 |
          +--------+--------+
                   |
                   v
              TASK PLANNER
                   |
                   v
              MODEL ROUTER
                   |
          +--------+--------+
          |        |        |
          v        v        v
        Gemini  Claude   OpenAI
          |        |        |
          +--------+--------+
                   |
                   v
             TOOL REGISTRY
                   |
                   v
             POLICY ENGINE
                   |
                   v
          EXECUTION ENGINE
                   |
          +--------+--------+
          |        |        |
          v        v        v
        Browser  Desktop   App Tools
          |        |        |
          +--------+--------+
                   |
                   v
               VERIFIER
                   |
                   v
              FINAL RESULT
```

Every component should have a clearly defined interface.

Avoid tightly coupling the agent to individual providers, tools or storage implementations.

---

# 4. Phase One — Fix and Upgrade the Existing Agent

**Priority: P0**

Start by improving the current [`agent.rs`](http://agent.rs) implementation.

### Problems to address

- The current agent has a fixed step limit.
- Its execution flow relies heavily on a model-provided `more` boolean.
- Conversation history is only held in memory.
- The system prompt is hardcoded.
- Existing custom system prompt settings are not properly integrated into Buddy.
- The configured AI temperature is not consistently respected.
- Tool execution and completion decisions are too tightly coupled.

### Implementation

Refactor [`agent.rs`](http://agent.rs) into a facade over smaller agent modules.

Create:

```
src/
  agent.rs
  agent/
    planner.rs
    executor.rs
    state.rs
    verifier.rs
    policy.rs
    context.rs
```

Keep existing public interfaces working wherever possible.

### Agent execution states

Implement explicit task states:

```
enum TaskStatus {
    Planning,
    Running,
    WaitingApproval,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}
```

The model must not decide that a task is finished merely by returning a completion flag.

Completion must depend on execution state and verification evidence.

### Execution loop

Implement this lifecycle:

1. Receive user request.
2. Understand the intended outcome.
3. Retrieve relevant context and memories.
4. Decide whether planning is necessary.
5. Create and persist a task.
6. Execute the next appropriate step.
7. Check permissions.
8. Execute the tool.
9. Record the result.
10. Evaluate whether the task remains achievable.
11. Retry or replan if necessary.
12. Verify the requested outcome.
13. Mark the task complete only when supported by evidence.
14. Explain the result to the user.

### Preserve the existing fast path

Keep `assist::parse()` for straightforward deterministic actions.

Do not send every basic command through an expensive multi-step planning loop.

Examples:

- "Open Chrome" → existing direct action.
- "Turn up the volume" → existing direct action.
- "Research these three technologies and compare them" → agent planning.
- "Find the bug in this project, fix it and run the tests" → multi-step agent workflow.

### Prompt construction

Implement:

```
pub fn build_system_prompt(cfg: &Config) -> String
```

It should combine:

- Base Buddy instructions.
- User-configured system prompt.
- Current task information.
- Relevant retrieved memories.
- Relevant desktop context.
- Available tool descriptions.
- Current autonomy and safety settings.

Use the configured `ai.temperature`.

---

# 5. Phase Two — Provider-Agnostic Intelligence Layer

**Priority: P0**

Buddy must not depend on Gemini, OpenAI, Claude or any specific model provider.

Create:

```
src/
  intelligence.rs
  intelligence/
    provider.rs
    router.rs
    request.rs
    response.rs
    capabilities.rs
```

### Unified provider interface

Design a normalized interface similar to:

```
pub trait ModelProvider: Send + Sync {
    fn name(&self) -> &'static str;

    fn capabilities(&self, model: &str)
        -> ModelCapabilities;

    fn generate(
        &self,
        request: ModelRequest
    ) -> Result<ModelResponse, ModelError>;
}
```

Adapt this interface to the existing application's threading and streaming architecture. Do not introduce an unnecessary async runtime.

### Normalized request

```
pub struct ModelRequest {
    pub system: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub temperature: f32,
    pub max_tokens: Option<u32>,
}
```

### Normalized response

```
pub enum ModelOutput {
    Text(String),
    ToolCalls(Vec<ToolCall>),
    Mixed {
        text: String,
        calls: Vec<ToolCall>,
    },
}
```

Use a normalized event representation for streaming as well.

### Model capabilities

Each model should advertise capabilities such as:

- Text generation.
- Vision.
- Native tool calling.
- Structured output.
- Reasoning.
- Streaming.
- Context window.
- Relative cost.
- Expected latency.

Do not assume every model supports every capability.

### Automatic model routing

Implement an intelligent router.

It should consider the task rather than blindly using the configured default model.

Examples:


| Task                     | Preferred capability         |
| ------------------------ | ---------------------------- |
| Simple conversation      | Fast, inexpensive model      |
| Complex reasoning        | Strong reasoning model       |
| Coding                   | Strong coding model          |
| Screenshot understanding | Vision-capable model         |
| Long document analysis   | Large context window         |
| Tool-heavy workflow      | Reliable native tool calling |
| Local/private task       | Available local model        |


The router must:

1. Discover available configured providers and models.
2. Filter out models that cannot perform the required task.
3. Rank compatible models.
4. Select the best available option.
5. Fall back if the provider fails.
6. Record routing decisions for debugging.

Allow the user to override automatic routing and select a preferred model.

### Provider compatibility

Extend `bridge/[web.rs](http://web.rs)` rather than replacing it.

Normalize provider-specific tool calls into one internal format.

Support the existing Gemini, Anthropic, OpenAI-compatible, Mistral, OpenRouter and Ollama integrations where their capabilities permit.

Keep CLI integrations compatible even if they initially use a validated JSON-based tool protocol rather than native tool calling.

Never pretend that a model supports native tools if it does not.

---

# 6. Phase Three — Persistent Memory

**Priority: P0**

Buddy must retain useful information between conversations and application restarts.

Implement local persistent storage.

Suggested location:

`%LOCALAPPDATA%/zor-ui/buddy.db`

Use SQLite.

Suggested Rust dependency:

```
rusqlite = { version = "0.38", features = ["bundled"] }
```

Verify compatibility with the repository's Rust toolchain and lockfile before adding it.

### Database structure

Create tables for:

```
conversations
messages
memories
memory_links
tasks
task_steps
```

### Memory categories

Implement:

- User preferences.
- Personal facts.
- Projects.
- Decisions.
- Important conversation events.
- Procedures and recurring workflows.

Each memory should contain:

- Unique ID.
- Category.
- Content.
- Project association, if applicable.
- Source.
- Confidence.
- Importance.
- Creation timestamp.
- Last updated timestamp.
- Last accessed timestamp.
- Optional expiration timestamp.
- Archived status.

### Memory behavior

Buddy should:

1. Identify potentially useful information.
2. Decide whether it is worth remembering.
3. Avoid storing every conversation.
4. Detect duplicate memories.
5. Update existing memories when information changes.
6. Retrieve relevant memories before answering.
7. Associate memories with ongoing projects.
8. Avoid injecting irrelevant memories into prompts.

Start with SQLite full-text search.

Design a retrieval interface that can later support vector embeddings without rewriting the memory system.

### Privacy

Memory must be local by default.

Only relevant memory should be included in requests to external model providers.

Add settings to:

- Enable or disable memory.
- View memories.
- Search memories.
- Edit memories.
- Delete individual memories.
- Clear conversation history.
- Exclude selected projects or applications from memory.
- Disable automatic memory extraction.

Do not silently store sensitive information.

Persist task state and conversation history so Buddy can recover after restarting Zor-UI.

---

# 7. Phase Four — Planner, Executor and Verification

**Priority: P1**

Buddy should operate on objectives rather than isolated tool calls.

### Planner

The planner should convert a user request into a structured plan.

Each step should include:

- Step ID.
- Description.
- Required tool or capability.
- Expected output.
- Dependencies.
- Success criteria.
- Approval requirements.
- Execution status.

### Executor

The executor should:

- Execute steps in dependency order.
- Persist progress.
- Capture tool results.
- Handle errors.
- Retry recoverable failures.
- Replan when assumptions change.
- Support cancellation.
- Resume interrupted tasks.

### Verifier

Implement independent verification.

Examples:


| User request        | Verification                                        |
| ------------------- | --------------------------------------------------- |
| Create a file       | Check that the file exists and has expected content |
| Modify code         | Inspect the diff                                    |
| Run tests           | Check the actual test exit status                   |
| Open an application | Confirm the application is running                  |
| Search the web      | Confirm relevant sources were retrieved             |
| Complete a workflow | Evaluate every required success criterion           |


A successful tool response does not automatically mean the user's overall task is complete.

The verifier should distinguish between:

- Completed.
- Partially completed.
- Failed.
- Unable to verify.

### Execution budgets

Retain sensible limits.

Initial defaults:

- 12 steps for ordinary agent tasks.
- 90 seconds for ordinary execution.
- Existing browser-specific limits remain intact.
- Bounded retries.
- Maximum context and token budgets.
- User cancellation support.

Do not allow the model to increase its own execution budget.

Make limits configurable later.

---

# 8. Phase Five — Balanced Autonomy and Safety

**Priority: P1**

Buddy should be capable without becoming reckless.

Implement centralized permission handling.

### Risk categories

```
enum RiskLevel {
    ReadOnly,
    Reversible,
    Consequential,
    Destructive,
}
```

### Permission behavior


| Action                       | Default behavior                |
| ---------------------------- | ------------------------------- |
| Search the web               | Automatic                       |
| Read a webpage               | Automatic                       |
| Inspect files                | Automatic                       |
| Read project source          | Automatic                       |
| Open an application          | Automatic                       |
| Change application settings  | Depends on setting              |
| Create or edit project files | Allowed within configured scope |
| Send a message               | Ask first                       |
| Delete important files       | Ask first                       |
| Make purchases               | Ask first                       |
| Shut down the computer       | Ask first                       |
| Modify security settings     | Ask first                       |


Preserve existing safety gates in:

- [`assist.rs`](http://assist.rs)
- [`browse.rs`](http://browse.rs)
- [`drive.rs`](http://drive.rs)

Do not bypass existing confirmations.

Approvals must be bound to the exact action and arguments. If the action changes, request approval again.

The model must never be able to grant itself permissions.

---

# 9. Phase Six — Unified Tool and Skill System

**Priority: P1**

Buddy already has several useful integrations.

Build a registry that exposes them through a consistent interface.

Create:

```
src/
  skills.rs
  skills/
    registry.rs
    developer.rs
    research.rs
    desktop.rs
```

### Tool interface

Each tool should provide:

- Name.
- Description.
- Input schema.
- Risk classification.
- Required capabilities.
- Execution function.
- Structured result.
- Error information.

Create a registry that can discover available tools and validate their arguments.

### Reuse existing tools

Build adapters around existing functionality:


| Existing module                   | Tool capabilities                   |
| --------------------------------- | ----------------------------------- |
| [`webread.rs`](http://webread.rs) | Search and read web content         |
| [`browse.rs`](http://browse.rs)   | Browser interaction                 |
| [`drive.rs`](http://drive.rs)     | Desktop automation                  |
| [`assist.rs`](http://assist.rs)   | Application actions and messaging   |
| [`media.rs`](http://media.rs)     | Media controls                      |
| [`proc.rs`](http://proc.rs)       | Application and process information |
| [`screen.rs`](http://screen.rs)   | Screen capture                      |
| [`mcp.rs`](http://mcp.rs)         | External MCP tools                  |


Do not duplicate these implementations.

### Skills versus tools

A tool performs an individual action.

A skill defines how to complete a category of work using multiple tools.

Implement initial built-in skills:

**Developer**

- Inspect a codebase.
- Understand project structure.
- Find bugs.
- Modify code.
- Run tests.
- Review changes.

**Research**

- Search multiple sources.
- Read and compare sources.
- Extract findings.
- Identify disagreements.
- Produce structured reports.

**Desktop**

- Inspect active applications.
- Navigate application interfaces.
- Perform approved interactions.
- Verify outcomes.

**Personal**

- Retrieve relevant memories.
- Manage tasks.
- Maintain project context.
- Recall previous decisions.

External installable skills can come later, after sandboxing and permission controls are established.

---

# 10. Phase Seven — Context Engine

**Priority: P2**

Buddy should understand relevant context without constantly sending unnecessary information to models.

Create a context engine that can provide:

```
pub struct AgentContext {
    pub current_time: String,
    pub active_window: Option<String>,
    pub active_application: Option<String>,
    pub media_context: Option<String>,
    pub active_project: Option<String>,
    pub relevant_memories: Vec<String>,
    pub active_task: Option<String>,
}
```

Use existing Windows integrations.

Retrieve context only when it is relevant to the task.

Examples:

- Coding request → inspect the active project.
- Media request → inspect current media state.
- Application request → inspect running applications.
- Follow-up question → retrieve relevant conversation and project memory.

Do not capture or transmit screenshots unnecessarily.

Treat webpages, file contents, OCR, window titles and other external text as untrusted data, never as instructions that can override system policy.

---

# 11. Phase Eight — Settings and User Experience

**Priority: P2**

Expose the new capabilities in Zor-UI settings.

Add settings for:

### Intelligence

- Automatic model routing.
- Manual model selection.
- Preferred reasoning model.
- Preferred coding model.
- Preferred vision model.
- Local-model preference.
- Fallback behavior.

### Autonomy

- Balanced autonomy.
- Approval requirements.
- File modification scope.
- Execution limits.

### Memory

- Enable or disable persistent memory.
- View and search memories.
- Edit and delete memories.
- Clear history.
- Configure exclusions.

### Tasks

- Show active tasks.
- Show execution progress.
- Show completed tasks.
- Cancel running tasks.
- Resume interrupted tasks.

Keep the UI consistent with existing Zor-UI design patterns.

Do not overload the interface with technical configuration that ordinary users do not need.

---

# 12. Phase Nine — Testing and Reliability

**Priority: P0 throughout implementation**

Create deterministic tests using fake model providers and mock tools.

Test the following scenarios:

1. Simple conversational request.
2. Multi-step task execution.
3. Model provider failure.
4. Automatic provider fallback.
5. Tool argument validation.
6. Native tool calling.
7. JSON compatibility tool calling.
8. Failed tool execution.
9. Recovery and replanning.
10. Persistent memory across restarts.
11. Memory retrieval relevance.
12. Memory deduplication.
13. Memory updates.
14. Sensitive memory exclusion.
15. Approval for consequential actions.
16. Rejection of an approval request.
17. Cancellation during execution.
18. Interrupted task recovery.
19. False completion detection.
20. Context-window overflow.

Add integration tests for realistic workflows.

### Performance and quality metrics

Track:

- Task completion rate.
- False completion rate.
- Tool selection accuracy.
- Planning quality.
- Memory retrieval relevance.
- Recovery success rate.
- Model latency.
- Token usage.
- Estimated model cost.

Do not log API keys, sensitive memory contents or private message contents.

---

# 13. Implementation Order

Do not implement everything in one giant change.

Work in the following milestones.

### Milestone 1 — Intelligence Foundation

- Fix Buddy's system prompt.
- Respect user-configured temperature.
- Fix the current completion and continuation logic.
- Introduce normalized model request and response types.
- Introduce provider abstraction.
- Implement basic automatic model routing.
- Support structured tool calls.
- Add deterministic tests.

**Acceptance criteria:** Buddy can complete a multi-step task using more than one configured provider without relying on provider-specific agent logic.

### Milestone 2 — Persistent Memory

- Add SQLite.
- Add schema and migrations.
- Persist conversations.
- Implement memory extraction.
- Implement retrieval.
- Implement deduplication and updates.
- Add memory settings.
- Persist task state.

**Acceptance criteria:** Buddy remembers useful information after Zor-UI is closed and reopened, and can retrieve it in a relevant conversation.

### Milestone 3 — Autonomous Execution

- Implement planner.
- Implement executor.
- Implement task state machine.
- Implement centralized policy.
- Implement tool registry.
- Add verification.
- Add retry and replanning.
- Preserve existing browser and desktop safeguards.

**Acceptance criteria:** Buddy can execute a multi-step workflow, recover from a failed step and verify the final result.

### Milestone 4 — Skills and Context

- Add Developer skill.
- Add Research skill.
- Add Desktop skill.
- Add Personal skill.
- Add context engine.
- Improve context selection.
- Add active task UI.

**Acceptance criteria:** Buddy can perform realistic workflows across the existing Zor-UI integrations with relevant context.

### Milestone 5 — Optimization

- Improve routing decisions.
- Add optional semantic memory retrieval.
- Optimize context usage.
- Improve latency and token consumption.
- Add benchmarks.
- Refine settings and progress UI.

**Acceptance criteria:** The assistant is reliable, measurable, configurable and efficient.

---

# 14. Engineering Constraints

These are mandatory.

1. Use Rust for the intelligence core.
2. Preserve the existing application architecture.
3. Reuse existing browser, desktop, process, media and MCP integrations.
4. Do not hardcode a single AI provider.
5. Do not make model-specific assumptions in the planner or executor.
6. All model communication must pass through the normalized provider layer.
7. All tool execution must pass through the tool registry and policy engine.
8. The model must never override user permissions.
9. Persistent task state must survive application restarts.
10. Memory must be local by default and user-controlled.
11. Tool success must not be confused with task completion.
12. Do not introduce unnecessary heavyweight frameworks.
13. Do not introduce a separate Python backend.
14. Do not introduce an always-running external vector database.
15. Preserve current voice and Buddy UI functionality.
16. Maintain backward compatibility with existing configuration where practical.
17. Do not remove existing integrations simply because a new abstraction is introduced.
18. Keep changes modular, testable and documented.

---

# 15. How the AI Coding Agent Must Work

Follow this workflow strictly.

**Step 1 — Inspect**

Read the actual source files before proposing changes. Do not rely solely on this document.

**Step 2 — Report**

Produce:

- Architecture assessment.
- Exact files to change.
- New files to create.
- Dependency changes.
- Potential compatibility problems.
- Implementation sequence.

**Step 3 — Implement incrementally**

Implement one milestone at a time.

Do not make unrelated refactors.

**Step 4 — Build and test**

After each meaningful change:

- Run formatting.
- Run `cargo check`.
- Run relevant unit tests.
- Run the full test suite when appropriate.
- Build the Windows application when possible.

Fix regressions before continuing.

**Step 5 — Report honestly**

For every milestone, report:

- What was implemented.
- Files created or modified.
- Tests executed.
- Build status.
- Known limitations.
- Remaining tasks.

Never claim something works without testing it.

Do not silently replace a feature with a placeholder.

If a provider does not support a capability, report it and use an explicitly documented fallback.

**Step 6 — Maintain documentation**

Update:

- Architecture documentation.
- Configuration documentation.
- Tool and skill documentation.
- Database schema and migration documentation.
- Development and testing instructions.

---

# 16. Definition of Done

The upgrade is complete when Buddy can:

- Understand a complex user objective.
- Break it into a sensible plan.
- Automatically select an appropriate available model.
- Use different providers without changing agent logic.
- Call existing tools through a unified registry.
- Execute multi-step workflows.
- Recover from errors and replan.
- Ask permission before consequential actions.
- Remember relevant information across sessions.
- Understand relevant desktop and project context.
- Verify actual outcomes.
- Resume interrupted tasks.
- Explain what it did and what it could not do.

The final result should feel like a capable desktop assistant, not a chatbot with a collection of disconnected commands.

## Status: M0 and M1

**M0 (audit and baseline), done.** The modules this brief names live on `origin/main` (commit `9a41524`, merged to main
through `8c8d46d`). Findings and their file references are in the M1 plan. Main of this checkout was fast-forwarded to
`origin/main` for the audit. Baseline on main before any M1 edit: `zorui` 938 passed, 10 ignored; `zorui-svc` 13 passed;
release build 5m 00s; `zorui.exe` 8,103,424 bytes; 59 warnings.

**M1 (verification and task-state reliability), done on branch `buddy-intel-m1b`.**
- Verdict decides the saved state: Completed, Unverified, Partial, Failed, Declined or Cancelled. A model's "Done." after
  failed tools is saved as Failed (`agent::outcome`, `state::status_of`).
- A declined tool is `Check::Declined`, not a failure.
- A failure that a later step recovers from is not counted (`verifier::judge`).
- Radio and brightness are verified by reading the real state back; brightness is refused unless the level matches.
- `open`-style acting calls that failed in a request are not run again (`agent::already_failed`).
- Tests added in `agent/verifier.rs`, `agent/state.rs`, `agent/tasks.rs` and `agent.rs`.
- Tests after M1: `zorui` 948 passed, 0 failed, 10 ignored; `zorui-svc` 13 passed. Warnings unchanged at 59.
- Remaining limitations (M1): volume and opening apps still have no read-back; the repeat block covers only the same call
  with the same arguments.

**M2 (centralized permission enforcement), done on branch `buddy-intel-m1b`.**
- Every risky call is asked in one place: `agent::approve`, with the question from `policy::approval_for`. Power (not lock),
  take-over and the browser are the three cases. The old asks in `power()`, `browse::run` and `drive::start` (for the agent
  path) are gone.
- A risky call whose level needs approval but has no question is refused (fails closed).
- Take-over asks on the request's own thread before it starts, so a "no" is a Declined task, not a silent "Checking with you
  first." The switches and the model check run before the question, so a take-over that can't run asks nothing.
- The browser's yes is per request (`browse::approve_request`); `browse::run` refuses a step that arrives without it.
- Tests: `policy.rs` (level and question agree for every risky call), `agent.rs` (`approve`: a no runs nothing, lock and
  lookups ask nothing, an unknown risky call is refused), `browse.rs` (a step without approval asks nothing).
- Tests after M2: `zorui` 951 passed, 0 failed, 10 ignored; `zorui-svc` 13 passed. Warnings 59 (unchanged).
- Remaining limitations (M2): the browser's per-step gate still lives in `browse::gate` (it is per action, not per request);
  the take-over's question text is built in `drive::question`, which `policy` calls.

**M3 (progressive context), done on branch `buddy-intel-m1b`.**
- Two sources added to `AgentContext`: the previous saved request and how it ended (when the words point back), and the
  names of the five newest Recent-folder shortcuts (when the words ask about files).
- Intent rules in Rust, no extra model call: a request that points back with nothing earlier gets one instruction to ask one
  short question.
- The whole context is cut to 700 characters.
- Tests: reference words, the newest-shortcut order in a temp folder, the previous-request and question rules, and the budget.
- Tests after M3: `zorui` 959 passed, 0 failed, 10 ignored; `zorui-svc` 13 passed. Warnings 59 (unchanged).
- Live: "what files did I open recently" listed real Recent files; "continue where we left off" answered from the last request.
- Remaining limitations (M3): no active-project source; "it" is not a reference word; the memory lookup still opens the
  database on every request; intent rules are keyword-based, so an unusual phrasing gets no earlier request.

**Start with Milestone 1. Inspect the repository, establish the baseline build, and implement the smallest complete, tested intelligence foundation before moving to persistent memory. Do not attempt the entire upgrade in one unverified change.**