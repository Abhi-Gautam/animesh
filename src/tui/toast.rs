//! Bottom-center toast queue. Auto-dismiss ~2.2s after the most
//! recent toast was pushed.

use std::time::Instant;

const TOAST_TTL_MS: u128 = 2200;

#[derive(Debug, Clone)]
pub struct Toast {
    pub text: String,
    pub at: Instant,
}

#[derive(Debug, Default)]
pub struct ToastQueue {
    current: Option<Toast>,
}

impl ToastQueue {
    pub fn push(&mut self, text: impl Into<String>) {
        self.current = Some(Toast {
            text: text.into(),
            at: Instant::now(),
        });
    }

    /// Returns Some(text) if a toast is currently visible. Pure
    /// read — the next push() will overwrite a stale entry naturally,
    /// so we don't need to mutate on visibility checks.
    pub fn visible(&self) -> Option<&str> {
        let t = self.current.as_ref()?;
        if t.at.elapsed().as_millis() < TOAST_TTL_MS {
            Some(t.text.as_str())
        } else {
            None
        }
    }
}
