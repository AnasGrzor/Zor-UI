# Zor UI — Dynamic Island 2.0

## Engineering &amp; Implementation Brief

### 1. Objective

Transform Zor UI's existing Dynamic Island into an intelligent, context-aware desktop activity center.

The goal is NOT to simply add more widgets or visual effects.

The goal is to create a unified activity system that understands what's happening on the computer, determines what deserves attention, and presents the most relevant information through a polished, adaptive interface.

The implementation must preserve Zor UI's existing native Rust architecture and all currently working functionality.

**Core principles:**

- Intelligent activity prioritization.
- Minimal visual clutter.
- Smooth, premium animations.
- Independent and extensible activity modules.
- Low CPU and memory consumption.
- Native rendering without WebViews.
- User-configurable behavior.

---

### 2. Existing implementation — preserve and improve

First, audit the existing Dynamic Island implementation.

Known relevant files:

- `native/zor-ui/src/[island.rs](http://island.rs)`
- `native/zor-ui/src/settings/[pages.rs](http://pages.rs)`

Verify actual repository paths and inspect all associated modules before modifying anything.

Existing functionality includes:

**Media**

- Now Playing.
- Album artwork.
- Playback progress.
- Lyrics integration.
- Equalizer.
- Previous, play/pause and next controls.
- Hover controls.

**System**

- Volume changes.
- Microphone mute.
- Microphone and camera activity.
- Screen recording status and duration.
- Notifications.

**Time**

- Timer.
- Stopwatch.
- Clock.
- Weather.
- Idle display.

**AI agents**

- Claude Code.
- Codex.
- Gemini CLI.
- Custom agent integrations.
- Agent working states.
- Agent permission requests.
- Agent questions.
- Usage percentages and reset times.
- Agent notification handling.

**Customization**

- Island positioning.
- Width settings.
- Expanded width.
- Click and right-click actions.
- Scroll behavior.
- Idle display preferences.
- Activity visibility.
- Expanded panel sections.

Do not remove, silently disable, or regress these features.

---

### 3. New architecture: Activity Manager

Introduce a unified activity management system.

Suggested modules:

