# Privacy and security

Afinín is local-first: there is no Afinín backend and no telemetry.

## What stays on your machine

- **Credentials** (SSO tokens, API keys) live only in the OS keychain — macOS Keychain, Windows
  Credential Manager or the Linux Secret Service. They never enter the database, logs, backups or
  LLM requests.
- **Data** (costs, inventory, findings, settings, assistant request log) is stored in a DuckDB
  database encrypted at rest. Its key is generated on first run and kept in the keychain.
- **CUR files** are downloaded to a temporary file, loaded into the encrypted database and
  deleted immediately.
- **Backups** are single files encrypted with your password (Argon2id + XChaCha20-Poly1305).

## What leaves your machine

- Read-only AWS API calls, made with your credentials, to your accounts.
- AWS Price List queries (public prices; no account data).
- If you enable the assistant: requests to the provider you chose.

## The assistant and redaction

With a cloud provider, redaction is on by default. Before anything is sent, Afinín replaces
account IDs, ARNs, resource IDs, IP addresses, host names, tag values and resource and account
names with placeholders such as `ACCOUNT_1` or `NAME_3`. The model works with placeholders, the
tools Afinín runs locally receive the real values, and the answer you see has them restored.
Local servers (Ollama, LM Studio on `localhost`) are not redacted by default because nothing
leaves your machine; you can change both defaults per provider.

Every request is recorded exactly as sent: **Assistant → Show what was sent**.

The model never sees raw data dumps and never calculates figures: it calls read-only tools
backed by Afinín's local query engine, and every number in an answer cites the tool result it
came from.

## Read-only by design

Afinín contains no AWS write calls. This is enforced by an automated test
(`crates/afinin-core/tests/readonly.rs`) and a CI check, and the IAM policies in `policies/`
grant read access only. Afinín's only "write-looking" calls are the OAuth token exchange of the
IAM Identity Center sign-in, which changes nothing in your accounts.

## Desktop hardening

Strict Content Security Policy, no remote content in the app window, a minimal command surface
between the window and the Rust core, no plugin permissions granted to the window, signed
updates and published SBOMs.

Report vulnerabilities privately through GitHub security advisories on the repository.
