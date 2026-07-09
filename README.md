# cmux-sidebar-ports

`cmux-sidebar-ports` is a sidebar plugin for the cmux terminal multiplexer. It shows TCP ports listening on the local machine together with their process names and PIDs. Select a port and press `Enter` to open `http://localhost:<port>` in a browser tab in the currently focused cmux pane.

The list refreshes every two seconds. Ports that appeared since the previous refresh are highlighted for one refresh tick, and common development ports from 3000 through 9999 receive a subtle highlight.

## Requirements

The plugin reads `lsof -nP -iTCP -sTCP:LISTEN`; `lsof` must be available on macOS or Linux. If it is missing, the sidebar displays an installation hint and keeps running so you can retry with `r`.

## Keys

- `Up` / `Down`, `Ctrl-K` / `Ctrl-J`: move the selection
- `Enter`: open the selected port in a cmux browser tab
- `k`: ask for confirmation, then send `SIGTERM` to the owning process
- `r`: refresh immediately
- `Ctrl-C`: exit cleanly
- `Esc`: never exits; cmux owns the focus escape chord

## Sidebar Plugin Contract

This plugin follows the ordinary-terminal sidebar contract documented by the [cmux-sidebar-fzf reference plugin](https://github.com/manaflow-ai/cmux-sidebar-fzf#sidebar-plugin-contract). It reads `CMUX_TUI_SOCKET` first and accepts the legacy `CMUX_MUX_SOCKET` fallback, runs in the sidebar PTY, responds to terminal resize events, and reconnects with backoff when the socket is unavailable.

At activation time, the plugin fetches the current cmux tree and targets the currently focused pane. If the connected server has no browser support, the error is shown in the sidebar status line and the plugin continues running.

## Standalone Development

Run cmux, find its JSON-lines socket path, and pass it to the plugin:

```sh
CMUX_TUI_SOCKET=/path/to/cmux-tui.sock cargo run
```

Socket paths commonly follow this convention:

```sh
ls "${TMPDIR:-/tmp}"/cmux-tui-*/*.sock
```

Running without a socket environment variable is supported and renders a helpful reconnect screen instead of panicking.

## Install With cmux

```sh
cmux-tui plugin install https://github.com/manaflow-ai/cmux-sidebar-ports
```

## Build and Test

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo build --release
```
