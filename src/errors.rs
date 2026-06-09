//! Error classification and process-exit discipline.
//!
//! Three categories — user input, durable state, network — each with
//! its own exit code so scripts can branch on it. Spec §11.
//!
//! The contract: where a command knows it's producing a *user* or
//! *network* error, it wraps the cause in `UserError` / `NetworkError`.
//! Anything else falls through to `Durable` by default, which is the
//! safe choice: durable errors get the loudest treatment, including a
//! pointer to the DB path.

use std::fmt;

use anyhow::Error as AnyError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitKind {
    User = 1,
    Durable = 2,
    Network = 3,
}

impl ExitKind {
    pub fn code(self) -> i32 {
        self as i32
    }
}

/// Wraps a cause to mark it as a user error (bad input, no match).
#[derive(Debug)]
pub struct UserError(pub AnyError);

impl fmt::Display for UserError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl std::error::Error for UserError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

/// Wraps a cause to mark it as a transient network error.
#[derive(Debug)]
pub struct NetworkError(pub AnyError);

impl fmt::Display for NetworkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#}", self.0)
    }
}

impl std::error::Error for NetworkError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

/// Build a user error from anything that flows into anyhow::Error
/// (incl. `anyhow!`, `&str`, owned `String`, etc).
pub fn user_error<E: Into<AnyError>>(e: E) -> AnyError {
    AnyError::new(UserError(e.into()))
}

/// Build a network error. Test-only today — production code wraps
/// network failures via `reqwest::Error`, which `classify` detects
/// directly. The helper exists so tests can synthesize a sentinel
/// without depending on a real HTTP failure.
#[cfg(test)]
pub fn network_error<E: Into<AnyError>>(e: E) -> AnyError {
    AnyError::new(NetworkError(e.into()))
}

/// Walk the error chain and report the highest-priority kind we find.
/// Priority: explicit sentinels > known downstream types > Durable.
pub fn classify(err: &AnyError) -> ExitKind {
    for cause in err.chain() {
        if cause.is::<UserError>() {
            return ExitKind::User;
        }
        if cause.is::<NetworkError>() {
            return ExitKind::Network;
        }
        if cause.is::<reqwest::Error>() {
            return ExitKind::Network;
        }
    }
    ExitKind::Durable
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{anyhow, Context};

    #[test]
    fn unwrapped_anyhow_defaults_to_durable() {
        let e = anyhow!("schema corrupted");
        assert_eq!(classify(&e), ExitKind::Durable);
    }

    #[test]
    fn user_error_is_classified_user() {
        let e = user_error(anyhow!("no AniList show with id 42"));
        assert_eq!(classify(&e), ExitKind::User);
    }

    #[test]
    fn user_error_through_context_chain() {
        let inner: anyhow::Error = user_error(anyhow!("no match"));
        let outer = Err::<(), _>(inner).context("looking up follow").unwrap_err();
        assert_eq!(classify(&outer), ExitKind::User);
    }

    #[test]
    fn network_error_classified_network() {
        let e = network_error(anyhow!("rate limited"));
        assert_eq!(classify(&e), ExitKind::Network);
    }

    #[test]
    fn exit_codes_match_spec() {
        assert_eq!(ExitKind::User.code(), 1);
        assert_eq!(ExitKind::Durable.code(), 2);
        assert_eq!(ExitKind::Network.code(), 3);
    }
}
