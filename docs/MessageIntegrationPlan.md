# Zor UI — Native Discord Buddy Integration

## Objective

Integrate Discord directly into Zor UI's existing Rust-based Buddy assistant using **Serenity**.

Users should be able to message Buddy through Discord from any device, execute authorized tasks on their Windows PC, check coding-agent progress, receive notifications, and continue conversations remotely.

Discord must act as another interface to the existing Buddy, not as a separate AI assistant.

### Hard requirements

- 100% Rust. No Node.js, Electron, Chromium, or JavaScript sidecars.
- Use Serenity for the official Discord API.
- Reuse existing Buddy intelligence, tools, memory, model routing, and permissions.
- Minimal idle CPU and memory usage.
- No additional AI runtime or duplicate tool executor.
- Optional integration, disabled by default.
- Support both Discord and WhatsApp through a shared messaging abstraction.
- Preserve existing desktop Buddy behavior.

## 1. Audit the Existing Codebase

Read CLAUDE.md before making changes.

Inspect:

- `native/zor-ui/src/agent.rs`
- `native/zor-ui/src/assist.rs`
- `native/zor-ui/src/agent/memory.rs`
- `native/zor-ui/src/agent/history.rs`
- `native/zor-ui/src/agent/tasks.rs`
- `native/zor-ui/src/agent/policy.rs`
- `native/zor-ui/src/agent/routing.rs`
- `native/zor-ui/src/settings.rs`
- `native/zor-ui/src/main.rs`
- `native/zor-ui/Cargo.toml`

Identify existing execution entry points, Tokio runtime ownership, shared state, cancellation mechanisms, and notification infrastructure.

Do not introduce duplicate systems where reusable implementations already exist.

## 2. Shared Messaging Architecture

Create a provider-agnostic messaging layer.

Proposed structure:

```
native/zor-ui/src/
  messaging/
    mod.rs
    transport.rs
    dispatcher.rs
    identity.rs
    permissions.rs
    notifications.rs

    discord/
      mod.rs
      client.rs
      events.rs
      commands.rs
      interactions.rs

    whatsapp/
      ...existing or future implementation
```

The messaging layer must support independent transports without coupling them to Buddy's internal execution logic.

Use a normalized message structure containing:

- Provider and message ID
- Sender's immutable platform ID
- Zor UI user identity
- Conversation ID
- Message content
- Timestamp
- Originating channel
- Attachment metadata

Implement an asynchronous transport interface for connection lifecycle, incoming events, outgoing messages, and connection status.

Keep provider-specific message formats inside their respective adapters.

## 3. Discord Integration

Use Serenity with Tokio.

### Authentication

- Create a Discord application and bot.
- Allow users to configure the bot token through Zor UI Settings.
- Store credentials securely using Windows-supported secret storage.
- Never expose tokens in logs, configuration exports, or crash reports.
- Support token replacement and disconnection.

### Connection

- Connect to Discord Gateway.
- Handle connection loss and automatic reconnection.
- Maintain connection status.
- Cleanly terminate the connection when disabled.
- Avoid blocking Zor UI's rendering thread.

### Messaging

For the initial implementation:

- Accept direct messages only.
- Restrict requests to explicitly authorized Discord user IDs.
- Ignore bot messages and unauthorized senders.
- Support natural-language requests without requiring slash commands.
- Use appropriate Discord Gateway intents, including Message Content where necessary.
- Send Buddy's responses back to the originating DM.
- Split responses exceeding Discord's message limits.
- Respect API rate limits.

Do not enable guild-wide command execution in the MVP.

## 4. Buddy Execution

Discord messages must use the existing Buddy execution pipeline.

Introduce channel-aware request metadata:

```
pub enum RequestChannel {
    Desktop,
    Discord,
    WhatsApp,
}

pub struct AgentRequest {
    pub request_id: String,
    pub channel: RequestChannel,
    pub user_id: String,
    pub conversation_id: String,
    pub message: String,
}
```

Refactor existing execution entry points only where necessary.

Requirements:

- Shared intelligence and model routing.
- Shared tools and task engine.
- Independent conversation contexts.
- Request-scoped cancellation.
- Structured results for remote delivery.
- Safe concurrent desktop and Discord requests.
- No shared-state corruption.
- No blocking the UI during remote requests.

Do not directly call desktop UI functions from Discord event handlers.

## 5. Remote Security and Permissions

Extend `agent/policy.rs` with channel-aware authorization.

Discord must never bypass existing Buddy safety restrictions.

### Automatically allowed

- General AI conversations
- Read-only system metrics
- Non-sensitive coding-agent status
- Safe task summaries

### Require explicit approval

- Shell command execution
- File modifications and deletion
- Git commits, pushes, or repository changes
- Sending messages to third parties
- Installing applications
- Desktop automation
- Shutdown or restart operations

### Denied by default

- Credential extraction
- Disabling security protections
- Unauthorized privileged operations
- Requests from unpaired identities

### Discord approval buttons

Use Discord message components for sensitive-action approval.

Each approval must:

