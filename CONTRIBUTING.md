# Contributing to Afinín

Thanks for helping tune Afinín!

## Ground rules

- **Read-only, always.** Afinín never mutates AWS resources. Only `Describe*`, `List*`, `Get*`
  and similar read calls are allowed. PRs adding write calls will be rejected (CI enforces this).
- **No credentials anywhere but the OS keychain.** Never log, persist or print secrets or
  account identifiers.
- **Permissive dependencies only**: MIT, Apache-2.0, BSD, ISC, Zlib, MPL-2.0. No GPL/AGPL.
- Every source file starts with the SPDX header:
  - Rust / TypeScript: `// SPDX-License-Identifier: MIT OR Apache-2.0`

## Developer Certificate of Origin (DCO)

Contributions are accepted under the project's dual license (MIT OR Apache-2.0).
Sign off every commit to certify the [DCO](https://developercertificate.org/):

```sh
git commit -s -m "your message"
```

## Checks before opening a PR

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps/desktop && npm run lint && npm run build
```
