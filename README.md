# Afinín

> *Tune your AWS spend. See where the money goes, locally and safely.*

Afinín (from *afinar*, to tune an instrument) is a local, cross-platform desktop app that
connects **read-only** to your AWS accounts, shows where the money goes and how your
services connect, and flags waste and anomalies — with any LLM or none.

Status: **early development.** Done: M0 skeleton, M1 Cost Explorer costs, M2 inventory & dependency graph, M3 CUR 2.0 & per-resource costs, M4 findings, M5 LLM assistant. See [`requirement.md`](requirement.md).

## Repository layout

| Path | What |
|---|---|
| `crates/afinin-core` | Rust core library (auth, storage, collectors, analysis). Independent of Tauri. |
| `apps/desktop` | Tauri desktop app: React + TypeScript UI (`src/`) and Rust shell (`src-tauri/`). |
| `policies/` | Read-only IAM policies for the accounts Afinín connects to. |
| `scripts/` | Policy checks run in CI: no AWS write calls (NFR-1), SPDX headers (NFR-7). |

## Development

Prerequisites: Rust (stable), Node.js 20+, and the
[Tauri system dependencies](https://v2.tauri.app/start/prerequisites/)
(on Arch: `sudo pacman -S webkit2gtk-4.1 libsoup3 libsecret`; on Debian/Ubuntu:
`libwebkit2gtk-4.1-dev librsvg2-dev libsecret-1-dev`). On Linux a Secret Service
provider (GNOME Keyring or KWallet) must be running: Afinín keeps its database key there.

```sh
cargo test -p afinin-core          # core library tests
cd apps/desktop
npm install
npm run tauri dev                  # run the desktop app
```

Policy checks:

```sh
./scripts/check-readonly.sh        # fails on write-like AWS SDK calls
./scripts/check-spdx.sh            # every source file has the SPDX header
```

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions. See [CONTRIBUTING.md](CONTRIBUTING.md).

Copyright (c) 2026 trompetin17 (juancarvajalsas)
