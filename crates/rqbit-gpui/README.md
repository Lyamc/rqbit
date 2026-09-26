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

## Browser build (WebAssembly)

The same UI code also compiles to `wasm32-unknown-unknown` and runs in a
browser on GPUI's web platform (`gpui_web`: canvas + WebGPU, falling back to
WebGL2; the browser `fetch` API for HTTP). It lives in `web/`, a separate
cargo workspace because it needs gpui from zed's git repository (the crates.io
release has no web platform). The UI code differs only behind the `gpui-main`
feature (one API signature changed) and `cfg(target_family = "wasm")`.

    cargo install wasm-bindgen-cli --version 0.2.120   # must match Cargo.lock
    crates/rqbit-gpui/web/build.sh                     # -> web/dist/

The toolchain comes from `web/rust-toolchain.toml` (1.98.1, same as zed).
Single threaded: no SharedArrayBuffer, so no COOP/COEP headers or nightly
`build-std` are needed.

rqbit serves the result at `/gpui/` from `$RQBIT_GPUI_WEB_DIR`, default
`$XDG_DATA_HOME/rqbit/gpui-web` (`~/.local/share/rqbit/gpui-web`):

    mkdir -p ~/.local/share/rqbit/gpui-web
    cp crates/rqbit-gpui/web/dist/* ~/.local/share/rqbit/gpui-web/

It talks to the API of the origin it is served from (same origin, no CORS;
basic auth, if enabled, is the browser's). The `.gz` files are served
precompressed when the browser accepts gzip. The UI font (IBM Plex Sans,
SIL OFL 1.1) is bundled because browsers don't expose system fonts to the
canvas renderer.

## Layout

- `src/api/` – HTTP client (`native.rs`: reqwest, `web.rs`: fetch via GPUI's
  `HttpClient`) and serde types mirroring
  `crates/librqbit/webui/src/api-types.ts`. Add new endpoints here.
- `src/format.rs` – formatting shared by views (matches the web UI).
- `src/ui/` – GPUI views. `mod.rs` is the root window; add new web UI features
  as new modules (details pane, add torrent, events, settings, ...).

API calls return futures that run on GPUI's background executor: natively
`reqwest::blocking` (GPUI has its own executor, no tokio runtime is started for
the GUI), in the browser `fetch`.
