# Afinín

> *Tune your AWS spend. See where the money goes, locally and safely.*

Afinín (from *afinar*, to tune an instrument) is a local, cross-platform desktop app that
connects **read-only** to your AWS accounts, shows where the money goes and how your
services connect, and flags waste and anomalies — with any LLM or none.

Status: **v0.1.0 (MVP), not yet released.** See [`CHANGELOG.md`](CHANGELOG.md) and the
[requirements](requirement.md).

## What it does

- **Where the money goes**: month to date, forecast, month-over-month; breakdowns by account,
  service, region, usage type, tag and (with CUR 2.0) individual resource; period comparisons;
  CSV export.
- **What you have and how it connects**: resources across every enabled region, inferred
  relationships (load balancer → targets → volumes, event sources, CloudFront origins…) on an
  interactive graph with a cost overlay and tag-based applications.
- **What to look at**: waste with estimated savings (unattached volumes, idle IPs, load
  balancers and NAT gateways, stopped instances, old snapshots), cost anomalies, top movers,
  data transfer, Savings Plans / RI coverage.
- **Optional assistant**: ask questions in plain language with a local model or a cloud
  provider; answers cite Afinín's own figures and identifiers are masked before leaving your
  machine. Teach it your business with a workspace context, check answers against references,
  and see what each answer cost.

Read-only, local-first, no telemetry. See [docs/setup.md](docs/setup.md) and
[docs/privacy.md](docs/privacy.md).

## Repository layout

| Path | What |
|---|---|
| `crates/afinin-core` | Rust core library (auth, storage, collectors, analysis). Independent of Tauri. |
| `apps/desktop` | Tauri desktop app: React + TypeScript UI (`src/`) and Rust shell (`src-tauri/`). |
| `policies/` | Read-only IAM policy, CloudFormation StackSet / Terraform role templates, CUR 2.0 export template. |
| `docs/` | Setup, privacy and security. |
| `scripts/` | CI checks (no AWS write calls, SPDX headers) and the IAM template generator. |

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
cargo test -p afinin-core --test readonly   # fails if any AWS write operation is called
./scripts/check-spdx.sh                     # every source file has the SPDX header
python3 scripts/generate-policy-templates.py  # after editing policies/afinin-readonly.json
```

Releases: see [RELEASING.md](RELEASING.md).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion
in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above,
without any additional terms or conditions. See [CONTRIBUTING.md](CONTRIBUTING.md).

Copyright (c) 2026 trompetin17 (juancarvajalsas)
