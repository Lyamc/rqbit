# rqbit-gpui

Native desktop client for rqbit built on [GPUI](https://gpui.rs) (Zed's UI
framework, crates.io `gpui = 0.2.2`). It is a pure client of the rqbit HTTP API
(the same API the web UI uses, including `status_detail`), so it can run on a
different machine than the server.

Off by default. Nothing changes in the normal server build unless the `gpui`
feature is enabled: the crate is an optional dependency of `rqbit` and is not in
the workspace `default-members`.

## What it does (first slice)

- Torrent list polled from `GET /torrents?with_stats=true` every second:
  Name, Status (server-computed detailed status + error), Size, Progress,
  ↓ Download, ↑ Upload (web UI order), Ratio (session upload ÷ bytes on disk).
- Per row: Start, Pause, Remove (`POST /torrents/{id}/forget`, files are kept;
  a confirmation dialog is shown first).
- Connection field (Enter or "Connect"). Accepts `host:port`,
  `http(s)://host:port[/prefix]`, and `http://user:pass@host:port` for basic auth.
- Clear error banner when the server is unreachable (retries every 3 s), and
  for failed actions.

## Build and run

Linux (needs pkg-config, libxkbcommon, wayland, libxcb/libX11, fontconfig,
freetype and a Vulkan driver at runtime):

    cargo build --release -p rqbit --no-default-features --features rust-tls,webui,prometheus,gpui
    target/release/rqbit gui --url http://127.0.0.1:3030

NixOS, without touching the system config:

    nix-shell crates/rqbit-gpui/shell.nix --run \
      'cargo build --release -p rqbit --no-default-features --features rust-tls,webui,prometheus,gpui'
    nix-shell crates/rqbit-gpui/shell.nix --run 'target/release/rqbit gui --url http://127.0.0.1:3030'

Windows (MSVC toolchain + Windows 10/11 SDK; release builds use `fxc.exe`
from the SDK to compile GPUI's HLSL shaders, override with `GPUI_FXC_PATH`):

    cargo build --release -p rqbit --no-default-features --features rust-tls,gpui
    target\release\rqbit.exe gui --url http://witherow:3030

Standalone client binary (does not compile the rqbit server, smaller/faster):

    cargo build --release -p rqbit-gpui --features rustls
    rqbit-gpui --url http://host:3030

`RQBIT_GUI_URL` sets the default URL; `RQBIT_GUI_LOG=info` prints GPUI logs.

## Layout

- `src/api/` – blocking HTTP client and serde types mirroring
  `crates/librqbit/webui/src/api-types.ts`. Add new endpoints here.
- `src/format.rs` – formatting shared by views (matches the web UI).
- `src/ui/` – GPUI views. `mod.rs` is the root window; add new web UI features
  as new modules (details pane, add torrent, events, settings, ...).

HTTP runs on GPUI's background executor via `reqwest::blocking` (GPUI has its
own executor, no tokio runtime is started for the GUI).
