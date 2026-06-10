//! Command registry — the spine that unifies keymap and `:` palette.
//!
//! Pressing `w` and typing `:watched` both resolve to `Command::Watched`
//! and flow through `App::dispatch`. This is the lazygit/nvim pattern:
//! one canonical action, multiple input surfaces.

use nucleo::{
    pattern::{CaseMatching, Normalization, Pattern},
    Config, Matcher,
};

/// Every user-invocable verb. New verbs land here and gain a keymap
/// + palette entry in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Watched,
    Drop,
    Stream,
    Sync,
    Doctor,
    Help,
    Quit,
    /// `:follow 12345` — numeric AniList id only for now.
    Follow(i64),
    /// `c` / `:context` — copy LLM context for current selection.
    CopyContext,
    /// `:subs add <name>` — subscribe to a streamer.
    SubsAdd(String),
    /// `:subs remove <name>` — unsubscribe from a streamer.
    SubsRemove(String),
    /// `:subs` — toast the current subscription list.
    SubsList,
    /// `:theme [id]` — open picker or apply a theme directly.
    Theme(Option<String>),
}

/// Static metadata for the palette. `name` is the canonical spelling
/// the user types; `aliases` are alternatives. `arg_hint` shows after
/// the name in the palette when the verb takes an argument.
#[derive(Debug, Clone, Copy)]
pub struct CommandSpec {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub description: &'static str,
    pub arg_hint: Option<&'static str>,
}

/// The catalogue. Order matters: this is the default ranking when the
/// query is empty.
pub const SPECS: &[CommandSpec] = &[
    CommandSpec {
        name: "watched",
        aliases: &["w", "mark", "seen"],
        description: "Mark current episode watched",
        arg_hint: None,
    },
    CommandSpec {
        name: "drop",
        aliases: &["d"],
        description: "Drop current show from library",
        arg_hint: None,
    },
    CommandSpec {
        name: "stream",
        aliases: &["g", "open", "where"],
        description: "Open streaming page in browser",
        arg_hint: None,
    },
    CommandSpec {
        name: "follow",
        aliases: &["add", "track"],
        description: "Follow a show by AniList id",
        arg_hint: Some("<anilist-id>"),
    },
    CommandSpec {
        name: "sync",
        aliases: &["refresh"],
        description: "Refresh all cached metadata from AniList",
        arg_hint: None,
    },
    CommandSpec {
        name: "doctor",
        aliases: &["status"],
        description: "Show DB path, counts, cache health",
        arg_hint: None,
    },
    CommandSpec {
        name: "theme",
        aliases: &["colorscheme", "color", "scheme"],
        description: "Choose the UI theme",
        arg_hint: Some("[theme-id]"),
    },
    CommandSpec {
        name: "help",
        aliases: &["?"],
        description: "Open the keymap overlay",
        arg_hint: None,
    },
    CommandSpec {
        name: "quit",
        aliases: &["q", "exit"],
        description: "Quit animesh",
        arg_hint: None,
    },
    CommandSpec {
        name: "context",
        aliases: &["c", "copy"],
        description: "Copy LLM context for selection",
        arg_hint: None,
    },
    CommandSpec {
        name: "subs",
        aliases: &[],
        description: "List or modify streamer subs",
        arg_hint: Some("[add|remove] <name>"),
    },
];

/// Parse error from a palette query.
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    /// Empty after trimming.
    Empty,
    /// First token doesn't match any verb name or alias.
    UnknownVerb(String),
    /// Verb requires an argument and none was given.
    MissingArg {
        verb: &'static str,
        hint: &'static str,
    },
    /// Verb takes an argument but the user supplied the wrong shape.
    BadArg { verb: &'static str, reason: String },
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::Empty => write!(f, "type a command"),
            ParseError::UnknownVerb(v) => write!(f, "unknown command: {v}"),
            ParseError::MissingArg { verb, hint } => {
                write!(f, ":{verb} needs {hint}")
            }
            ParseError::BadArg { verb, reason } => write!(f, ":{verb}: {reason}"),
        }
    }
}

