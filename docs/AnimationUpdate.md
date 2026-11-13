Task: Bring Zor UI's animation quality closer to Coucou without replacing the existing Rust architecture.

Reference: louis-cfm/coucou, especially windows/src/core/anim.ts and windows/src/island/island.ts.

Read [CLAUDE.md](http://CLAUDE.md) first. Audit current main branch and worktree before editing.

Existing Zor UI files:

- native/zor-ui/src/[anim.rs](http://anim.rs): spring, fade, grow, mix, tests

- native/zor-ui/src/[island.rs](http://island.rs): island content

- native/zor-ui/src/[buddy.rs](http://buddy.rs): procedural Buddy animation

- native/zor-ui/src/[app.rs](http://app.rs): Win32 frame timer

- Inspect actual island geometry and window rendering owners before changes.

Goals:

1. Keep existing spring and Buddy systems.

2. Add a reusable Tracked&lt;T&gt; or scalar TrackedMotion abstraction with spring, timed-curve, and idle modes.

3. Use spring motion for expansion and cubic-bezier(.45,0,.2,1) over 340ms for contraction as an initial Coucou-derived preset.

4. Coordinate island width, height, radius, and content transitions where supported by the renderer.

5. Ensure mid-animation retargeting never snaps and existing input hit testing remains aligned with visual geometry.

6. Add consistent motion presets for island, Buddy, taskbar, and notifications.

7. Preserve demand-driven animation scheduling.

8. Respect Windows reduced-motion settings.

9. Avoid webviews, JS runtimes, new dependencies, per-element threads, and unnecessary redraws.

10. Add tests for interruptions, convergence, frame-time clamping, and geometry consistency.

First deliver a concrete implementation map after inspecting the actual geometry ownership. Then implement in small independently testable changes. Run cargo fmt, cargo test, and the appropriate Windows build checks. Report measurements and any tests that cannot run.