# Requirements — **Afinín**

> *Tune your AWS spend. See where the money goes, locally and safely.*
>
> Afinín (from *afinar*, to tune an instrument) is a local, cross-platform desktop app that connects read-only to your AWS accounts, shows where the money goes and how your services connect, and flags waste and anomalies, with any LLM or none.
>
> By trompetin17 · juancarvajalsas · sibling project of [Fanfarin](https://github.com/juancarvajalsas/fanfarin)
>
> Status: Draft v0.1 · Last updated: 2026-10-13

## 1. Vision

A free, open-source, cross-platform desktop app that connects securely to multiple AWS accounts and answers three questions:

1. **Where is the money going?** (by account, service, region, tag, resource, over time)
2. **What do I have and how is it connected?** (inventory + dependency map)
3. **What should I look at?** (anomalies, waste, patterns — explained with optional LLM help)

The app is **read-only**: it analyzes and recommends; users act in the console or their IaC.

## 2. Target users

- Engineers, DevOps/platform teams and founders at startups and small companies.
- Typical customer: **1–10 AWS accounts**, often under AWS Organizations.
- Users are technical but not necessarily FinOps experts.

## 3. Product decisions (settled)

| Topic | Decision |
|---|---|
| Name | **Afinín** (written `afinin` in code, package names, binaries and repo: `juancarvajalsas/afinin`) |
| Distribution | Fully open source, dual-licensed **MIT OR Apache-2.0** (Rust ecosystem convention; maximum adoption, attribution required) |
| Form factor | Desktop GUI with **Tauri** (Rust core + web UI) |
| Frontend | **React + TypeScript** for the whole UI |
| Platforms | macOS, Windows, Linux at launch |
| Scope of AWS access | **Read-only**, never mutates resources |
| Data storage | **Local only**, encrypted at rest; no backend |
| Cost data | Tiered: **Cost Explorer API** (zero setup) + **CUR 2.0 / Data Exports** (resource-level detail) |
| LLM | **Pluggable** provider; optional; redaction of identifiers **on by default** for cloud providers |

## 4. Functional requirements

### 4.1 Account connection & authentication

- **FR-1** Support these credential sources, in order of recommendation:
  1. IAM Identity Center (SSO) — **recommended default**, short-lived credentials.
  2. Existing `~/.aws/config` profiles, including `role_arn` / `source_profile` assume-role chains.
  3. AWS Organizations: discover member accounts from the management account and assume a cross-account read-only role in each (e.g. `OrganizationAccountAccessRole` replacement with read-only permissions).
  4. Long-lived IAM access keys — supported but **discouraged**; show a warning and a link to migrate to SSO.
- **FR-2** Secrets (access keys, cached tokens) are stored only in the **OS keychain** (macOS Keychain, Windows Credential Manager, Linux Secret Service). Never in the app database, logs or config files.
- **FR-3** Provide a **"Test connection"** check per account that validates identity (`sts:GetCallerIdentity`) and reports which required permissions are missing.
- **FR-4** Ship a **ready-to-deploy read-only IAM policy / CloudFormation StackSet** (and Terraform module) that customers can apply to their accounts.
- **FR-5** Group accounts into **workspaces** (e.g. "Company A", "Personal").

### 4.2 Cost analysis

- **FR-10** Cost overview dashboard: total spend, month-to-date, forecast, month-over-month change.
- **FR-11** Breakdown and filtering by account, service, region, usage type, linked account, cost allocation tag, and (with CUR) individual resource ID.
- **FR-12** Time ranges: daily / monthly granularity, custom range; comparison between two periods.
- **FR-13** Cost metric selection: unblended, amortized (to reflect Savings Plans / RIs), net.
- **FR-14** **Tier 1 — Cost Explorer**: works out of the box. The app must track and display Cost Explorer API calls made (AWS charges per request) and cache results locally to minimize calls.
- **FR-15** **Tier 2 — CUR 2.0**: guided setup wizard to create a Data Export to S3 (Parquet) in the payer account; the app downloads new files incrementally and queries them locally.
- **FR-16** Export any view to CSV.

### 4.3 Inventory & service dependency map

- **FR-20** Discover resources across all enabled regions per account (EC2, EBS, ELB/ALB/NLB, RDS, Lambda, S3, DynamoDB, ECS/EKS, NAT Gateway, VPC/subnets, CloudFront, API Gateway, SQS/SNS, ElastiCache, Lightsail — extendable via a provider/plugin model).
- **FR-21** Use AWS Config relationships and Resource Explorer when enabled; fall back to service `Describe*/List*` APIs.
- **FR-22** Infer relationships, e.g.: VPC → subnet → instance; LB → target group → instance/ECS service; Lambda → event source (SQS, DynamoDB stream, API Gateway); resource → security group; EBS → instance; ENI → owner.
- **FR-23** Interactive graph view: zoom, filter by account/region/VPC/tag/service (including resources *missing* a tag), search, click a node for details. Two layouts of the same graph: **Flow** (layered by role: entry points → compute → data, with a minimap) and **Network** (force-directed; isolated resources hidden behind a "show them" action; labels hidden when zoomed out).
- **FR-24** **Cost overlay** on the graph: node size/color by monthly cost (requires CUR for per-resource cost; otherwise aggregated by service).
- **FR-25** Group resources into logical "applications" by tag (e.g. `app`, `env`) or manual grouping, and show the cost per application.

### 4.4 Anomalies, waste & patterns

- **FR-30** Cost anomaly detection: daily spend compared against a rolling baseline per account/service (statistical rule, configurable sensitivity). Optionally import AWS Cost Anomaly Detection findings.
- **FR-31** Waste detection rules (each with estimated monthly savings):
  - Unattached EBS volumes; old EBS snapshots
  - Unassociated Elastic IPs / public IPv4 charges
  - Idle load balancers (no targets / no traffic)
  - Idle NAT Gateways; high NAT data-processing costs
  - Stopped EC2 instances still paying for EBS
  - Low-utilization EC2/RDS (CloudWatch metrics — opt-in, as metric API calls cost money)
  - S3 buckets without lifecycle policies; old-generation instance types
  - Untagged resources (tag coverage %)
- **FR-32** Patterns & trends: top movers, cost growth by service, data transfer breakdown, Savings Plans / RI coverage and utilization.
- **FR-33** Findings list with severity, estimated savings, affected resources, and a "how to fix" explanation. Findings can be dismissed/snoozed (stored locally).
- **FR-34** Savings estimates use the customer's actual rates from CUR when available (including discounts, Savings Plans, RIs); otherwise public prices from the AWS Price List API, clearly labeled as "estimate (public price)".

> **MVP rule set (FR-31):** unattached EBS volumes, unassociated Elastic IPs / public IPv4, idle load balancers, stopped EC2 with attached EBS, old EBS snapshots, idle NAT Gateways. All other rules (CloudWatch utilization, S3 lifecycle, old-generation types, tag coverage) follow after the MVP.

### 4.5 LLM assistant (optional)

- **FR-40** Providers (pluggable interface): local (Ollama / any OpenAI-compatible local server), AWS Bedrock, Anthropic, OpenAI-compatible APIs. The app is fully usable with **no LLM configured**.
- **FR-41** Chat panel to ask natural-language questions, e.g. "Why did our bill go up 30% last week?", "What is connected to this RDS instance?", "What are the top 5 savings opportunities?"
- **FR-42** The LLM works through **tool calls** against local data (query costs, list findings, get graph neighbors) instead of receiving raw data dumps. All tools are read-only.
- **FR-43** "Explain this" buttons on anomalies, findings and graph nodes.
- **FR-44** **Redaction layer** for cloud providers (on by default): mask account IDs, ARNs, resource IDs, IPs, hostnames and tag values with reversible placeholders; un-mask in the displayed answer. User can disable per provider.
- **FR-45** "Show what was sent" log of every LLM request, stored locally.
- **FR-46** Answers must cite the data they used (which query/finding) so users can verify numbers. Numbers shown come from the local query engine, not generated by the model. Figures in an answer that match no source, the question or the workspace context are flagged as unverified.
- **FR-47** **Workspace context**: per workspace, the user describes the business (what each account is for, what tags and glossary terms mean, answer instructions) as key → value entries. It is sent with every question, masked like everything else, versioned with history, and answers show which revision they used. The current screen (view, period, filters) is sent too, so "this" means what the user is looking at.
- **FR-48** **Answer quality loop**: conversations are saved and can be resumed; answers can be rated (useful / not right with a note) and marked as **reference answers**. References guide similar questions (only their tool calls are shown to the model, never their old figures) and can be **re-checked** against current data: the question is asked again and the queries, unverified figures and **expected values** (figures matched at any rounding, or text) are compared. When the model reports that data is missing (CUR, inventory sync, cost allocation tags, history), the answer says how to get it and History lists these gaps.
- **FR-49** **Traceability and LLM cost**: each answer records its rounds (tokens, prompt-cache reads/writes, latency, linked request) and tool calls (label, run time, size sent), shown as a trace with warnings for wasteful patterns (repeated queries, failed calls, large results resent every round, hitting the round limit). Identical tool calls in one answer run once. Anthropic requests use prompt caching. With a price per model in Settings, cost is estimated per round, answer, re-check run and month; an optional monthly budget stops the assistant when reached. Clearing the request log keeps token counts.

### 4.6 Data sync & settings

- **FR-50** Background sync with configurable schedule; manual refresh; per-account sync status and last-updated time.
- **FR-51** Keep history locally (configurable retention) to enable trend and anomaly analysis beyond the Cost Explorer window.
- **FR-52** Settings: regions to scan, sync frequency, LLM provider, redaction, currency display, theme.
- **FR-53** Backup & restore: export all local data (history, findings state, workspaces, settings — never credentials) to a single password-encrypted file; import it on another machine.

## 5. Non-functional requirements

### 5.1 Security & privacy
- **NFR-1** Read-only by design: the app requests and uses only read/list/describe/get permissions; no write SDK calls exist in the codebase (enforced by a CI check).
- **NFR-2** Local database encrypted at rest; the key is generated on first run and stored only in the OS keychain. Prototype DuckDB native encryption first; SQLCipher as fallback.
- **NFR-3** No telemetry by default. Any telemetry is opt-in and documented.
- **NFR-4** Credentials never logged; logs scrubbed of identifiers.
- **NFR-5** Tauri security: strict CSP, minimal IPC command surface, no remote code loaded into the webview.
- **NFR-6** Signed releases and auto-updates (Tauri updater with signature verification); SBOM published per release.
- **NFR-7** Licensing: the project is dual-licensed MIT OR Apache-2.0; users may choose either. Both require keeping the copyright notice `Copyright (c) 2026 trompetin17 (juancarvajalsas)`. Dependencies must be permissively licensed (MIT, Apache-2.0, BSD, ISC, Zlib; MPL-2.0 acceptable); no GPL/AGPL dependencies. CI runs a license check (e.g. `cargo-deny`, `license-checker` for npm). Every source file carries the SPDX header `MIT OR Apache-2.0`. Contributions accepted under the same dual license (DCO sign-off).

### 5.2 Performance
- **NFR-10** Initial sync for 10 accounts × all regions completes in < 10 minutes; subsequent syncs are incremental.
- **NFR-11** Dashboard and filter interactions respond in < 1 s on cached data.
- **NFR-12** Graph view handles at least 5,000 nodes with filtering.
- **NFR-13** Respect AWS API rate limits (retries with backoff, concurrency limits per account/region).

### 5.3 Usability
- **NFR-20** Onboarding wizard: connect first account in < 5 minutes.
- **NFR-21** Works offline on previously synced data.
- **NFR-22** Layouts adapt to the window size (no horizontal scrolling on narrow windows).
- **NFR-23** One standard for messages that need the user's attention (callouts with a tone: action, warning, danger, success, info; a title; optional actions and dismiss), used across the app instead of ad-hoc banners.

### 5.4 Cost of running the app
- **NFR-30** Show the estimated AWS cost the app itself generates (Cost Explorer requests, CloudWatch metric calls, S3 GET for CUR) and keep defaults cheap.
- **NFR-31** Show the assistant's token usage and estimated cost per model this month, and keep it cheap by default (prompt caching, no repeated tool calls, a round limit per question, optional budget) — see FR-49.

## 6. Proposed architecture

```
┌────────────────────────── Tauri app ──────────────────────────┐
│ Web UI (React + TypeScript, Vite)                              │
│  · Dashboards (ECharts)  · Graph (React Flow)                   │
│  · Findings  · Chat panel                                      │
├──────────────── IPC (typed commands) ─────────────────────────┤
│ Rust core                                                       │
│  · auth: aws-config (SSO, profiles, assume role) + keyring      │
│  · collectors: aws-sdk-rust per service (inventory, CE, CUR)    │
│  · sync scheduler (tokio)                                       │
│  · analysis engine: cost queries, anomaly + waste rules         │
│  · graph builder: relationship inference                        │
│  · llm: provider trait, tool definitions, redaction layer       │
│  · storage: DuckDB (analytics, reads CUR Parquet natively)      │
│             + small encrypted store for app state               │
└────────────────────────────────────────────────────────────────┘
```

Design principles:
- **Rust core as a library crate** independent of Tauri, so a CLI/TUI could reuse it later.
- **Collectors and rules are pluggable** to make community contributions easy (fits the open-source model).
- The LLM never computes numbers; the engine does, the LLM explains.

## 7. Required AWS permissions (summary)

- Identity: `sts:GetCallerIdentity`, `sts:AssumeRole`
- Organizations: `organizations:ListAccounts`, `organizations:DescribeOrganization`
- Cost: `ce:Get*`, `ce:Describe*`, `ce:List*`, `bcm-data-exports:*Get/List*`, `s3:GetObject`/`s3:ListBucket` on the CUR bucket
- Inventory: AWS managed `ViewOnlyAccess` (or a scoped custom policy), `config:Select*`/`Describe*`, `resource-explorer-2:Search`
- Metrics (opt-in): `cloudwatch:GetMetricData`

Exact policy JSON to be published in `/policies`.

## 8. MVP scope

**In the MVP**
- Connections: SSO, profiles, Organizations discovery, access keys (with warning)
- Cost breakdown via Cost Explorer; CUR 2.0 ingestion
- Inventory for core services + dependency graph with cost overlay
- Anomaly detection + first set of waste rules
- LLM assistant with Ollama + one cloud provider, redaction on

**Not in the MVP / non-goals**
- Any write action on AWS resources (permanently out of scope)
- Team sharing / backend sync
- Multi-cloud (Azure, GCP)
- Budgets/alert notifications (email, Slack) — candidate for v2
- Mobile or web-hosted version

## 9. Milestones (suggested)

1. **M0 — Skeleton**: Tauri app, Rust core crate, DuckDB storage, SSO/profile login, account list.
2. **M1 — Costs**: Cost Explorer sync, dashboard, filters, caching, API-call counter.
3. **M2 — Inventory & graph**: collectors for core services, relationship inference, graph UI.
4. **M3 — CUR & cost overlay**: CUR setup wizard, Parquet ingestion, per-resource costs on graph.
5. **M4 — Findings**: anomaly detection, waste rules, findings UI.
6. **M5 — LLM**: provider trait, tools, redaction, chat + "Explain this".
7. **M6 — Release**: signing, auto-update, installers for 3 OSes, docs, IAM policy templates.

## 10. Acceptance criteria (MVP)

- A user can connect 3 accounts via SSO and see combined cost by service in under 10 minutes.
- The graph shows an ALB → target group → EC2 → EBS chain with per-resource monthly cost (CUR enabled).
- Seeding an unattached EBS volume produces a finding with correct estimated savings.
- With a cloud LLM and redaction on, the request log contains no real account IDs or ARNs.
- An automated test proves no AWS write API is called.

## 11. Open questions

- Default anomaly sensitivity (baseline window and threshold) — tune with real data during M4.
- Domain: `afinin.com` is listed for sale by a domain reseller; decide whether to buy it or use another domain (e.g. `afinin.dev`, or a GitHub Pages site).
- Reserve the name: create `juancarvajalsas/afinin` and publish placeholder `afinin` packages on crates.io and npm.

## 12. Resolved decisions log

| # | Decision |
|---|---|
| D-1 | Encryption at rest: key generated on first run and stored in the OS keychain; local database encrypted with it. Prototype DuckDB native encryption first, SQLCipher as fallback (NFR-2). |
| D-2 | First waste rules (MVP): unattached EBS volumes, unassociated Elastic IPs / public IPv4, idle load balancers, stopped EC2 with attached EBS, old EBS snapshots, idle NAT Gateways. CloudWatch-based rules come later (FR-31). |
| D-3 | Savings estimates use the customer's real rates from CUR when connected; otherwise AWS Price List API public prices, labeled as estimates (FR-34). |
| D-4 | Backup: single encrypted export file, importable on another machine (FR-53). |
| D-5 | Frontend: React + TypeScript. |
| D-6 | License: MIT OR Apache-2.0, DCO for contributions. |
| D-7 | Name: Afinín (`afinin`). "Sordín" was dropped because "Sordin" is a registered trademark of a hearing-protection company with marks in the software class. `afinin` is free on crates.io and npm; no conflicting trademark found. |
