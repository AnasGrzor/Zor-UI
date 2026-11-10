# Claude accounts: one folder per account

Status: plan, not started.

## Goal

Replace the file swap in `native/zor-ui/src/bridge/claude_acct.rs` with the model Orca uses. Each saved Claude login gets its own Claude Code config folder, and the active account is picked with `CLAUDE_CONFIG_DIR`. Today a switch rewrites `claudeAiOauth` in `~\.claude\.credentials.json` and `oauthAccount` in `~\.claude.json`. After this change zor-ui never edits those two files.

What this gains:

- Two accounts can run at once, in different terminals.
- A switch no longer needs Claude Code's lock directories or the JSON splicing code.
- A running session keeps its account when you switch. Today a switch changes the login under a session that is still running.

What it costs:

- A switch reaches new terminals only. Open terminals keep the account they started with.
- Each account folder needs the shared parts of `~\.claude` (history, projects, settings) linked in, or every account looks like a fresh install.

## How Orca does it (for reference)

Read from the installed Orca bundle, `resources/app.asar.unpacked/out`.

- Account folders live at `<orca data>\claude-profiles\<accountId>\home`. A `profile.json` beside `home` ties the folder to one account.
- Each terminal pane gets `CLAUDE_CONFIG_DIR` pointing at the chosen folder. A `claude` shell function re-reads a pointer file (`ORCA_CLAUDE_PROFILE_POINTER`) on every launch.
- The user's own `CLAUDE_CONFIG_DIR` is saved in `ORCA_CLAUDE_USER_CONFIG_DIR`. Orca's "System default" choice uses it.
- A setup worker links shared state from the default home into each profile. Directories (`projects`, `todos`, `plans`, `file-history`, `shell-snapshots`, `session-env`, `tasks`, `paste-cache`, `transcripts`) become junctions on Windows. `history.jsonl` becomes a hard link, and Orca records the file id so it can tell when Claude Code replaced the link with a new file.
- The profile's `CLAUDE.md` holds one line, `@~/.claude/CLAUDE.md`, so the user's global instructions are imported, not copied.
- Account-specific keys of `.claude.json` (`oauthAccount`, model option caches) stay per profile.

zor-ui can't set a variable inside another app's terminal the way Orca does for its own panes. The nearest equivalent is the user environment variable, described in step 3.

## Step 0: check Claude Code's behaviour (no code)

Run these by hand before building anything. Each answer changes a step below.

1. Set `CLAUDE_CONFIG_DIR` to an empty folder and run `claude`. Record where `.claude.json`, `.credentials.json` and the lock directories (`.oauth_refresh.lock`, `.claude.json.lock`) appear. The plan assumes all of them land inside the folder.
2. Set `CLAUDE_CONFIG_DIR` in `HKCU\Environment`, broadcast `WM_SETTINGCHANGE`, then open a new Windows Terminal tab and a new Orca pane. Record which ones see the new value without a restart.
3. With a session running in folder A, have a second `claude` process in folder A refresh its token. Record whether the first session keeps working. This decides whether zor-ui may renew tokens in a folder that a live session uses (step 5).

## Step 1: account folders

New layout, under `crate::conf::data_dir()`:

```
claude-profiles\
  <n>\
    profile.json        { "n": 1, "email": "...", "org": "...", "orgName": "...", "plan": "max" }
    home\               the CLAUDE_CONFIG_DIR for this account
      .credentials.json Claude Code's own file, written by `claude /login`
      .claude.json      Claude Code's own file
      CLAUDE.md         "@~/.claude/CLAUDE.md"
      projects\ ...     junctions into ~\.claude
```

- `~\.claude` stays as the "Default" account. Choosing it clears `CLAUDE_CONFIG_DIR`.
- Tokens sit in plain JSON, as Claude Code keeps them in `~\.claude`. The DPAPI slot blobs go away. The folder inherits the user-only ACL of `%LOCALAPPDATA%`.
- Path checks from Orca to copy: the folder must sit under `claude-profiles`, no path part may be a link, and the profile folder and `~\.claude` must not contain each other.

## Step 2: link shared state

A `link_shared(profile_home)` function runs when a profile is created and again on every switch, so a link that Claude Code replaced gets restored.

- Directories become junctions (no admin rights needed): `projects`, `todos`, `plans`, `file-history`, `shell-snapshots`, `session-env`, `tasks`, `paste-cache`, `transcripts`, `commands`, `agents`, `skills`, `plugins`.
- `settings.json` is copied from `~\.claude` on every switch. A hard link would break the first time Claude Code saves the file with a write-and-rename. The copy also brings zor-ui's own hooks (`agent_hook.rs`) into every profile.
- `history.jsonl` is a hard link. If the link was replaced, merge the profile's lines into the shared file and link again, as Orca does.
- `CLAUDE.md` gets the one-line import.
- `.claude.json` is seeded once, at creation, from `~\.claude.json` without `oauthAccount`. After that the profile owns it. MCP servers added later to the default home won't appear in other profiles (open question 1).
- A path that already exists and isn't zor-ui's link is left alone and reported. Never delete user data to make room for a link.