- Be associated with the exact pending action and arguments.
- Verify the interacting Discord user ID.
- Expire automatically.
- Be single-use.
- Be persisted safely across reconnects.
- Be audited.
- Support rejection and cancellation.

Do not interpret a generic conversational "yes" as unrestricted authorization.

## 6. Persistent Conversations

Reuse existing SQLite-backed memory and history.

Implement conversation isolation using stable identifiers such as:

```
desktop:<session-id>
discord:<user-id>:<channel-id>
whatsapp:<account-id>:<chat-id>
```

Requirements:

- Preserve conversations across application restarts.
- Prevent Discord and desktop conversations from overwriting each other.
- Allow shared long-term memory only according to identity and access policy.
- Associate tasks with their originating channel.
- Return asynchronous task results to the correct conversation.
- Deduplicate incoming Discord events.

Do not create a second memory database.

## 7. Proactive Notifications

Integrate Discord with existing Buddy and coding-agent events.

Supported notification categories:

- Coding agent finished
- Coding agent needs input
- Build succeeded or failed
- Task completed or failed
- Important system alerts
- Approval required

Implement a centralized notification dispatcher.

Users should eventually be able to select:

- Desktop notification
- Dynamic Island
- Discord DM
- WhatsApp
- Multiple channels

Avoid sending sensitive file contents, credentials, or private conversations in automatic notifications.

Use a bounded delivery queue with retries and failure reporting.

## 8. Native Zor UI Settings

Add a Discord integration section to Buddy settings.

Follow the existing native Rust rendering system, spacing tokens, and visual hierarchy.

### Connection

- Enable Discord integration
- Configure bot token
- Connect/disconnect
- Connection status
- Last connection error

### Authorization

- Pair Discord account
- View authorized account
- Revoke access
- Configure remote permissions

Prefer a one-time pairing challenge that verifies the user's Discord identity rather than relying on a manually entered username.

### Notifications

- Enable proactive notifications
- Choose notification categories
- Test notification delivery

Do not introduce a web-based settings interface.

## 9. Resource Efficiency

Optimize for minimal overhead.

- Reuse the existing Tokio runtime where practical.
- Keep one Discord Gateway connection.
- Avoid busy polling.
- Avoid unnecessary background threads.
- Bound message queues and concurrent agent requests.
- Do not load AI models merely because Discord is connected.
- Release resources when disabled.
- Keep Discord-specific dependencies behind a Cargo feature where appropriate.

Measure idle memory and CPU overhead before and after enabling Discord.

Investigate Game Mode behavior and document whether remote connectivity remains active when the shell UI is suspended.

## 10. Implementation Phases

### Phase 1 — Foundation

- Audit the existing Buddy architecture.
- Implement the shared messaging interfaces.
- Introduce channel-aware requests.
- Implement identity verification and permissions.

### Phase 2 — Discord MVP

- Integrate Serenity.
- Connect to Discord Gateway.
- Implement account pairing.
- Receive DMs.
- Route messages to Buddy.
- Send text responses.
- Persist conversations.

**Milestone:** Send "What's my CPU usage?" from Discord and receive the correct result from the existing Buddy.

### Phase 3 — Remote Agent Features

- Coding-agent task status
- Build notifications
- Task progress updates
- Remote approval buttons
- Cancellation and task management

### Phase 4 — Production Hardening

- Reconnection and error recovery
- Deduplication and durable delivery
- Token protection
- Permission auditing
- Resource optimization
- Comprehensive tests

### Phase 5 — Multi-Platform Support

Reuse the messaging infrastructure for `whatsapp-rust`.

Do not rewrite Buddy intelligence or create separate execution systems for different messaging platforms.

## 11. Testing

Test:

- Valid and invalid bot tokens
- Authorized and unauthorized Discord accounts
- First-time pairing and revocation
- Concurrent desktop and Discord requests
- Duplicate events
- Network disconnects
- Application restarts
- Long-running tasks
- Approval expiration and replay attempts
- Discord API rate limits
- Game Mode behavior
- Clean shutdown
- Memory and CPU overhead

Run existing Rust tests and build checks after each implementation phase.

## Deliverables

1. Working Discord integration using Serenity.
2. Secure account pairing.
3. Natural-language Buddy conversations through Discord DMs.
4. Shared agent execution and persistent history.
5. Remote permissions and approval controls.
6. Proactive notifications.
7. Native settings interface.
8. Automated tests and build verification.
9. Documentation covering setup, security, and limitations.
10. Architecture ready for the future WhatsApp integration.

## Implementation Constraints

Do not perform a large-scale rewrite of Zor UI.

Prefer small, reviewable changes that reuse existing architecture.

Follow CLAUDE.md, preserve the native UI design system, and keep performance overhead minimal.

Verify the actual Serenity APIs and dependency compatibility before implementation.

Do not claim completion until the integration has been tested with a real Discord bot and a paired account.

**End goal:** Discord becomes a secure, lightweight, native remote interface to Zor Buddy, sharing the same intelligence, memory, and execution capabilities as the desktop assistant.