/// Resolve a typed query (without the leading `:`) into a `Command`.
///
/// Examples:
/// - `"watched"` → `Watched`
/// - `"w"` → `Watched` (alias)
/// - `"follow 21"` → `Follow(21)`
/// - `"q"` → `Quit`
pub fn parse(query: &str) -> Result<Command, ParseError> {
    let trimmed = query.trim();
    if trimmed.is_empty() {
        return Err(ParseError::Empty);
    }
    let mut parts = trimmed.splitn(2, char::is_whitespace);
    let verb_token = parts.next().unwrap();
    let rest = parts.next().map(str::trim).unwrap_or("");

    let spec = SPECS
        .iter()
        .find(|s| {
            s.name.eq_ignore_ascii_case(verb_token)
                || s.aliases.iter().any(|a| a.eq_ignore_ascii_case(verb_token))
        })
        .ok_or_else(|| ParseError::UnknownVerb(verb_token.to_string()))?;

    match spec.name {
        "watched" => Ok(Command::Watched),
        "drop" => Ok(Command::Drop),
        "stream" => Ok(Command::Stream),
        "sync" => Ok(Command::Sync),
        "doctor" => Ok(Command::Doctor),
        "help" => Ok(Command::Help),
        "theme" => Ok(Command::Theme(if rest.is_empty() {
            None
        } else {
            Some(rest.to_string())
        })),
        "quit" => Ok(Command::Quit),
        "context" => Ok(Command::CopyContext),
        "subs" => {
            if rest.is_empty() {
                return Ok(Command::SubsList);
            }
            let mut it = rest.splitn(2, char::is_whitespace);
            match (it.next(), it.next()) {
                (Some("add"), Some(name)) if !name.trim().is_empty() => {
                    Ok(Command::SubsAdd(name.trim().to_string()))
                }
                (Some("remove"), Some(name)) if !name.trim().is_empty() => {
                    Ok(Command::SubsRemove(name.trim().to_string()))
                }
                _ => Err(ParseError::BadArg {
                    verb: "subs",
                    reason: "usage: :subs [add|remove] <name>".to_string(),
                }),
            }
        }
        "follow" => {
            if rest.is_empty() {
                return Err(ParseError::MissingArg {
                    verb: "follow",
                    hint: spec.arg_hint.unwrap_or("<arg>"),
                });
            }
            let id: i64 = rest.parse().map_err(|_| ParseError::BadArg {
                verb: "follow",
                reason: format!("'{rest}' is not a numeric AniList id"),
            })?;
            Ok(Command::Follow(id))
        }
        _ => unreachable!("spec name not handled: {}", spec.name),
    }
}

/// A scored match for the palette dropdown.
#[derive(Debug, Clone)]
pub struct Suggestion {
    pub spec: &'static CommandSpec,
    pub score: u32,
}