[`activity.rs`](http://activity.rs)

- Common activity representation.
- Activity types.
- Activity states.
- Activity metadata.
- Priority definitions.

`activity_[manager.rs](http://manager.rs)`

- Register and unregister activities.
- Update activity states.
- Manage activity lifecycle.
- Handle activity dismissal.
- Track active activities.

`activity_[scheduler.rs](http://scheduler.rs)`

- Determine which activity should be displayed.
- Resolve priority conflicts.
- Handle temporary interruptions.
- Manage activity expiration.
- Restore previously visible activities.

`island_[layout.rs](http://layout.rs)`

- Compact presentation.
- Expanded presentation.
- Ambient presentation.
- Context-dependent layout decisions.

`island_[animation.rs](http://animation.rs)`

- Expansion and contraction.
- Content transitions.
- Progress animations.
- Activity entrance and exit.
- Animation timing and reduced-motion support.

[`island.rs`](http://island.rs)

- Integrate the new architecture with the existing native renderer.
- Preserve existing input handling and rendering behavior.
- Avoid turning this into a complete rewrite.

These are proposed module boundaries, not mandatory filenames. Follow existing repository conventions where appropriate.

### 4. Unified activity model

Every activity should publish standardized information.

An activity should contain:

- Unique identifier.
- Activity type.
- Source.
- Display title.
- Subtitle or description.
- Current state.
- Priority.
- Creation timestamp.
- Last update timestamp.
- Optional expiration.
- Optional progress.
- Available actions.
- Whether it can be dismissed.
- Whether it should remain pinned.

Suggested activity states:

- Idle
- Starting
- Active
- Working
- Waiting
- Completed
- Failed
- Dismissed

Activities must be independent of the presentation layer.

For example, the timer must not know whether it is being displayed in compact or expanded mode.

It should publish its current state, and the presentation engine should decide how to render it.

Use strongly typed Rust structures and enums rather than loosely structured JSON wherever possible.

### 5. Intelligent prioritization

Implement a configurable priority system.

Suggested initial priority levels:


| Priority   | Activity                  |
| ---------- | ------------------------- |
| Critical   | Agent approval request    |
| Critical   | Agent requires user input |
| High       | Recording started/stopped |
| High       | Timer completed           |
| Medium     | New notification          |
| Medium     | New media track           |
| Low        | Agent working             |
| Background | Clock and weather         |


Important behavior:

**Scenario A: Music + AI agent**

1. Spotify is currently visible.
2. Claude Code starts working.
3. The music display remains visible.
4. Claude requests permission.
5. The island temporarily switches to the approval request.
6. The user responds.
7. The island returns to Spotify.

**Scenario B: Multiple agents**

1. Claude is working.
2. Codex is waiting for approval.
3. Gemini is idle.
4. Codex receives priority.
5. Claude remains accessible through the activity switcher.
6. Gemini does not unnecessarily occupy the compact island.

**Scenario C: Timer completion**

1. Music is playing.
2. A timer reaches zero.
3. The island briefly displays timer completion.
4. The user can dismiss it.
5. The previous activity is restored.

Never silently approve agent operations.

The presentation system must never bypass existing agent permission checks.

Priority should be configurable in settings.

Avoid building an overly complicated AI-based scheduler. Deterministic rules should handle core scheduling. Future intelligent enhancements can be added separately.

### 6. Three presentation modes

Implement three presentation states using the same underlying activity data.

#### A. Compact

The default state.

Requirements:

- Small floating pill.
- Current activity icon.
- Short status text.
- Optional progress indicator.
- Minimal animation.
- No unnecessary descriptions.

Examples:

- Music: icon + track.
- Agent: agent icon + Working.
- Timer: countdown.
- Recording: red recording indicator.

#### B. Expanded

The detailed activity interface.

Requirements:

- Context-specific layout.
- Rich activity information.
- Interactive controls.
- Multiple active activities.
- Activity switching.
- Appropriate use of existing island width settings.

Examples:

Music:

- Artwork.
- Song and artist.
- Progress.
- Lyrics.
- Playback controls.

AI agent:

- Agent name.
- Current task.
- Working state.
- Usage information.
- Reset countdown.
- Approval actions.

Timer:

- Large countdown.
- Pause/resume.
- Reset.
- Stop.

#### C. Ambient

A minimal background state.

Requirements:

- Small indicators for background activities.
- Running agent count.
- Recording status.
- Active timer indicator.
- Minimal resource consumption.

Ambient indicators should not continuously demand attention.

### 7. Activity-specific improvements

#### AI agents

This is a major differentiator for Zor UI.

Improve the agent presentation with:

- Current task description.
- Execution duration.
- Working/waiting/completed status.
- Current tool or operation, when available.
- Usage percentage.
- Reset countdown.
- Permission prompts.
- Error indicators.
- Completion summaries.

Support multiple simultaneously active agents.

Reuse existing agent hooks and integrations rather than creating duplicate detection systems.

#### Music

Improve:

- Smooth track transitions.
- Artwork transitions.
- Progress synchronization.
- Lyrics presentation.
- Equalizer animations.
- Playback controls.

Preserve existing media integrations.

#### Notifications

Improve:

- Grouping by application.
- Notification count.
- Individual dismissal.
- Clear all.
- Temporary notification previews.

Do not introduce a new notification backend if the existing bridge provides the required information.

#### Timer and recording

Improve:

- Persistent visibility when appropriate.
- Smooth countdown updates.
- Completion animations.
- Quick controls.
- Restoration of previous activities.

---

### 8. Animation and visual design

Maintain Zor UI's existing design language and theme integration.

The Dynamic Island should feel premium, fluid and native.

Implement:

- Spring-like expansion and contraction.
- Smooth content transitions.
- Subtle activity entrance and exit.
- Animated progress indicators.
- Smooth equalizer transitions.
- Subtle notification appearance.
- Consistent corner radii and spacing.

Important constraints:

- Avoid animating the entire island unnecessarily.
- Animate only elements whose states change.
- Avoid expensive continuous redraws.
- Pause unnecessary animations when idle.
- Respect reduced-motion preferences.
- Use existing rendering primitives and animation infrastructure.

Do not introduce a new UI framework.

Keep the existing native Rust/tiny-skia/Win32 architecture.

---

### 9. Settings improvements

Extend the existing Dynamic Island settings page.

Add the following options:

**Activity behavior**

- Activity priority customization.
- Auto-collapse delay.
- Activity pinning.
- Background activity visibility.
- Notification grouping.

**Appearance**

- Animation intensity.
- Compact presentation preferences.
- Expanded panel maximum height.
- Activity-specific visual preferences.

**Interaction**

- Activity switcher behavior.
- Click behavior.
- Dismissal behavior.
- Hover expansion.

**Focus**

- Quiet hours.
- Suppress low-priority activities.
- Preserve critical agent approvals.

Maintain existing settings keys and backward compatibility.

Existing users must not lose their preferences after updating.

Add an improved live preview if feasible, allowing users to see changes before saving.

---

### 10. Performance requirements

Performance is a first-class requirement.

The new architecture must:

- Avoid unnecessary polling.
- Prefer event-driven updates where existing integrations support them.
- Avoid duplicate activity monitoring.
- Avoid unnecessary allocations in rendering loops.
- Avoid excessive background threads.
- Avoid unnecessary redraws.
- Reuse existing timers and event infrastructure.
- Avoid expensive layout recalculations when nothing changes.

Do not add dependencies unless they provide a clear benefit.

The island should remain lightweight when idle.

---

### 11. Implementation phases

Work incrementally.

**Phase 0 — Repository audit**

Before writing code:

- Inspect the entire existing island implementation.
- Trace how activities reach the island.
- Identify existing state management.
- Inspect the rendering lifecycle.
- Inspect settings persistence.
- Identify dependencies and shared utilities.
- Document existing behavior and potential regressions.

Deliver an audit before making architectural changes.

**Phase 1 — Activity foundation**

- Implement common activity types.
- Implement activity lifecycle.
- Introduce activity registration.
- Integrate existing activity sources.
- Preserve existing rendering.

Acceptance: all existing activities continue working through the new model.

**Phase 2 — Activity Manager**

- Implement priority management.
- Implement scheduling.
- Implement expiration.
- Implement activity restoration.
- Handle simultaneous activities.

Acceptance: higher-priority activities can temporarily interrupt lower-priority ones without losing their state.

**Phase 3 — Presentation system**

- Implement compact mode.
- Implement expanded mode.
- Implement ambient mode.
- Integrate existing renderer.
- Implement activity switching.

Acceptance: all presentation modes use the same underlying activity model.

**Phase 4 — Agent experience**

- Improve multi-agent presentation.
- Add contextual task information.
- Improve approval interactions.
- Improve usage information.
- Add error and completion states.

Acceptance: multiple agents can be monitored without overwhelming the island.

**Phase 5 — Animations and polish**

- Implement transitions.
- Improve media animations.
- Add activity entrance and exit animations.
- Improve interaction feedback.
- Optimize redraw behavior.

Acceptance: transitions are smooth and do not cause excessive resource consumption.

**Phase 6 — Settings and customization**

- Add priority configuration.
- Add activity-specific display preferences.
- Add animation controls.
- Add quiet hours.
- Add preview functionality.

Acceptance: settings persist across application restarts and existing settings remain compatible.

**Phase 7 — Testing and optimization**

- Run Rust formatting.
- Run relevant tests.
- Run Clippy where supported.
- Build the application.
- Test all existing activities.
- Test simultaneous activity scenarios.
- Check performance while idle and active.

Fix regressions before considering the work complete.

---

### 12. Required test scenarios

At minimum, verify:

1. Music playing with no other activity.
2. Music playing while Claude is working.
3. Claude requests approval while music is playing.
4. Multiple agents working simultaneously.
5. Multiple agents requesting attention.
6. Timer completion during music playback.
7. Recording started while an agent is active.
8. Notifications arriving during expanded mode.
9. Activity dismissal and restoration.
10. Pinned activity persistence.
11. Switching between compact and expanded modes.
12. Changing island settings and restarting Zor UI.
13. No activity running.
14. Long-running idle session.
15. Rapid successive activity updates.

Also verify that no interaction with the island accidentally approves an agent operation.

---

### 13. Non-goals

Do not:

- Rewrite Zor UI's entire rendering system.
- Replace Rust with React or another frontend framework.
- Introduce WebViews.
- Remove existing functionality.
- Replace existing media integrations.
- Reimplement agent detection unnecessarily.
- Build an AI-powered scheduler for basic priority decisions.
- Add unnecessary third-party dependencies.
- Compromise security for convenience.

---

### 14. Final deliverables

Provide:

1. Repository audit.
2. Proposed architecture and dependency diagram.
3. Implementation plan with actual repository paths.
4. Incremental implementation.
5. Tests for new functionality.
6. Updated Dynamic Island settings.
7. Performance observations.
8. List of changed files.
9. List of existing features preserved.
10. Known limitations and future improvements.

### 15. Execution instructions

Do not blindly implement this entire specification in one massive refactor.

First audit the existing codebase and produce a concrete implementation plan referencing actual files, functions, types and dependencies.

Then implement phase by phase.

Keep changes focused and independently testable.

After each phase:

- Run relevant tests.
- Build the application.
- Fix regressions.
- Report what changed.
- Report any remaining limitations.

If the existing architecture already provides a suitable abstraction, reuse it instead of introducing a duplicate.

If a proposed feature conflicts with existing behavior, explain the conflict and propose the smallest compatible solution.

**Definition of success:**

The Zor UI Dynamic Island should become a polished, lightweight and intelligent activity center that seamlessly coordinates media, notifications, timers, system activity and AI agents — while remaining fully native, customizable and extensible.