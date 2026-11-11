# M9: Developer skill, scope

Status: M9a (read-only) and M9b (checks) are built. M9c (edits) is not started. Items marked **Decided** carry your answer;
items marked **Open** are still waiting for you.

## Goal

Let the buddy inspect a project, search it, run its tests and builds, and make small edits, so it can work on the
Zor-UI codebase when you authorize it (brief section 9). It is not a shell. Every tool runs inside one workspace, and
every risky call goes through the policy layer that already exists (`agent/policy.rs`, `agent::approve`).

## Non-goals

- No general shell, and no command string built from the AI's text.
- No network access from the developer tools. Web lookups stay in the research skill.
- No git push, commit, or branch changes. Reading diffs is allowed; writing history is not.
- No access outside the workspace, and no reading of secrets (see Secrets).

## Workspace

- A workspace is one folder, added by you in Settings. The buddy has none until you add one.
- Every path the AI gives is resolved to its real location (`std::fs::canonicalize`) before the check. The existing
  `agent::inside()` compares strings only, so a junction or symlink inside a workspace could point outside it. The
  developer tools must not reuse that check as it is.
- A path that does not resolve inside the workspace is refused with a reason, and the refusal is logged.
- **Decided:** the workspace is a folder Buddy creates itself, a new test folder, and it is the only workspace for now.
  The Zor-UI repository is NOT a workspace. The setting is the existing `buddies.developerWorkspace` key. When it is unset,
  the developer tools stay unavailable, except that the create helper below may make the test folder when asked.
- **Test folder:** `developer::create_test_workspace()` writes `%LOCALAPPDATA%\zor-ui\dev-workspace`: a small Cargo project
  (`Cargo.toml`, `.gitignore`, `src\main.rs`) written by hand, with no `cargo new` and no network. Files already there are
  kept. It runs only when called, never at startup, and it does not write Settings: you set `buddies.developerWorkspace` to
  that folder yourself. No button calls it yet; the Settings button comes later.
- A `..` in the setting is refused. A junction or symlink in the setting is followed (the setting is your own choice, not AI
  text); the AI's paths are checked as in M9a.

## Tools and risk levels

Each tool maps to a risk level in `policy::risk_of`. A new tool that is not in the table does not compile, so it cannot
bypass the policy (brief section 3.2).

| Tool | What it does | Risk | Approval |
|---|---|---|---|
| `list_tree` | names of files and folders under a path in the workspace, depth-limited | ReadOnly | none |
| `read_file` | text of one file in the workspace, size-capped, secrets refused | ReadOnly | none |
| `search_code` | text search in the workspace, result-capped, skips build output and `.git` | ReadOnly | none |
| `git_diff` | `git diff` / `git status` for the workspace, read-only subcommands only | ReadOnly | none |
| `edit_file` | replace text in one file in the workspace, written atomically, with a backup | Reversible | per task, see below |
| `run_check` | run one allowed command (see Commands) in the workspace, with a timeout | Consequential | per workspace, see below |

**Open** (for M9c): should `edit_file` ask each time, or once per task? Suggestion: once per task, with the list of files
shown in the island before the first edit.

**Decided:** `run_check` asks once per workspace, and the question names the command. The yes is kept in memory for the
rest of the app session only (not Settings, the DPAPI store or `buddy.db`). A no makes the task Declined.

## Commands (`run_check`)

- Fixed argument lists only. The AI picks one entry from the list; it never supplies argv.
  Suggested first list: `cargo check`, `cargo test`, `cargo build` (with `--release` allowed), and `cargo clippy`.
- Each runs with the workspace as its working directory, with no shell, a time limit (suggest 10 minutes), and output
  capped at a fixed size.
- Anything not on the list is refused before it runs. Nothing is added to the list by the AI.
- **Decided:** no extra commands. The list is exactly `cargo check`, `cargo test`, `cargo build`, `cargo build --release`
  and `cargo clippy`, held in `developer::CHECKS`. The AI chooses by name; the argument lists are fixed in code.
- **Built limits (M9b):** no shell (`std::process::Command`, stdin closed); the working directory is the canonical
  workspace; a wall-clock limit of 10 minutes, after which the child is killed (a cargo child's `rustc` may outlive it on
  Windows); stdout and stderr are redacted, then cut to their last 16 KB with a note that the start was cut.
- Verdicts: a check that exits 0 is passing evidence. A check that failed (exit code or time limit) keeps the task from
  Completed for the whole request, even when a later lookup works. A task that claims a code result (tests pass, builds,
  clippy clean) with no check run ends Unverified. A refused name asks nothing and starts nothing.
- Not in M9b: `edit_file`, and the rule that the next check must pass after the last edit (M9c). A failed check is not
  run again within one request (the generic rule for acting calls refuses a repeat), so a retry needs an edit first.

## Secrets

- Never read: `.env*`, `*.pem`, `*.pfx`, `*.key`, `id_rsa*`, `credentials*`, `*.kdbx`, and any file under `.git/`.
- Output from `read_file`, `search_code` and `run_check` is scanned for key-like patterns and redacted before the AI
  sees it.
- The developer tools never write to Settings, the DPAPI-encrypted key store, or `buddy.db`, except the task rows the
  app already writes.

## Verification after changes

- After `edit_file`, the next `run_check` is the verification. A task that edited files is not Completed until one
  `run_check` has passed after the last edit (brief section 3.1). The verifier gets a new kind for this.
- If no check exists for the workspace, the task ends Unverified, and the answer says so.

## Acceptance tests

- Path resolution: a symlink or junction inside the workspace that points outside is refused. A `..` path is refused.
- Secrets: `read_file` on `.env` is refused; a key-like string in a search result is redacted.
- Commands: a command not on the list is refused; the argv is never built from AI text (a test feeds shell
  metacharacters and checks they reach nothing).
- Approval: `edit_file` and `run_check` do not run without the answer; a "no" is a Declined task.
- Verification: a task that edited a file and ran no check ends Unverified, not Completed.
- Limits: output and time caps are enforced.

## Split

- **M9a (read-only, built):** `list_tree`, `read_file`, `search_code`, `git_status` (the scope named `git_diff`; the built tool is
  `git_status`, which prints the status and a diff summary), the workspace check, the secrets rules, and their tests.
- **M9b (checks, built):** `run_check` with the command list, its approval, the limits, and the verifier change.
- **M9c (edits, not started):** `edit_file`, its approval, the backups, and the task-level verification.

## Open decisions, in one list

1. Which folders are workspaces at first. **Decided:** a test folder Buddy creates; the Zor-UI repository is not a workspace.
2. Whether `edit_file` asks once per task or every time. **Open** (decided with M9c; suggestion: once per task).
3. Whether `run_check` asks once per workspace or every time. **Decided:** once per workspace, for the app session.
4. Which commands beyond cargo to allow. **Decided:** none; the five cargo entries only.
5. Whether M9a ships before M9b and M9c. **Done:** M9a shipped first, then M9b.
