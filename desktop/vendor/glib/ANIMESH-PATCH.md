# GLib security backport

Source: crates.io glib 0.18.5 (MIT), retained with its license and upstream source.
The functional source change is the fix from https://github.com/gtk-rs/gtk-rs-core/pull/1343:
VariantStrIter::impl_get passes a mutable out-pointer to g_variant_get_child.
This fixes RUSTSEC-2024-0429 without mixing incompatible GLib versions in GTK3.
The desktop manifest pins this source; scripts/check-security.py verifies it.
Remove this backport when Tauri's GTK dependency supports a fixed upstream version.

The library root also allows the compiler style lints `unused_parens` and `mismatched_lifetime_syntaxes` introduced/enabled since this upstream release. Application lints remain strict.

The test-only `trybuild2` dependency aliases maintained `trybuild`, whose compatible TestCases API avoids the abandoned `basic-toml` dependency. This does not affect the shipped dependency graph. Its separate test lockfile is included in the advisory gate.

The crates.io package SHA-256 is `233daaf6e83ae6a12a52055f568f9d7cf4671dabb78ff9560ab6da230ce00ee5`. The advisory gate restores the registry identity in its temporary scan input so future advisories still apply to this local dependency. Only the explicitly backported advisory is accepted, after checking the patched files.
