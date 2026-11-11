# M9: Developer skill, scope

Status: proposal. Nothing here is built yet. Items marked **Decision** need your answer before M9 starts.

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
- **Decision:** which folders are workspaces at first. Suggestion: only the Zor-UI repository.

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

**Decision:** should `edit_file` ask each time, or once per task? Suggestion: once per task, with the list of files
shown in the island before the first edit.

**Decision:** should `run_check` ask once per workspace or every time? Suggestion: once per workspace, and the question
names the command.

## Commands (`run_check`)

- Fixed argument lists only. The AI picks one entry from the list; it never supplies argv.
  Suggested first list: `cargo check`, `cargo test`, `cargo build` (with `--release` allowed), and `cargo clippy`.
- Each runs with the workspace as its working directory, with no shell, a time limit (suggest 10 minutes), and output
  capped at a fixed size.
- Anything not on the list is refused before it runs. Nothing is added to the list by the AI.
- **Decision:** which extra commands to allow, if any. Suggestion: none beyond cargo for now.

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

- **M9a (read-only):** `list_tree`, `read_file`, `search_code`, `git_diff`, the workspace check, the secrets rules, and
  their tests. No approvals needed, so it is safe to ship first.
- **M9b (checks):** `run_check` with the command list, its approval, and the verifier change.
- **M9c (edits):** `edit_file`, its approval, the backups, and the task-level verification.

## Open decisions, in one list

1. Which folders are workspaces at first (suggest: the Zor-UI repository only).
2. Whether `edit_file` asks once per task or every time (suggest: once per task).
3. Whether `run_check` asks once per workspace or every time (suggest: once per workspace).
4. Which commands beyond cargo to allow (suggest: none).
5. Whether M9a ships before M9b and M9c (suggest: yes).
