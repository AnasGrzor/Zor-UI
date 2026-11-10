//! Provider registry: providers register by id, discovery lists them, and switching the active one is
//! transactional. The old provider stays active unless every step of the new one passes.

use super::contract::{Cancel, ProviderCapabilities, ProviderError, SharedProvider};

#[derive(Debug, PartialEq)]
pub enum SwitchError {
    UnknownProvider(String),
    /// The new provider lacks a capability the caller needs (its name).
    MissingCapability(&'static str),
    /// The provider has no voice with this id, so the binding cannot be resolved.
    NoVoice(String),
    Health(ProviderError),
    /// The test preview failed; the old provider stays active.
    Preview(ProviderError),
}

#[derive(Default)]
pub struct Registry {
    providers: Vec<SharedProvider>,
    active: Option<SharedProvider>,
}

impl Registry {
    pub fn new() -> Registry { Registry::default() }

    /// Adds a provider. A second one with the same id is refused, so a plugin cannot replace a built-in.
    pub fn register(&mut self, p: SharedProvider) -> Result<(), String> {
        if self.providers.iter().any(|q| q.id() == p.id()) {
            return Err(format!("a provider with id '{}' is already registered", p.id()));
        }
        self.providers.push(p);
        Ok(())
    }

    /// Ids of every registered provider, in registration order.
    pub fn ids(&self) -> Vec<&'static str> { self.providers.iter().map(|p| p.id()).collect() }

    pub fn get(&self, id: &str) -> Option<SharedProvider> { self.providers.iter().find(|p| p.id() == id).cloned() }

    pub fn active(&self) -> Option<SharedProvider> { self.active.clone() }

    /// Activates the built-in fallback at start-up without the switch checks, so speech works even before
    /// anything else is set up. Returns false when `id` is not registered.
    pub fn activate_builtin(&mut self, id: &str) -> bool {
        match self.get(id) {
            Some(p) => { self.active = Some(p); true }
            None => false,
        }
    }

    /// Moves to provider `id`: checks it exists, has `need`, has `voice`, passes `health`, and speaks a test
    /// preview. Only then does it become active.
    pub fn switch(&mut self, id: &str, voice: &str, need: &ProviderCapabilities) -> Result<(), SwitchError> {
        let next = self.get(id).ok_or_else(|| SwitchError::UnknownProvider(id.into()))?;
        if let Some(missing) = next.capabilities().missing(need) {
            return Err(SwitchError::MissingCapability(missing));
        }
        let voices = next.voices().map_err(SwitchError::Health)?;
        if !voices.iter().any(|v| v.id == voice) {
            return Err(SwitchError::NoVoice(voice.into()));
        }
        next.health().map_err(SwitchError::Health)?;
        next.preview(voice, &Default::default(), &Cancel::new()).map_err(SwitchError::Preview)?;
        if let Some(old) = self.active.replace(next) {
            if old.id() != id { old.shutdown(); }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::voice::mock::MockTts;
    use std::sync::Arc;

    fn reg(providers: Vec<MockTts>) -> Registry {
        let mut r = Registry::new();
        for p in providers { r.register(Arc::new(p)).unwrap(); }
        r
    }

    #[test]
    fn registers_and_lists_in_order_and_refuses_duplicates() {
        let mut r = reg(vec![MockTts::new("a"), MockTts::new("b")]);
        assert_eq!(r.ids(), vec!["a", "b"]);
        assert!(r.register(Arc::new(MockTts::new("a"))).is_err(), "same id twice is refused");
        assert_eq!(r.ids(), vec!["a", "b"], "the refused provider is not added");
    }

    #[test]
    fn switch_activates_a_healthy_provider() {
        let mut r = reg(vec![MockTts::new("a"), MockTts::new("b")]);
        r.switch("a", "v1", &Default::default()).unwrap();
        assert_eq!(r.active().unwrap().id(), "a");
        r.switch("b", "v1", &Default::default()).unwrap();
        assert_eq!(r.active().unwrap().id(), "b", "switching at runtime works");
    }

    #[test]
    fn failed_health_or_preview_keeps_the_old_provider() {
        let mut r = reg(vec![MockTts::new("a"), MockTts::new("bad_health").failing_health(), MockTts::new("bad_preview").failing_preview()]);
        r.switch("a", "v1", &Default::default()).unwrap();
        assert!(matches!(r.switch("bad_health", "v1", &Default::default()), Err(SwitchError::Health(_))));
        assert!(matches!(r.switch("bad_preview", "v1", &Default::default()), Err(SwitchError::Preview(_))));
        assert_eq!(r.active().unwrap().id(), "a", "no half-switched state");
    }

    #[test]
    fn unknown_provider_missing_voice_and_missing_capability_are_refused() {
        let mut r = reg(vec![MockTts::new("a")]);
        assert_eq!(r.switch("nope", "v1", &Default::default()), Err(SwitchError::UnknownProvider("nope".into())));
        assert_eq!(r.switch("a", "ghost", &Default::default()), Err(SwitchError::NoVoice("ghost".into())));
        let need = ProviderCapabilities { cloning: true, ..Default::default() };
        assert_eq!(r.switch("a", "v1", &need), Err(SwitchError::MissingCapability("voice cloning")));
        assert!(r.active().is_none(), "nothing activated by any refused switch");
    }
}
