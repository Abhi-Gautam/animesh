//! Help overlay content — the keymap surfaced to the user via `?`.

pub(crate) const HELP_PAIRS: &[(&str, &str)] = &[
    ("j / k / ↓ / ↑", "Move selection"),
    ("Tab / Shift-Tab", "Cycle focused panel"),
    ("1 / 2 / 3", "Jump to Playable / Dropping / Following"),
    ("h / l / ← / →", "Switch focused panel"),
    ("w", "Mark watched (+1)"),
    ("c", "Copy LLM context for selection"),
    ("d", "Drop show"),
    ("g", "Open where to watch (prefers subs)"),
    (":", "Command mode  (try :watched, :sync)"),
    (":subs", "Manage streamer subscriptions (add/remove/list)"),
    (":theme", "Choose a Catppuccin theme"),
    ("t", "Open theme picker"),
    (":context", "Alias of c — copy LLM context"),
    ("/", "Jump to a followed show by fuzzy title"),
    ("a", "Discover and follow a new release"),
    ("?", "This help"),
    ("Esc", "Close overlay"),
    ("q / Ctrl-C", "Quit"),
];