/// Rank specs by nucleo fuzzy score against `query`. Empty query
/// returns the catalogue in its declared order (score = 0).
pub fn suggest(query: &str) -> Vec<Suggestion> {
    let q = query.trim();
    if q.is_empty() {
        return SPECS
            .iter()
            .map(|spec| Suggestion { spec, score: 0 })
            .collect();
    }

    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(q, CaseMatching::Ignore, Normalization::Smart);

    let mut out: Vec<Suggestion> = Vec::new();
    for spec in SPECS {
        // Match against the name; aliases as fallback haystacks so
        // `:s` ranks `snooze` (alias match) but loses to a name-match
        // hit if one exists.
        let name_score = pattern.score(nucleo::Utf32Str::Ascii(spec.name.as_bytes()), &mut matcher);
        let alias_score = spec
            .aliases
            .iter()
            .filter_map(|a| pattern.score(nucleo::Utf32Str::Ascii(a.as_bytes()), &mut matcher))
            .max();
        let score = match (name_score, alias_score) {
            (Some(n), Some(a)) => Some(n.max(a)),
            (Some(n), None) => Some(n),
            (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        if let Some(s) = score {
            out.push(Suggestion { spec, score: s });
        }
    }
    out.sort_by(|a, b| b.score.cmp(&a.score));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_empty_is_error() {
        assert_eq!(parse(""), Err(ParseError::Empty));
        assert_eq!(parse("   "), Err(ParseError::Empty));
    }

    #[test]
    fn parse_unknown_verb() {
        match parse("nope") {
            Err(ParseError::UnknownVerb(v)) => assert_eq!(v, "nope"),
            other => panic!("expected UnknownVerb, got {other:?}"),
        }
    }

    #[test]
    fn parse_canonical_names() {
        assert_eq!(parse("watched").unwrap(), Command::Watched);
        assert_eq!(parse("drop").unwrap(), Command::Drop);
        assert_eq!(parse("stream").unwrap(), Command::Stream);
        assert_eq!(parse("sync").unwrap(), Command::Sync);
        assert_eq!(parse("doctor").unwrap(), Command::Doctor);
        assert_eq!(parse("help").unwrap(), Command::Help);
        assert_eq!(parse("quit").unwrap(), Command::Quit);
    }

    #[test]
    fn parse_resolves_aliases() {
        assert_eq!(parse("w").unwrap(), Command::Watched);
        assert_eq!(parse("seen").unwrap(), Command::Watched);
        assert_eq!(parse("q").unwrap(), Command::Quit);
        assert_eq!(parse("exit").unwrap(), Command::Quit);
        assert_eq!(parse("refresh").unwrap(), Command::Sync);
    }

    #[test]
    fn parse_is_case_insensitive() {
        assert_eq!(parse("WATCHED").unwrap(), Command::Watched);
        assert_eq!(parse("Quit").unwrap(), Command::Quit);
    }

    #[test]
    fn parse_follow_with_id() {
        assert_eq!(parse("follow 21").unwrap(), Command::Follow(21));
        assert_eq!(parse("add  154587").unwrap(), Command::Follow(154587));
    }

    #[test]
    fn parse_follow_missing_id() {
        assert!(matches!(
            parse("follow"),
            Err(ParseError::MissingArg { verb: "follow", .. })
        ));
    }

    #[test]
    fn parse_follow_non_numeric() {
        match parse("follow frieren") {
            Err(ParseError::BadArg {
                verb: "follow",
                reason,
            }) => {
                assert!(reason.contains("frieren"));
            }
            other => panic!("expected BadArg, got {other:?}"),
        }
    }

    #[test]
    fn suggest_empty_returns_full_catalogue() {
        let s = suggest("");
        assert_eq!(s.len(), SPECS.len());
        // Order preserved.
        assert_eq!(s[0].spec.name, "watched");
    }

    #[test]
    fn suggest_ranks_prefix_match_highest() {
        let s = suggest("wat");
        assert!(!s.is_empty(), "expected at least one match for 'wat'");
        assert_eq!(s[0].spec.name, "watched");
    }

    #[test]
    fn suggest_finds_via_alias() {
        let s = suggest("refresh");
        let names: Vec<_> = s.iter().map(|x| x.spec.name).collect();
        assert!(
            names.contains(&"sync"),
            "expected sync via alias 'refresh', got {names:?}"
        );
    }

    #[test]
    fn suggest_typo_tolerance() {
        // nucleo allows non-contiguous subsequence; 'wtchd' should
        // still find 'watched'.
        let s = suggest("wtchd");
        let names: Vec<_> = s.iter().map(|x| x.spec.name).collect();
        assert!(names.contains(&"watched"), "wtchd → watched; got {names:?}");
    }

    #[test]
    fn parses_context() {
        assert!(matches!(parse("context").unwrap(), Command::CopyContext));
    }

    #[test]
    fn parses_subs_no_arg_is_list() {
        assert!(matches!(parse("subs").unwrap(), Command::SubsList));
    }

    #[test]
    fn parses_subs_add() {
        match parse("subs add Netflix").unwrap() {
            Command::SubsAdd(s) => assert_eq!(s, "Netflix"),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_subs_remove() {
        match parse("subs remove Netflix").unwrap() {
            Command::SubsRemove(s) => assert_eq!(s, "Netflix"),
            _ => panic!(),
        }
    }

    #[test]
    fn subs_without_subcmd_arg_errors() {
        assert!(parse("subs add").is_err());
    }

    #[test]
    fn parse_error_display_messages_are_user_friendly() {
        assert_eq!(format!("{}", ParseError::Empty), "type a command");
        assert_eq!(
            format!("{}", ParseError::UnknownVerb("xyz".into())),
            "unknown command: xyz",
        );
        assert_eq!(
            format!(
                "{}",
                ParseError::MissingArg {
                    verb: "follow",
                    hint: "<anilist-id>"
                }
            ),
            ":follow needs <anilist-id>",
        );
    }
}
