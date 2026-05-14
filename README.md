# wake-nudge

Tiny Rust app that exposes a local Start/Stop button for periodically nudging
the mouse cursor to keep a local desktop session active.

The app does not click, type, or hide itself. By default it does not run
activity until you press Start. It runs in the foreground until stopped with
`Ctrl-C`.

## Build

```sh
cargo build --release
```

## Run

```sh
cargo run -- --interval 30 --port 7878
```

Then open:

```text
http://127.0.0.1:7878
```

Use the button on that page to turn the activity simulation on or off.

## Run As A macOS Agent

Build the release binary first:

```sh
cargo build --release
```

Install and start the user LaunchAgent:

```sh
./target/release/wake-nudge --install-agent --start-enabled --interval 30 --port 7878
```

The app will keep running in your macOS login session and will start again when
you log in. With `--start-enabled`, it begins active immediately. Open the same
local button page to turn simulation on or off:

```text
http://127.0.0.1:7878
```

Agent commands:

```sh
./target/release/wake-nudge --agent-status
./target/release/wake-nudge --stop-agent
./target/release/wake-nudge --start-agent
./target/release/wake-nudge --uninstall-agent
```

## GitHub Releases

Every pushed tag that starts with `v` builds a macOS release binary and attaches
it to a GitHub Release.

Create a release:

```sh
git tag v0.1.0
git push origin v0.1.0
```

After the workflow finishes, download:

```text
wake-nudge-aarch64-apple-darwin.tar.gz
```

Options:

```text
-i, --interval <seconds>  Seconds between nudges (default: 30)
-p, --port <port>         Local web control port (default: 7878)
    --once                Nudge once and exit
    --start-enabled       Start with simulation already on
    --dry-run             Print what would happen without starting the server
    --install-agent       Install and start a macOS LaunchAgent
    --uninstall-agent     Stop and remove the macOS LaunchAgent
    --start-agent         Start the installed macOS LaunchAgent
    --stop-agent          Stop the installed macOS LaunchAgent
    --agent-status        Print macOS LaunchAgent status
-h, --help                Show help
```

## Platform

Cursor nudging is currently implemented for macOS using CoreGraphics. While
enabled, wake-nudge also holds a macOS idle-display-sleep assertion through
IOKit. Other platforms return a clear unsupported-platform error.

If your Mac is managed by company policy, macOS may still force a lock even when
the app is running.
