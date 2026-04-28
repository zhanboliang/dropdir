# dropdir

[English](./README.md) | [中文](./README.zh-CN.md)

A tiny, zero-config file manager served over HTTP from a local directory. Run it in a folder (or point it at one), open the printed URL in your browser, and you get a browseable list with upload / delete / rename / in-browser text editing.

Built as a single Rust binary on top of [axum](https://github.com/tokio-rs/axum) + [tokio](https://tokio.rs). The frontend is one static HTML file baked into the binary — no build step, no Node, no external assets.

## Features

- Browse the serving directory and any sub-directories (relative-path URLs, never absolute system paths).
- Upload files (multi-select, up to 1 GiB per request).
- Rename and delete files / empty directories.
- In-browser editor for common text formats (`.txt .md .json .yaml .toml .rs .go .py .rb .js .ts .html .css ...` — see `src/text_ext.rs`).
- Stream download of any file type.
- Single self-contained binary — drop it on any machine and run.

## Security model

dropdir is aimed at "the tool you point at a folder when you need to move files around quickly". It is **not** a production file server, but it is built to be safe enough for local-network use.

- **Localhost-first.** Default bind is `127.0.0.1:8089`. Pass `--open` (or `--host 0.0.0.0`) to expose on the LAN. The banner prints a big warning when this happens.
- **Token auth on every request.** A fresh 128-bit token (from `getrandom`) is generated on startup. Accepted via `Authorization: Bearer <t>`, the `X-Dropdir-Token` header, or a `?t=<t>` query parameter. Comparison is constant-time. The frontend grabs the token from the initial URL, stashes it in memory, and strips it from the address bar with `history.replaceState`.
- **Path confinement.** Every path argument is validated: absolute paths, empty / `.` / `..` components, and null bytes are rejected. The result is canonicalized against the nearest existing ancestor and required to stay under the serve root, so symlinked escapes are blocked for both existing and not-yet-created targets.
- **Write-side filename blocklist.** `upload`, `write`, and `rename`-destination refuse native-executable and shell-script extensions (`.exe .dll .msi .bat .cmd .ps1 .vbs .app .dmg .pkg .so .dylib .sh .bash .zsh .fish .apk .jar ...`) and sensitive filenames (`authorized_keys`, `.bashrc`, `autorun.inf`, etc.). Reads / downloads of files that *already* exist are not blocked — this is a write-side wall, not censorship.
- **Symlink write protection.** `write`, `upload`, and `rename` use `symlink_metadata` to refuse operations that would go *through* a symlink to an arbitrary target.
- **Browser hardening.** Every response carries a strict CSP (`default-src 'none'`), `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, and `X-Frame-Options: DENY`. Downloads are served as `Content-Disposition: attachment`, so HTML/SVG uploaded to dropdir cannot execute scripts in the browser.
- **Size caps.** Editor read/write is capped at 10 MiB. Upload body is capped at 1 GiB.

## Build & install

Requires a recent Rust toolchain (Rust 1.85+ for edition 2024).

```bash
cargo build --release
# The binary lives at target/release/dropdir
# Copy it somewhere on your PATH:
cp target/release/dropdir /usr/local/bin/
```

## Usage

```text
dropdir [DIR] [OPTIONS]
```

| Positional | Meaning |
|---|---|
| `DIR` | Directory to serve. Must be the first argument if supplied. Defaults to the current working directory. |

| Flag | Default | Description |
|---|---|---|
| `--host <HOST>` | `127.0.0.1` | Bind address |
| `--open` | — | Shortcut for `--host 0.0.0.0` (expose on LAN) |
| `--port <PORT>` | `8089` | TCP port |
| `--token <TOKEN>` | random | Use a fixed token instead of a generated one |
| `--no-auth` | — | Disable auth entirely. **Dangerous** on shared networks |
| `-h`, `--help` | — | Print help |

### Examples

```bash
dropdir                                     # serve the current directory
dropdir /Users/me/Downloads                 # serve a specific directory
dropdir ./project --open --port 9000        # LAN share of ./project on :9000
dropdir /data --token mysecret --port 8100  # fixed token, custom port
```

On startup dropdir prints an `open url` line that already contains the token — open it in the browser and you're in.

## HTTP API

All endpoints require the auth token (unless `--no-auth`). `?t=<token>` is accepted on every endpoint; the `Authorization: Bearer` header is accepted on all of them; the `X-Dropdir-Token` header works too.

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/` | Single-page UI |
| `GET` | `/api/list?path=<subdir>` | Directory contents (name, is_dir, size, modified, editable) |
| `GET` | `/api/read?path=<file>` | Read a text file (UTF-8, ≤ 10 MiB, editable types only) |
| `POST` | `/api/write` | `{ "path": "...", "content": "..." }` — save a text file |
| `POST` | `/api/upload?path=<subdir>` | `multipart/form-data` upload of one or more files |
| `POST` | `/api/rename` | `{ "from": "...", "to": "..." }` |
| `DELETE` | `/api/delete?path=<file_or_empty_dir>` | Remove a file or empty directory |
| `GET` | `/api/download?path=<file>` | Stream download of any file |

## Limitations

- Single user: no per-user accounts, just one shared token. If you need multi-user, wrap dropdir behind a reverse proxy that handles identity.
- No HTTPS. Run behind a TLS-terminating proxy (Caddy / nginx / Cloudflare Tunnel) if you need that.
- Delete only removes empty directories. Recursive delete is deliberately not offered.
- The editor is a plain `<textarea>` — no syntax highlighting. Keeping the HTML small was the goal.

## Layout

```
src/
  main.rs            # CLI parsing, auth setup, router wiring
  routes.rs          # HTTP handlers + auth middleware + security headers
  fs_ops.rs          # Path validation (safe_join), FsError
  text_ext.rs        # Editable text extensions + write-side blocklist
  assets/index.html  # Single-file frontend (compiled into the binary)
```
