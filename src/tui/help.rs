//! Help overlay content — the keymap surfaced to the user via `?`.

pub const HELP_PAIRS: &[(&str, &str)] = &[
    ("j / k / ↓ / ↑", "Move selection"),
    ("Tab / Shift-Tab", "Cycle focused panel"),
    ("1 / 2 / 3", "Jump to Today / Late / Backlog"),
    ("h / l / ← / →", "Switch focused panel"),
    ("w", "Mark watched (+1)"),
    ("s", "Snooze to tomorrow"),
    ("d", "Drop show"),
    ("g", "Open where to watch"),
    ("a / : / /", "Command palette"),
    ("?", "This help"),
    ("Esc", "Close overlay"),
    ("q / Ctrl-C", "Quit"),
];
