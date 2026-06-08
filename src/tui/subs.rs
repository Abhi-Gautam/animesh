//! Subscription state — thin wrapper over `Library` kv-backed list.

use std::sync::Arc;

use anyhow::Result;

use crate::library::Library;

#[derive(Debug, Clone, Default)]
pub struct Subs {
    streamers: Vec<String>,
}

impl Subs {
    pub fn load(lib: &Library) -> Result<Self> {
        Ok(Self {
            streamers: lib.subscribed_streamers()?,
        })
    }

    pub fn streamers(&self) -> &[String] {
        &self.streamers
    }

    /// Case-insensitive substring-equality match.
    pub fn matches(&self, streamer: &str) -> bool {
        let needle = streamer.to_ascii_lowercase();
        self.streamers.iter().any(|s| s.to_ascii_lowercase() == needle)
    }

    pub fn add(&mut self, lib: &Library, streamer: &str) -> Result<bool> {
        let needle = streamer.trim();
        if needle.is_empty() || self.matches(needle) {
            return Ok(false);
        }
        self.streamers.push(needle.to_string());
        lib.set_subscribed_streamers(&self.streamers)?;
        Ok(true)
    }

    pub fn remove(&mut self, lib: &Library, streamer: &str) -> Result<bool> {
        let needle = streamer.trim().to_ascii_lowercase();
        let before = self.streamers.len();
        self.streamers
            .retain(|s| s.to_ascii_lowercase() != needle);
        if self.streamers.len() == before {
            return Ok(false);
        }
        lib.set_subscribed_streamers(&self.streamers)?;
        Ok(true)
    }
}

impl Subs {
    pub fn load_arc(lib: &Arc<Library>) -> Result<Self> {
        Self::load(lib.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::FixedClock;

    fn lib() -> Library {
        Library::open_in_memory(Arc::new(FixedClock(1))).unwrap()
    }

    #[test]
    fn add_persists_and_matches_case_insensitive() {
        let lib = lib();
        let mut s = Subs::load(&lib).unwrap();
        assert!(s.add(&lib, "Netflix").unwrap());
        assert!(s.matches("netflix"));
        assert!(s.matches("NETFLIX"));
        let reloaded = Subs::load(&lib).unwrap();
        assert!(reloaded.matches("netflix"));
    }

    #[test]
    fn add_is_idempotent() {
        let lib = lib();
        let mut s = Subs::load(&lib).unwrap();
        assert!(s.add(&lib, "Netflix").unwrap());
        assert!(!s.add(&lib, "netflix").unwrap());
        assert_eq!(s.streamers().len(), 1);
    }

    #[test]
    fn remove_drops_and_persists() {
        let lib = lib();
        let mut s = Subs::load(&lib).unwrap();
        s.add(&lib, "Netflix").unwrap();
        s.add(&lib, "Crunchyroll").unwrap();
        assert!(s.remove(&lib, "netflix").unwrap());
        assert!(!s.matches("netflix"));
        assert!(s.matches("crunchyroll"));
        let reloaded = Subs::load(&lib).unwrap();
        assert!(reloaded.matches("crunchyroll"));
        assert!(!reloaded.matches("netflix"));
    }
}
