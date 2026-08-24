//! The machine-readable output mode.
//!
//! One document on stdout, always the same shape, so an agent can parse the
//! answer without knowing which command produced it:
//!
//! ```text
//! {"ok":true,"kind":"upcoming","data":[...]}
//! {"ok":false,"error":{"code":"unavailable","message":"...","retry_after_secs":null}}
//! ```
//!
//! `kind` and `data` are lifted straight off the adjacently tagged [`Response`],
//! so the JSON surface is the IPC contract rather than a parallel one that can
//! drift from it. Exit codes are identical in both modes: JSON is a second
//! rendering, never a second protocol.

use serde_json::{Map, Value};

use crate::error::AppError;

use super::Outcome;

/// Renders a successful outcome.
pub fn success(outcome: &Outcome) -> String {
    let mut object = match outcome {
        Outcome::Replied(response) => match serde_json::to_value(response) {
            // `Response` is adjacently tagged, so this is already
            // `{"kind":..,"data":..}` and only needs `ok` added.
            Ok(Value::Object(map)) => map,
            // Unreachable for the derived impl; degrading beats panicking in a
            // mode whose whole purpose is being parseable.
            _ => tagged("unknown", Value::Null),
        },
        Outcome::Local { kind, message } => tagged(
            kind,
            Value::Object(Map::from_iter([(
                "message".to_owned(),
                Value::String(message.clone()),
            )])),
        ),
    };
    object.insert("ok".to_owned(), Value::Bool(true));
    render(Value::Object(object))
}

/// Renders a failure. The error's `code` is the stable part; `message` is prose.
pub fn failure(error: &AppError) -> String {
    let mut object = Map::new();
    object.insert("ok".to_owned(), Value::Bool(false));
    object.insert(
        "error".to_owned(),
        serde_json::to_value(error).unwrap_or(Value::Null),
    );
    render(Value::Object(object))
}

fn tagged(kind: &str, data: Value) -> Map<String, Value> {
    Map::from_iter([
        ("kind".to_owned(), Value::String(kind.to_owned())),
        ("data".to_owned(), data),
    ])
}

/// One line, so output stays greppable and streamable.
fn render(value: Value) -> String {
    serde_json::to_string(&value).unwrap_or_else(|_| {
        r#"{"ok":false,"error":{"code":"internal","message":"the reply could not be serialized"}}"#
            .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::read_models::{FollowSummary, UpcomingRelease};
    use crate::error::ErrorCode;
    use crate::ipc::protocol::Response;

    fn parse(document: &str) -> Value {
        serde_json::from_str(document).expect("output is valid json")
    }

    #[test]
    fn a_reply_carries_ok_kind_and_data() {
        let document = success(&Outcome::Replied(Response::ListFollows(Vec::new())));
        let value = parse(&document);
        assert_eq!(value["ok"], Value::Bool(true));
        assert_eq!(value["kind"], "list_follows");
        assert_eq!(value["data"], Value::Array(Vec::new()));
    }

    #[test]
    fn the_kind_is_the_ipc_reply_name() {
        // The two must not drift: an agent that learns `kind` from the protocol
        // docs has to see the same string here.
        for response in [
            Response::ListFollows(Vec::new()),
            Response::SearchAnime(Vec::new()),
            Response::Upcoming(Vec::new()),
        ] {
            let expected = response.name().to_owned();
            let value = parse(&success(&Outcome::Replied(response)));
            assert_eq!(value["kind"], expected);
        }
    }

    #[test]
    fn a_local_outcome_carries_its_message() {
        let document = success(&Outcome::Local {
            kind: "service",
            message: "Animesh is running in the background.".to_owned(),
        });
        let value = parse(&document);
        assert_eq!(value["ok"], Value::Bool(true));
        assert_eq!(value["kind"], "service");
        assert_eq!(
            value["data"]["message"],
            "Animesh is running in the background."
        );
    }

    #[test]
    fn a_failure_carries_the_stable_code() {
        let value = parse(&failure(&AppError::new(
            ErrorCode::Unavailable,
            "Animesh is not running.",
        )));
        assert_eq!(value["ok"], Value::Bool(false));
        assert_eq!(value["error"]["code"], "unavailable");
        assert_eq!(value["error"]["message"], "Animesh is not running.");
    }

    #[test]
    fn a_retryable_failure_says_how_long_to_wait() {
        let value = parse(&failure(
            &AppError::new(ErrorCode::SourceRateLimited, "rate limited").with_retry_after(60),
        ));
        assert_eq!(value["error"]["retry_after_secs"], 60);
    }

    #[test]
    fn every_document_is_a_single_line() {
        // Line-delimited output is what makes `animesh --json next | jq` and a
        // streaming agent reader both work.
        let document = success(&Outcome::Replied(Response::Upcoming(Vec::new())));
        assert!(!document.contains('\n'), "{document}");
    }

    #[test]
    fn payload_fields_survive_the_envelope() {
        // Guards against the envelope summarising instead of carrying the reply.
        let rows: Vec<UpcomingRelease> = Vec::new();
        let follows: Vec<FollowSummary> = Vec::new();
        assert_eq!(
            parse(&success(&Outcome::Replied(Response::Upcoming(rows))))["data"],
            Value::Array(Vec::new())
        );
        assert_eq!(
            parse(&success(&Outcome::Replied(Response::ListFollows(follows))))["data"],
            Value::Array(Vec::new())
        );
    }
}
