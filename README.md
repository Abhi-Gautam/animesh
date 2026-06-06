# animesh

<div align="center">

![animesh demo](examples/images/example1.png)

[![Crates.io](https://img.shields.io/crates/v/animesh)](https://crates.io/crates/animesh)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

**The terminal anime tracker for people who never leave it.**

[Installation](#installation) • [Usage](#usage) • [Status](#status)

</div>

## What it is

1. You follow the shows you care about. animesh remembers them.
2. When a new episode drops, you get a desktop notification — no phone, no browser.
3. One command shows you what aired today, what's airing tomorrow, what's late.
4. Another shows you what's next in your backlog when you've got an hour free.
5. Click into any show and animesh tells you where to stream it — Crunchyroll, Netflix, wherever it lives.
6. Add, mark watched, snooze, drop — all keyboard, all from the terminal, all instant.
7. Your list lives on your machine. No login, no account, no risk of losing it if a service dies.
8. Pop it open as a tmux overlay or run it standalone. It feels native either way.
9. Anime first — but the core works for manga, podcasts, F1, anything that drops on a schedule.
10. Tiny, fast, boring in the best way. Built to still be useful in ten years.

## Status

Early. Today `animesh schedule` works against AniList — see [Usage](#usage) below. The rest of the product described above is the direction we're building toward.

## 🚀 Installation

### Using Cargo (Recommended)

```bash
cargo install animesh
```

### Using Release Assets

1. Visit the [Releases](https://github.com/Abhi-Gautam/animesh/releases) page
2. Download the appropriate asset for your platform:
   - Windows: `animesh-windows.zip`
   - macOS: `animesh-macos.tar.gz`
   - Linux: `animesh-linux.tar.gz`
3. Extract the archive
4. Add the binary to your PATH:

#### Windows
```powershell
# Add to PATH for current user
$env:Path += ";C:\path\to\extracted\folder"
# Or add to system PATH through System Properties > Environment Variables
```

#### macOS/Linux
```bash
# Move binary to a directory in your PATH
sudo mv animesh /usr/local/bin/
# Or add to PATH in your shell config (~/.bashrc, ~/.zshrc, etc.)
export PATH="$PATH:/path/to/extracted/folder"
```

## 📖 Usage

### View Schedule

```bash
# Show schedule for next 1 day (default)
animesh schedule

# Show schedule for next 7 days
animesh schedule --interval 7

# Show schedule in a specific timezone
animesh schedule --timezone "IST"

# Show past episodes from last 3 days
animesh schedule --interval 3 --past
```

![Schedule Command Output](examples/images/example2.png)

## 🌍 Timezone Support

The tool supports various timezone formats:
- Standard timezone names (e.g., "UTC", "IST", "JST")
- UTC offsets (e.g., "UTC+5:30", "UTC-4:00")
- IANA/Olson timezone database names (e.g., "America/New_York", "Europe/London")

If no timezone is specified, the tool will try to fallback to your current time zone.

## 🤝 Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## 📝 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## 🙏 Acknowledgments

- [AniList](https://anilist.co/) for their amazing API
- [chrono-tz](https://github.com/chronotope/chrono-tz) for timezone support
- All the contributors who help improve this project