## Step 3: switching

`switch(n)` becomes:

1. Write `HKCU\Environment\CLAUDE_CONFIG_DIR` = `<profile>\home`, or delete the value for Default.
2. Broadcast `WM_SETTINGCHANGE` with `"Environment"` so Explorer and new terminals pick it up.
3. Run `link_shared`.
4. Store the active `n` in config (`claude.active`), so zor-ui doesn't depend on its own stale process environment.

Deleted with the old switch: `value_span`, `value_end`, `splice`, `read_obj` for the two files, `lock_all`, `take`, `sync_live`, `seal`, `open`, the "save it first, or switching away would lose it" refusal.

If you set `CLAUDE_CONFIG_DIR` yourself, zor-ui saves the old value as `claude.userConfigDir` the first time and restores it when you choose Default.

## Step 4: adding and signing in

- "Add account" creates the next `claude-profiles\<n>`, runs `link_shared`, then opens a terminal with `CLAUDE_CONFIG_DIR` set to that folder running `claude /login`.
- zor-ui polls the folder every 2 s for 5 minutes. When `.credentials.json` has an access token and `.claude.json` has `oauthAccount`, it writes `profile.json` and the row appears. If the email matches an existing profile, the new folder is removed and the old one is reused.
- "Sign in again" on an expired row opens the same terminal in that row's folder.
- "Remove" asks for confirmation (today it doesn't), then deletes the folder. Junctions are removed as links first, so the shared folders behind them are never followed.

## Step 5: usage and renewal

- `claude_usage::token()` reads `.credentials.json` from the active folder (`claude.active`), not from `USERPROFILE`.
- `list()` reads each profile's token straight from its folder.
- Renewal depends on step 0.3. If a second process renewing is safe, zor-ui renews under the profile's own `.oauth_refresh.lock`, re-reads the file under the lock, and skips the renewal if another process already did it. If it's not safe, zor-ui never renews and shows "usage stale" for an expired token.
- `invalid_grant` still marks the row expired.

## Step 6: auto-switch

`pick()` and the countdown stay. The banner changes to "New Claude sessions will use {email}", because open sessions keep their account. The cooldown stays at 10 minutes.

## Step 7: other callers

- `claude_cli.rs` (sidebar chat through the `claude` command): set `CLAUDE_CONFIG_DIR` on the spawned process from `claude.active`. zor-ui's own environment is the one it started with.
- `agent_hook.rs` `dirs_of`: add every profile `home` to the folders it installs hooks into. This covers the time before the next switch copies `settings.json`.

## Step 8: migration

On first start after the update, for each slot in `claude-accounts.json`:

1. Decrypt the slot (the existing `open`).
2. Create `claude-profiles\<n>\home`, write `.credentials.json` with `claudeAiOauth`, and seed `.claude.json` from `~\.claude.json` with that slot's `oauthAccount`.
3. Run `link_shared`.

After all slots succeed, rename `claude-accounts.json` to `claude-accounts.json.migrated`. A slot that fails stays in the old file and the row shows "Sign in again". The account currently in `~\.claude` stays as Default and also gets no profile, so it isn't logged in twice.

The login in `~\.claude` and a profile for the same account would share one refresh token. The first one to renew logs out the other. Migration therefore skips the slot that matches the Default login.

## Step 9: Settings page

Services > Claude accounts, rows in this order:

- Default (`~\.claude`), then each profile. Each row shows email, plan and usage as today.
- Buttons per row: "Use" (or "Active"), "Sign in again" when expired, "Remove" with a confirm.
- Under the rows: "Add account", the auto-switch switch and the threshold slider.
- A note: "Switching applies to new terminals. Open sessions keep their account."

No setting here changes how something looks, so the live preview rule in CLAUDE.md doesn't apply.

## Tests

- Profile path checks: escaping the root, a link in the path, nesting with `~\.claude`.
- `link_shared` against a temp home: creates junctions, leaves an existing real folder alone, restores a replaced `history.jsonl` link and merges its lines, copies `settings.json`.
- Switch: sets and clears the variable through a fake environment writer, restores the user's own value on Default.
- Migration: two slots become two folders; the slot that matches Default is skipped; a failed slot stays in the old file.
- Sign-in poll: detects a finished login, merges into an existing profile with the same email.
- Remove: deletes the folder without following junctions.
- Keep the existing `pick`, `cooled`, `summary`, `token_ok` and `read_renewal` tests.

## Open questions

1. Should MCP servers and project trust from `~\.claude.json` keep syncing into profiles after creation? Doing it means editing a file that a live session may be writing.
2. Which terminal should "Add account" open: Windows Terminal, or the terminal set in zor-ui's config?
3. Orca reads the user's `CLAUDE_CONFIG_DIR` when Orca starts. Orca panes on "System default" won't follow a zor-ui switch until Orca restarts. Acceptable, or should zor-ui say so in the banner?
