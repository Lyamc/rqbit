[![crates.io](https://img.shields.io/crates/v/rqbit.svg)](https://crates.io/crates/rqbit)
[![crates.io](https://img.shields.io/crates/v/librqbit.svg)](https://crates.io/crates/librqbit)
[![docs.rs](https://img.shields.io/docsrs/librqbit.svg)](https://docs.rs/librqbit/latest/librqbit/)

# rqbit - bittorrent client in Rust

**rqbit** is a bittorrent client written in Rust. Has HTTP API and Web UI, and can be used as a library.

Also has a desktop app built with [Tauri](https://tauri.app/).

> **This is a fork of [ikatson/rqbit](https://github.com/ikatson/rqbit).**
> Everything upstream still works as documented below. The fork adds server features, a larger web UI and a native/WASM GPUI client; see [Fork additions](#fork-additions).
> Everything new is either off by default or changes nothing until you use it.

## Fork additions

Server settings added by the fork live in `preferences.json` (and `admin.json`, `queue.json`, `torrent-rules.json`, `events.jsonl`), next to the session persistence data. They are edited in the web UI's **Configure** dialog, the GPUI Preferences panel, or through `GET/POST /torrents/preferences`. `GET /` on the HTTP API lists every endpoint.

### Disk error recovery and repair

- **Soft I/O recovery** (`soft_recover_on_io_error`, off by default): when a disk write fails, only the affected piece is invalidated and downloaded again. Without it, the whole torrent goes to an error state.
- **Backoff**: automatic retries use exponential backoff with ±20% jitter (`recovery_backoff_base_secs`, `recovery_backoff_cap_secs`). After `recovery_max_attempts` consecutive failures, the piece or file is marked **needs attention**. **Fix errors** retries right away.
- **Disk full** (`ENOSPC`, or `EDQUOT` for a quota): not a fault of the data, so it doesn't count toward the backoff or give-up. The torrent stops downloading (seeding goes on), holds the piece back and shows `Disk full: downloading paused until space is freed (N GB free on <mount>)` as its status, in `stats.damage.disk_full` and as one `disk_full` event. Free space is re-checked every 15 s; once what's left to download (at most 1 GiB, at least two pieces) fits, it resumes by itself (`disk_space_available` event). **Fix errors** retries right away. With soft recovery off, the torrent stops with the error `Disk full: torrent stopped (N GB free on <mount>): ...`.
- **Damaged-file repair** (`POST /torrents/{id}/repair_files`, "Repair damaged files" in the UIs) is for files with ranges that return EIO on every read. It scans the file (O_DIRECT when possible) and punches holes over the unreadable ranges. If that isn't possible, it falls back to copy-and-replace: everything readable is copied to a temp file that is atomically renamed over the original. Only the pieces overlapping the zeroed ranges are downloaded again. The original is never removed until the replacement is complete.
- **Auto-repair** (`auto_repair_damaged_files`, off by default) runs that repair automatically when soft recovery flags a file as damaged.
- **Torrent actions**: Restart (pause + start), Fix errors (re-initialize an errored torrent while keeping pieces that still verify), and Recheck.

### Completion: event hooks and chainable actions

- `completion_actions` is an ordered pipeline that runs when a torrent finishes. Available actions:
  - `shell`: run a command with `sh -c` / `cmd /C`, with `RQBIT_TORRENT_ID`, `RQBIT_INFO_HASH`, `RQBIT_NAME`, `RQBIT_OUTPUT_FOLDER`, `RQBIT_CATEGORY` (the category label), `RQBIT_CATEGORY_SOURCE`, `RQBIT_CATEGORY_ID` and `RQBIT_TORZNAB_CATEGORY` (the Torznab number auto-organize uses) set; unset values are empty
  - `move`: move or copy the torrent to a folder. A multi-file torrent always keeps its own folder: it ends up in `<path>/<TorrentName>/...` with its subfolders, never loose in `<path>`. A single-file torrent's file goes straight into `<path>`.
  - `organize`: see auto-organize below
  - `drop_incomplete_ext`: remove the incomplete suffix
- If the list is empty, the legacy settings are used instead: `on_complete_hook`, `move_completed_path` / `move_completed_copy`, and the organize / incomplete-extension toggles.
- **How completed torrents move** (`move_mode`, used by the `move` and `organize` actions):
  - `"folder"` (default, "Move the whole folder at once"): when the torrent finishes, its whole folder moves in one go. On the same file system that is a single rename. Across file systems, every file is copied, fsynced and read back to verify it, the copy is renamed into place, and only then is the original removed.
  - `"files"` ("Move files individually as they complete"): each file moves to `<destination>/<TorrentName>/<subpath>` as soon as it finishes. The torrent keeps downloading the rest and seeds from both places. When it finishes, the remaining files join them. In this mode the remove dialog doesn't ask about finishing the done files ("finish what's done" just deletes the unfinished files, because the finished ones have already moved).
  - Moves never overwrite. If the destination folder exists and none of the torrent's files clash, the torrent merges into it. If something clashes, it goes to `Name (2)` (or `file (2).ext` for a single file). The incomplete extension comes off before anything moves. Empty folders are cleaned up only inside the old torrent folder. Moved, Copied and Move failed entries show up in Events.
- **Live move/rename**, including while a torrent is active:
  - `POST /torrents/{id}/relocate` `{"destination", "copy", "into"}` moves (or copies) a torrent's data. Seeding continues from the new location with no recheck, and it is persisted. With `"into": true`, `destination` is the folder to put the torrent in (a multi-file torrent goes to `<destination>/<TorrentName>`, which is what both UIs send). Without it, `destination` is the torrent's folder itself. The response describes what happened (`output_folder`, `method`: rename/copy, `whole_folder`, `moved_files`). Reads and writes wait during the move instead of failing.
  - `POST /torrents/{id}/rename_file` `{"file_id", "new_path"}` renames a file on disk. File ids and the piece mapping stay the same.
  - `POST /torrents/{id}/category` `{"category", "category_source", "category_id", "torznab_category"}` edits a torrent's category (also of a magnet still resolving its metadata). A missing key is left alone and `null` clears it. The change is saved, logged as a `category_changed` event and only affects future organizing; nothing is moved. Returns the torrent's details.
- **Auto-organize** (`auto_organize_enabled`, **off by default**): classifies a completed torrent from its name, file names and extensions (anime, tv, movie, game, porn, music, book, software, other) and moves it to `{auto_organize_root}/{type folder}/{name}`. A category takes priority: first the torrent's Torznab number, then the number looked up from its category source and id (see below), then the heuristics. The heuristics can be wrong.
- **Incomplete extension** (`incomplete_extension`, e.g. `.part` or `.!qB`): files get this suffix on disk while downloading, and it is removed on completion. It applies to torrents added after it is set.

### Configure / Admin (qBittorrent-style preferences)

- The web UI's Configure dialog has the tabs Speed, Connection, BitTorrent, Downloads, Organize, Completion, Automation, Interface and Web UI / Admin.
- **Admin** settings are saved in `admin.json` and apply on the next start (environment variables and CLI flags take precedence). They cover the HTTP listen address, basic auth, listen/announce ports, DHT / LSD / trackers / uTP / TCP / UPnP, SOCKS proxy, IPv4-only, bind device and peer limit.
- Admin endpoints: `GET /admin`, `POST /admin/config`, `POST /admin/reload` (re-read `preferences.json`) and `POST /admin/restart`. Restart exits with code 75 so systemd `Restart=on-failure` brings the server back.
- **Categories**: `POST /torrents` takes optional category parameters, stored with the torrent (also while a magnet is resolving), kept across restarts and used by auto-organize:
  - `torznab_category=<number>`: Newznab/Torznab id, e.g. 2000 = Movies, 5070 = Anime, 6000 = Adult. Numbers only; anything else is a 400.
  - `category=<text>`: display name, e.g. `Anime - English-translated` (trimmed, at most 100 characters, no control characters).
  - `category_source=<source>` and `category_id=<id>`: where the category comes from and that source's own id, e.g. `nyaa` and `1_2` (source: 1-32 characters of `a-z 0-9 _ -`, stored lowercase; id: 1-32 characters of `A-Z a-z 0-9 _ . -`). Invalid values are a 400 `invalid_input`.
  - Without a Torznab number, auto-organize looks `(category_source, category_id)` up in a built-in table (`crates/librqbit/src/source_category.rs`, currently `nyaa` and `sukebei`; an unknown `X_n` id falls back to its parent `X_0`). Torznab 8000 (Other, e.g. pictures) has no folder of its own and falls back to the heuristics.
  - Torrent lists and details return `torznab_category`, `category`, `category_source`, `category_id` and `category_label`: the name, else the table name, else `source id`, else the Torznab name (e.g. 5070 is `Anime`). Fields that aren't set are left out.
  - Both UIs show a sortable Category column, a category filter and a Category row in the details, and edit categories of one or more selected torrents from the right-click menu ("Set category…").

### Adding torrents

- The web UI's **Add** window has three tabs: **Upload** (.torrent files and .zip archives of them), **URLs** (magnets and http(s) links, one per line) and **Browse server**. Browse server walks the server's filesystem, limited to the download folder plus `RQBIT_FS_BROWSE_ROOTS`, a comma- or colon-separated list of absolute paths. Items are staged in a queue before anything is added, with an optional custom output folder.
- **Magnets never wait and never fail for missing metadata**: `POST /torrents` checks that a magnet is well formed, queues it and answers at once with its id, info hash and `"resolving": true, "state": "resolving_metadata"`. The metadata is fetched in the background for as long as it takes: an attempt (10 min, or `?magnet_timeout_secs=`) that finds no peers is followed by a backoff (1 min doubling to 30 min, no DHT/tracker queries in between), shown as "Resolving metadata (no peers yet)". Placeholders survive restarts. Only malformed input is an error (400 `invalid_input`: bad scheme, no `xt=urn:btih`, bad hash length/encoding, BTv2-only magnet, unparseable .torrent, a URL that can't be fetched or isn't a .torrent). `?defer_metadata=true` is still accepted (no effect); `?wait_for_metadata=true` restores the old blocking add, but still answers the resolving placeholder instead of an error if the metadata doesn't arrive in time. A `list_only` preview of a magnet needs the metadata: without it the answer is 202 `{"resolving": true, "id": null}` (same for `POST /torrents/resolve_magnet`). Progress of an add can be followed with `?add_job_id=` and `GET /add_jobs/{id}` (`resolving_in_background` = queued).
- **Paused adds**: `POST /torrents?paused=true` (also add jobs and the watch folder) adds paused, including magnets whose metadata is still resolving: the metadata (a few KB, needed to list and choose files) is still fetched (peer lookup via DHT/trackers, metadata exchange only), then the torrent stays paused with no data downloaded; it shows as "Paused · resolving metadata" until then. Preference `when_added` ("When a torrent is added": `start`, the default, or `paused`) is the default for any add that doesn't pass `paused`; an explicit `paused=` wins.
- **Start after I finish the Add dialog** (`start_after_add_dialog`, off by default): adds from an Add dialog (web Add window, GPUI Add panel, the magnet-link handler) carry `add_dialog_id`; the server adds them paused and answers `"held": true`. The dialog sends `POST /add_dialog/{id}/heartbeat` every 15 s and `POST /add_dialog/{id}/finish` when it closes (Close/Cancel, Escape, auto-close after success; `navigator.sendBeacon` on tab close), which starts everything it held that is still there (removed ones are skipped; "Add paused" adds are never held). If the dialog vanishes without finishing, the server starts them after 150 s without a heartbeat; holds are persisted and started shortly after a restart. API, anorak and watch-folder adds are unaffected.
- **Transfer from another client** (`?adopt_foreign_incomplete=auto`) adopts partial files another client left behind (`name.!qB`, `.part`, `.incomplete`, ...) before the first check:
  - a candidate is used only if it is the unique match for a file and its sampled pieces hash-match (or are all zeros)
  - matching files are renamed into place
  - nothing is ever deleted or truncated
- Magnet links / `#add=<magnet>` URLs: see the GPUI client and Preferences > Interface below.
- **JSON add body**: `POST /torrents` with `Content-Type: application/json` takes `{"url": "magnet:..." | "http(s)://..."}` or `{"torrent_base64": "<.torrent bytes, base64>"}` plus any add option under its query-parameter name. See [Add torrent through HTTP API](#add-torrent-through-http-api).

### Compression and the torrent list stream

- **Responses** are compressed with gzip, brotli or zstd (level 4) when the client sends `Accept-Encoding`. Only text-like bodies (JSON, HTML, JS, CSS, wasm, SVG, playlists) of 256 bytes or more are compressed. Media files and ranged responses are never compressed, so Range requests and streaming work as before, and the GPUI bundle's precompressed files aren't compressed again. Streams (`text/event-stream`, `/stream_logs`) aren't compressed either.
- **Request bodies** can be compressed on every endpoint: `Content-Encoding: gzip`, `deflate` (zlib), `br` or `zstd`; uncompressed requests work as before. The body is decompressed as it is read, and the endpoint's usual size limit applies to the *decompressed* size: `POST /torrents` uses `max_upload_body_size` (10 MiB by default) and the JSON endpoints use 2 MiB. Going over the limit returns `413`, data that isn't valid for its encoding returns `400`, and any other `Content-Encoding` returns `415 Unsupported Media Type` with `Accept-Encoding: gzip,deflate,br,zstd`.
- **`GET /stream/torrents`** is the torrent list the UIs use. As a WebSocket it sends a snapshot of a lean list on connect, then one delta per tick (`?tick_ms=`, 1 s by default). A delta is a JSON merge patch carrying only added/removed torrents and the fields that changed. Messages have sequence numbers; a client that sees a gap reconnects and gets a new snapshot, and pings keep idle proxies from closing the connection. With `?enc=deflate` the messages are raw-deflate compressed with a shared window; the WebSocket library has no permessage-deflate, so this is done in the app. Without an Upgrade it is the polling fallback: `?since=<seq>&epoch=<epoch>` returns the delta since then, or a snapshot if that's too old. The auth is the same as the rest of the API. `GET /torrents?with_stats=true` is unchanged.

### Events log

- `events.jsonl` is a persistent, size-capped event log (`event_log_max_mb`, default 10 MB). It records:
  - repairs and damage
  - I/O errors, aggregated per 60 s
  - needs-attention flags, rechecks, adoption renames
  - magnet metadata resolved (waiting for metadata is never logged as a failure)
  - rules firing, queue rotation, cleanup scans
  - every torrent removal
- **Removal logging**: every path writes a `torrent_removed` entry. That covers `/forget`, `/delete`, `/remove`, bulk actions in either UI, rules, finish-what's-done and placeholder magnets. Each entry has the id and name, whether files were kept or deleted, the trigger (`manual`, `automation` or `library`), the endpoint or rule, and the client IP (from `X-Forwarded-For` / `X-Real-IP` behind a proxy) and user agent.
- API: `GET /events?kind=&severity=&torrent_id=&info_hash=&since=`, `GET /events/summary` (repair counters and unseen counts), `POST /events/counters/reset`. Both UIs have an Events view and a per-torrent Events tab.

### Queueing and detailed status

- **Queueing** (off by default) works like qBittorrent: limits on active downloads, uploads and torrents in total, optionally ignoring slow torrents. Held torrents start in queue order (`GET /torrents/queue`, `POST /torrents/queue/move`, persisted in `queue.json`).
- **Detailed status** (`status_detail` in torrent stats) is computed on the server from engine state rather than guessed by the client. Values include: queued for checking / downloading / seeding, checking, resolving metadata, downloading, stalled, seeding, moving, renaming, repairing, waiting to retry, needs attention, disk full, finishing removal.

### Remove policies

- `POST /torrents/{id}/remove` follows a policy, set separately for complete and incomplete torrents: complete `keep`/`delete`, incomplete `keep`/`delete`/`finish`. The saved default is `remove_policy`, and the UIs' remove dialog is preset from it.
  - **Finish what's done** (`finish`): deselects files that aren't fully verified and deletes their partial data, runs the completion actions on the finished files, then forgets the torrent. If an action fails, the torrent is kept and flagged.
- `GET /torrents/remove_preview?ids=` feeds the dialog.
- `confirm_remove` (on by default) controls whether removing asks first. Deleting files always asks.

### Automation rules

Rules are configured globally (`rules`) with per-torrent overrides (`GET/POST /torrents/{id}/rules`). Counters survive restarts. All rules are off by default.

- **No-progress timeout (stalled)**: an incomplete torrent with no verified progress for N seconds of running time can be paused, flagged, or removed. Removal can keep files, follow the policy, delete, or finish what's done.
- **Seeding limits**: stop at a seeding time, uploaded amount or ratio. The torrent is paused or removed.
- **Full-speed window**: seed uncapped for the first N seconds or bytes after completion, then cap this torrent's upload or stop.
- **Slot rotation** (`queue_seed_rotation_secs`, needs queueing and a max-uploads limit): when more torrents want to seed than there are slots, each seeds for that long before the torrent that waited longest takes its slot.

### Download order

Download order is set at three levels, from general to specific: global defaults (`download_order`), per torrent, and per file (`GET/POST /torrents/{id}/download_order`). The options are:
- sequential files vs round-robin
- file order: name, torrent order, smallest first or largest first
- sequential pieces within a file
- first/last piece first, for media previews

Streaming priority still comes first.

### Orphan Cleanup

"Clean up orphaned downloads" finds files and folders in the download locations that no torrent uses any more:
- The scan is a read-only dry run (`GET /cleanup/scan`). It never follows symlinks, skips hidden/system files, and ignores anything modified in the last 60 minutes (`cleanup_min_age_minutes`).
- Everything a torrent uses is protected, including renamed and incomplete-suffix variants.
- Selected items can be moved to `<root>/.rqbit-quarantine/<batch>/` (reversible, `POST /cleanup/restore`) or deleted (explicit confirmation required). Every item is re-validated before anything happens.
- `cleanup_scan_hours` schedules report-only scans.
- Extra folders can be added with `cleanup_extra_roots`, as long as they are inside the allowed roots.

### Public IP

`GET /public_ip` returns the public IPv4/IPv6 as seen from the server's own network (useful behind a VPN or network namespace). Results are cached for 30 s; `?refresh=true` re-checks. `RQBIT_PUBLIC_IP_CHECK=false` disables the lookups, which contact third-party services. `RQBIT_PUBLIC_IP_URLS_V4` / `_V6` override the providers.

### GPUI client (native and WebAssembly)

A second UI built on [GPUI](https://gpui.rs), Zed's UI framework. It is a pure client of the HTTP API, so it can run on another machine. It covers:
- the torrent list, with filters, search, sorting, multi-select and context menus
- a details pane (overview, files, peers, events)
- the Add panel (files, URLs, browse server)
- the Events view, Preferences, and Orphan Cleanup

Details are in [crates/rqbit-gpui/README.md](crates/rqbit-gpui/README.md).

- **Native, inside rqbit**: the optional `gpui` feature adds `rqbit gui`. It is off by default and doesn't change the server build.

      cargo build --release -p rqbit --no-default-features --features rust-tls,webui,prometheus,gpui
      target/release/rqbit gui --url http://127.0.0.1:3030

- **Native, standalone client**: `cargo build --release -p rqbit-gpui --features rustls`, then `rqbit-gpui --url http://host:3030`. `RQBIT_GUI_URL` sets the default URL.
  - Linux needs pkg-config, libxkbcommon, wayland, libxcb/libX11, fontconfig, freetype and Vulkan. On NixOS use `crates/rqbit-gpui/shell.nix`.
  - Windows needs MSVC and the Windows SDK.
- **Default handler for magnet links and .torrent files**:
  - Use `rqbit gui --register-handlers` (or `rqbit-gpui --register-handlers`; `--unregister-handlers` undoes it), or Preferences > Interface.
  - Registration is per user: HKCU on Windows, then confirm in Settings > Default apps. On Linux it writes a `.desktop` file and runs `xdg-mime`.
  - Passing a magnet/URL/file to a running instance hands it to that window.
  - Windows packaging: `crates/rqbit-gpui/packaging/windows/register-handlers.ps1` and an Inno Setup installer script, `rqbit-gpui.iss`.
- **WebAssembly at `/gpui/`**: build it with `crates/rqbit-gpui/web/build.sh`. That is a separate workspace pinned to Rust 1.98.1, and it needs `wasm-bindgen-cli` 0.2.120. Copy `web/dist/*` to `$RQBIT_GPUI_WEB_DIR` (default `~/.local/share/rqbit/gpui-web`); the server serves it at `/gpui/`, same-origin. Browser builds (web UI and `/gpui/`) can register as the magnet handler (`navigator.registerProtocolHandler`, `#add=` prefill).

### Building this fork

- Server as deployed (no OpenSSL / Postgres):

      cargo +stable build --release -p rqbit --no-default-features --features rust-tls,webui,prometheus

  Current dependencies (e.g. sqlx for the `postgres` feature) need a recent stable Rust. The `webui` feature needs npm.
- Feature flags added by the fork: `gpui` (on `rqbit`: the `rqbit gui` subcommand, off by default) and `rustls` (on `rqbit-gpui`: its HTTP client TLS). Upstream flags (`default-tls` / `rust-tls`, `webui`, `postgres`, `prometheus`, ...) are unchanged.
- Tests: `cargo +stable test -p librqbit --lib`; web UI: `cd crates/librqbit/webui && npm test`.

## Usage quick start

### Optional - start the server

Assuming you are downloading to ~/Downloads.

    rqbit server start ~/Downloads

### Download torrents

Assuming you are downloading to ~/Downloads. By default it'll download to current directory.

    rqbit download [-o ~/Downloads] 'magnet:?....' [https?://url/to/.torrent] [/path/to/local/file.torrent]

## Web UI

Access at http://localhost:3030/web/. See screenshot below (torrent names and speeds are simulated).

<img width="1000" src="https://github.com/user-attachments/assets/d916b3d9-ebbd-462a-889d-df3916cc2681" />

## Desktop app

The desktop app is a [thin wrapper](https://github.com/ikatson/rqbit/blob/main/desktop/src-tauri/src/main.rs) on top of the Web UI frontend.

Download it in [Releases](https://github.com/ikatson/rqbit/releases) for OSX and Windows. For Linux, build manually with

    cargo tauri build

It looks similar to the Web UI (screenshot above).

## Streaming support

rqbit can stream torrent files and smartly block the stream until the pieces are available. The pieces getting streamed are prioritized. All of this allows you to seek and live stream videos for example.

You can also stream to e.g. VLC or other players with HTTP URLs. Supports seeking too (through various range headers).
The streaming URLs look like http://IP:3030/torrents/<torrent_id>/stream/<file_id>

## Integrated UPnP Media Server

rqbit can advertise managed torrents to LAN, e.g. your TVs and stream torrents there (without transcoding). Seeking to arbitrary points in the videos is supported too.

Usage from CLI

```
rqbit --enable-upnp-server server start ...
```

## mDNS advertising

rqbit can advertise its HTTP API on your LAN via mDNS/DNS-SD, so you can open the Web UI at http://rqbit.local:3030/web/ from any device without knowing the server's IP.

Usage from CLI (requires a non-loopback listen address):

```
rqbit --enable-mdns --http-api-listen-addr 0.0.0.0:3030 server start ...
```

## IPv6

rqbit supports IPv6. By default it listens on all interfaces in dualstack mode. It can work even if there's no IPv6 enabled.

## Shell completions

Assuming bash, add this to your `~/.bashrc`. Modify for your shell of choice.

```
eval "$(rqbit completions bash)"
```

## Socks proxy support

```
rqbit --socks-url socks5h://[username:password]@host:port ...
```

Supported schemes: `socks5h://` (DNS resolved by the proxy), `socks5://` (DNS resolved
locally).

## Watching a directory for .torrents

```
rqbit server start --watch-folder [path] /download/path
```

## Systemd socket activation

rqbit can be started on-demand via [systemd socket activation](https://0pointer.de/blog/projects/socket-activation.html) by installing the [service and socket systemd units](systemd) into `$XDG_CONFIG_HOME/systemd/user/` (`~/.config/systemd/user`) and customizing them to your needs. If the associated [`rqbit.conf`](systemd/rqbit.conf) file is installed in `$XDG_CONFIG_HOME/rqbit/rqbit.conf` (`~/.config/rqbit/rqbit.conf`), it will be used to configure `rqbit` when started via the provided systemd unit.

## Performance

Anecdotally from a few reports, rqbit is faster than other clients they've tried, at least with their default settings.

Memory usage for the server is usually within a few tens of megabytes, which makes it great for e.g. RaspberryPI.

I've got a report that rqbit can saturate a 20Gbps link, although I don't have the hardware to confirm.

## Installation

There are pre-built binaries in [Releases](https://github.com/ikatson/rqbit/releases).

[![](https://repology.org/badge/vertical-allrepos/rqbit.svg)](https://repology.org/project/rqbit/versions)

### Homebrew

**rqbit** can be installed using Homebrew.
```sh
brew install rqbit
```

### Cargo

If you have the Rust toolchain installed then you can use the following.
```sh
cargo install rqbit
```

## NixOS

Current NixOS already has `services.rqbit`. Use that. No container is required.

```nix
services.rqbit = {
  enable = true;
  downloadDir = "/var/lib/rqbit/downloads";
  httpHost = "0.0.0.0";
  httpPort = 3030;
  openFirewall = true;
};
```

`nix/package.nix` installs the static `v9.0.1` Linux binary if you want this release instead of the nixpkgs build:

```nix
services.rqbit.package = pkgs.callPackage ./nix/package.nix { };
```

```bash
nix-build -E 'with import <nixpkgs> {}; callPackage ./nix/package.nix {}'
```

To keep torrent traffic off the host route, import `nix/module.nix` and run rqbit inside a network namespace that has no path except a VPN:

```nix
imports = [ /path/to/rqbit/nix/module.nix ];

services.rqbit = {
  enable = true;
  httpHost = "0.0.0.0";
  networkNamespace = "vpn";
  namespaceService = "vpn-netns.service";
};
```

Create the namespace first, and point `/etc/netns/vpn/resolv.conf` at resolvers reached through the tunnel. Leave `openFirewall` off. From the host, open the web UI on the namespace address and port, for example `http://192.0.2.2:3030/web/`.

## Build

Just a regular Rust binary build process.

    cargo build --release

The "webui" feature requires npm installed.

## Some useful options

Run ```rqbit --help``` to see all available CLI options.

### -v <log-level>

Increase verbosity. Possible values: trace, debug, info, warn, error.

### --list

Will print the contents of the torrent file or the magnet link.

### --overwrite

If you want to resume downloading a file that already exists, you'll need to add this option.

### -r / --filename-re

Use a regex here to select files by their names.

## Features (not exhaustive)

### Supported BEPs

- [BEP-3: The BitTorrent Protocol Specification](https://www.bittorrent.org/beps/bep_0003.html)
- [BEP-5: DHT Protocol](https://www.bittorrent.org/beps/bep_0005.html)
- [BEP-7: IPv6 Tracker Extension](https://www.bittorrent.org/beps/bep_0007.html)
- [BEP-9: Extension for Peers to Send Metadata Files](https://www.bittorrent.org/beps/bep_0009.html)
- [BEP-10: Extension Protocol](https://www.bittorrent.org/beps/bep_0010.html)
- [BEP-11: Peer Exchange (PEX)](https://www.bittorrent.org/beps/bep_0011.html)
- [BEP-12: Multitracker Metadata Extension](https://www.bittorrent.org/beps/bep_0012.html)
- [BEP-14: Local service discovery](https://www.bittorrent.org/beps/bep_0014.html)
- [BEP-15: UDP Tracker Protocol](https://www.bittorrent.org/beps/bep_0015.html)
- [BEP-20: Peer ID Conventions](https://www.bittorrent.org/beps/bep_0020.html)
- [BEP-23: Tracker Returns Compact Peer Lists](https://www.bittorrent.org/beps/bep_0023.html)
- [BEP-27: Private Torrents](https://www.bittorrent.org/beps/bep_0027.html)
- [BEP-29: uTorrent Transport Protocol](https://www.bittorrent.org/beps/bep_0029.html)
- [BEP-32: IPv6 extension for DHT](https://www.bittorrent.org/beps/bep_0032.html)
- [BEP-47: Padding files and extended file attributes](https://www.bittorrent.org/beps/bep_0047.html)
- [BEP-53: Magnet URI extension - Select specific file indices for download](https://www.bittorrent.org/beps/bep_0053.html)

### Some supported features

- Sequential downloading (the default and only option)
- Resume downloading file(s) if they already exist on disk
- Selective downloading using a regular expression for filename
- DHT support. Allows magnet links to work, and makes more peers available.
- HTTP API
- Pausing / unpausing / deleting (with files or not) APIs
- Stateful server
- Web UI
- Streaming, with seeking
- UPNP port forwarding to your router
- UPNP Media Server
- mDNS advertising
- Fastresume (no rehashing)
- Download / upload rate limiting
- Prometheus metrics at ```/metrics``` and ```/torrents/<id_or_infohash>/peer_stats/prometheus```

## HTTP API

By default it listens on http://127.0.0.1:3030.

```
curl -s 'http://127.0.0.1:3030/'

{
  "apis": {
    "GET /": "list all available APIs",
    "GET /dht/stats": "DHT stats",
    "GET /dht/table": "DHT routing table",
    "GET /metrics": "Prometheus metrics",
    "GET /stats": "Global session stats",
    "GET /stream_logs": "Continuously stream logs",
    "GET /torrents": "List torrents",
    "GET /stream/torrents": "Torrent list for UIs: WebSocket (snapshot, then deltas; ?tick_ms=, ?enc=deflate) or, without Upgrade, a delta since ?since=<seq>&epoch=<epoch> (else a snapshot)",
    "GET /torrents/playlist": "Playlist for supported players",
    "GET /torrents/{id_or_infohash}": "Torrent details",
    "GET /torrents/{id_or_infohash}/haves": "The bitfield of have pieces",
    "GET /torrents/{id_or_infohash}/metadata": "Download the corresponding torrent file",
    "GET /torrents/{id_or_infohash}/peer_stats": "Per peer stats",
    "GET /torrents/{id_or_infohash}/peer_stats/prometheus": "Per peer stats in prometheus format",
    "GET /torrents/{id_or_infohash}/playlist": "Playlist for supported players",
    "GET /torrents/{id_or_infohash}/stats/v1": "Torrent stats",
    "GET /torrents/{id_or_infohash}/stream/{file_idx}": "Stream a file. Accepts Range header to seek.",
    "GET /web/": "Web UI",
    "POST /rust_log": "Set RUST_LOG to this post launch (for debugging)",
    "POST /torrents": "Add a torrent here. magnet: or http:// or a local file, or a JSON body (Content-Type: application/json) {\"url\" or \"torrent_base64\", plus add options by query-param name}. Request bodies may be gzip/deflate/br/zstd (Content-Encoding).",
    "POST /torrents/create": "Create a torrent and start seeding. Body should be a local folder",
    "POST /torrents/resolve_magnet": "Resolve a magnet to torrent file bytes",
    "POST /torrents/{id_or_infohash}/add_peers": "Add peers (newline-delimited)",
    "POST /torrents/{id_or_infohash}/delete": "Forget about the torrent, remove the files",
    "POST /torrents/{id_or_infohash}/forget": "Forget about the torrent, keep the files",
    "POST /torrents/{id_or_infohash}/pause": "Pause torrent",
    "POST /torrents/{id_or_infohash}/start": "Resume torrent",
    "POST /torrents/{id_or_infohash}/update_only_files": "Change the selection of files to download. You need to POST json of the following form {\"only_files\": [0, 1, 2]}"
  },
  "server": "rqbit",
  "version": "9.0.0-beta.1"
}
```

### Basic auth

For HTTP API basic authentication set RQBIT_HTTP_BASIC_AUTH_USERPASS environment variable.

```
RQBIT_HTTP_BASIC_AUTH_USERPASS=username:password rqbit server start ...
```

### Add torrent through HTTP API

`curl -d 'magnet:?...' http://127.0.0.1:3030/torrents`

OR

`curl -d 'http://.../file.torrent' http://127.0.0.1:3030/torrents`

OR

`curl --data-binary @/tmp/xubuntu-23.04-minimal-amd64.iso.torrent http://127.0.0.1:3030/torrents`

Supported query parameters, all optional:

- overwrite=true|false
- only_files_regex - the regular expression string to match filenames
- output_folder - the folder to download to. If not specified, defaults to the one that rqbit server started with
- list_only=true|false - if you want to just list the files in the torrent instead of downloading

OR, with a JSON body (only when `Content-Type: application/json`; any other body is taken as above, so a .torrent that happens to start with `{` is never read as JSON):

```
curl -H 'Content-Type: application/json' \
  -d '{"url": "magnet:?xt=urn:btih:...", "torznab_category": 5070, "category": "Anime - English-translated", "category_source": "nyaa", "category_id": "1_2", "paused": true}' \
  http://127.0.0.1:3030/torrents
```

- Exactly one of `url` (a `magnet:` link or an `http(s)://` URL of a .torrent) and `torrent_base64` (the .torrent file, base64; standard or URL-safe alphabet, padding optional).
- Any add option under its query-parameter name, in JSON types: `torznab_category` (number), `category`, `category_source`, `category_id`, `paused`, `overwrite`, `list_only`, `output_folder`, `sub_folder`, `only_files` (`[0, 2]`), `only_files_regex`, `initial_peers` (`["192.0.2.1:6881"]`), `peer_connect_timeout`, `peer_read_write_timeout`, `magnet_timeout_secs`, `wait_for_metadata`, `defer_metadata`, `adopt_foreign_incomplete`, `add_job_id`, `add_dialog_id`. Strings are also accepted where the query string takes them (e.g. `"only_files": "0,2"`).
- Query parameters still apply, but an option set in both places takes the JSON value; `null` in the JSON unsets it. `is_url` and `from_server_path` aren't accepted in a JSON body.
- Errors: `400` with `invalid_input` for invalid JSON, a missing or doubled `url`/`torrent_base64`, an unknown field, a value of the wrong type or bad base64; the add itself fails in the same ways as the other forms. `413` if the body is over `max_upload_body_size`. The body may be compressed (`Content-Encoding: gzip|deflate|br|zstd`), e.g. `gzip -c add.json | curl -H 'Content-Type: application/json' -H 'Content-Encoding: gzip' --data-binary @- http://127.0.0.1:3030/torrents`.

## Code organization

- crates/rqbit - main binary
- crates/librqbit - main library
- crates/librqbit-core - torrent utils
- crates/bencode - bencode serializing/deserializing
- crates/buffers - wrappers around binary buffers
- crates/clone_to_owned - a trait to make something owned
- crates/sha1w - wrappers around sha1 libraries
- crates/peer_binary_protocol - the protocol to talk to peers
- crates/dht - Distributed Hash Table implementation
- crates/upnp - upnp port forwarding
- crates/upnp_serve - upnp MediaServer
- desktop - desktop app built with [Tauri](https://tauri.app/)
- [librqbit-utp](https://github.com/ikatson/librqbit-utp/) - uTP protocol
- [librqbit-dualstack-sockets](https://github.com/ikatson/librqbit-dualstack-sockets) - cross-platform IPv6+IPv4 listeners with canonical IPs

## Motivation

This project began purely out of my enjoyment of writing code in Rust. I wasn’t satisfied with my regular BitTorrent client and wanted to see how much effort it would take to build one from scratch. Starting with the bencode protocol, then the peer protocol, it gradually evolved into what it is today.

## Donations and sponsorship

If you love rqbit, please consider donating through one of these methods. With enough support, I might be able to make this my full-time job one day — which would be amazing!

- [Github Sponsors](https://github.com/sponsors/ikatson)
- Crypto
  - ETH (Ethereum) 0x68c54b26b5372d5f091b6c08cc62883686c63527
  - XMR (Monero) 49LcgFreJuedrP8FgnUVB8GkAyoPX7A9PjWfKZA1hNYz5vPCEcYQ9HzKr3pccGR6Lc3V3hn52bukwZShLDhZsk57V41c2ea
  - XNO (Nano) nano_1ghid3z6x41x8cuoffb6bbrt4e14wsqdbyqwp5d8rk166meo3h77q7mkjusr
