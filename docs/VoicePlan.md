# Zor-UI — Provider-Agnostic Voice Platform

## Objective

Build a fully modular, provider-agnostic voice platform for Zor-UI.

The system must support:

- Built-in Buddy character voices.
- User-created custom voices.
- Reference-audio voice cloning.
- Voice customization.
- Local and cloud TTS providers.
- Runtime provider switching.
- Future providers without modifying Buddy or application business logic.

KittenTTS must NOT be the foundation of the architecture. It is just one optional provider.

## 1. Architecture

Implement the following layers:

1. Buddy integration layer.
2. Provider-independent Voice API.
3. Voice orchestration and provider registry.
4. Voice profile and library management.
5. Provider adapters.
6. Audio generation and playback pipeline.
7. Provider lifecycle and installation management.

Buddy must communicate exclusively through the Voice API.

Never call provider-specific SDKs, HTTP endpoints, Python processes or model libraries from [buddy.rs](http://buddy.rs) or [cast.rs](http://cast.rs).

## 2. Provider contract

Define an async Rust TtsProvider trait.

It should expose:

- Provider identifier and metadata.
- Capability discovery.
- Voice enumeration.
- Speech synthesis.
- Voice preview.
- Voice cloning where supported.
- Voice design where supported.
- Streaming where supported.
- Cancellation.
- Health checks.

Use optional capabilities instead of requiring every provider to implement unsupported functionality.

Create common contracts for:

- SynthesisRequest
- SynthesisResult
- VoiceProfile
- VoiceReference
- CloneVoiceRequest
- VoiceCapabilities
- ProviderCapabilities
- AudioOutput
- ProviderError

Standardize error handling.

Distinguish unsupported operations, authentication errors, rate limits, network failures, invalid audio and model-loading failures.

## 3. Provider types

Support three provider execution types:

### Native

Rust libraries or native model runtimes.

### Sidecar

Independent local executables or Python services.

### Remote

HTTP and WebSocket providers.

Implement the provider registry so new providers can register without changing the orchestrator.

Initial adapters to investigate:

- KittenTTS
- Piper
- Kokoro
- OpenAI
- ElevenLabs
- Generic OpenAI-compatible TTS endpoints

Do not assume every provider supports voice cloning.

## 4. Provider lifecycle

Implement:

- Discovery.
- Installation.
- Configuration.
- Initialization.
- Health checks.
- Shutdown.
- Removal.
- Version reporting.
- Model download and verification.

Do not download large models automatically.

Users must explicitly approve model downloads and see their sizes.

## 5. Voice profiles

Separate logical voice identity from provider-specific implementations.

A voice profile must have:

- Internal UUID.
- Name.
- Description.
- Personality metadata.
- Default speech settings.
- Reference audio reference.
- One or more provider bindings.

Provider bindings should contain:

- Provider ID.
- Provider voice ID.
- Model ID.
- Provider-specific metadata.

Keep provider-specific metadata isolated in a versioned extension object.

A profile must survive provider changes.

Do not promise exact voice migration between incompatible models.

Preserve original user-authorized reference recordings so profiles can be recreated with another provider.

## 6. Custom Voice Studio

Implement three sections:

### Voice Library

- Built-in character voices.
- Custom voices.
- Preview.
- Assign voice to Buddy.
- Rename and delete.
- Provider information.

### Create Voice

- Upload audio.
- Record using microphone.
- Validate duration and audio quality.
- Enter reference transcript.
- Choose a provider supporting voice cloning.
- Generate voice.
- Preview.
- Save or discard.

### Voice Settings

- Provider selection.
- Model selection.
- Speaking speed.
- Pitch if supported.
- Emotion.
- Expressiveness.
- Execution mode.
- Voice preview.

Controls must dynamically reflect provider capabilities.

## 7. Buddy integration

Replace the existing global gojoVoice implementation with a provider-independent Voice API.

Preserve existing Buddy dialogue behavior.

Support per-Buddy voice assignments.

Every Buddy should have a logical voice ID.

AI-generated responses and ambient dialogue must use the same speech pipeline.

Do not introduce provider-specific logic into [buddy.rs](http://buddy.rs) or [cast.rs](http://cast.rs).

## 8. Audio pipeline

Implement:

- Centralized speech queue.
- Playback interruption.
- Generation cancellation.
- Priority management.
- Audio format normalization.
- Streaming playback when supported.
- Temporary audio cleanup.
- Concurrent request protection.

Do not block the UI thread.

Normalize audio into an application-wide internal format.

## 9. Configuration and storage

Keep application configuration independent of providers.

Store:

- Provider settings.
- Voice assignments.
- Voice profiles.
- Model installation metadata.
- User preferences.

Keep credentials in secure OS credential storage.

Store large model files and reference recordings outside the application executable.

Use versioned migrations for configuration changes.

## 10. Provider switching

Implement a transactional switching workflow:

1. Validate the new provider.
2. Verify required capabilities.
3. Resolve a compatible voice binding.
4. Initialize the provider.
5. Generate a test preview.
6. Activate the provider only after successful validation.

If a voice cannot be migrated, offer recreation using its reference recording.

Never silently replace a user's voice with a generic voice.

## 11. Testing

Create mock providers to test the entire system without downloading actual models.

Test:

- Provider registration.
- Provider discovery.
- Provider switching.
- Unsupported capabilities.
- Voice assignment.
- Voice cloning workflows.
- Configuration migrations.
- Playback interruption.
- Provider failure.
- Model installation.
- Audio normalization.

## 12. Backward compatibility

Inspect the existing repository before modifying code.

Preserve the current Windows TTS implementation as a fallback adapter.

Migrate existing gojoVoice configuration without breaking existing installations.

Do not remove existing Buddy dialogue, animation or UI functionality.

## 13. Implementation approach

Build in stages:

Stage 1: Contracts, registry and mock provider.

Stage 2: Existing Windows TTS migrated into an adapter.

Stage 3: Provider lifecycle and local sidecar support.

Stage 4: One local TTS provider and one remote provider.

Stage 5: Voice Library and per-Buddy assignments.

Stage 6: Custom Voice Studio and voice cloning.

Stage 7: Provider switching, migrations, packaging and testing.

## Acceptance criteria

- Buddy works without knowing which provider is active.
- Switching providers does not require changes to Buddy code.
- Adding a provider requires implementing the provider contract and registering it.
- Unsupported features are clearly disabled.
- Custom profiles survive provider changes.
- Existing users retain their current voice behavior.
- The application works when optional providers are not installed.
- No large model is bundled with the base installer.

IMPORTANT: Inspect actual repository code and conventions before implementing. Do not invent files, functions or UI components. Deliver working, tested code rather than placeholder interfaces or architecture diagrams.