# Changelog

## Unreleased — v0.1.0

First feature-complete version (MVP, milestones M0–M6).

- **Accounts**: IAM Identity Center (in-app sign-in), CLI profiles with assume-role chains,
  access keys (discouraged); workspaces; connection test with missing-permission report.
- **Costs** (Cost Explorer): incremental sync, overview with forecast, breakdowns by service,
  account, region, usage type and tag; daily/monthly; unblended/amortized/net; comparisons;
  CSV export; no double counting across management and member accounts.
- **CUR 2.0**: setup wizard with CloudFormation template and validation; incremental, manifest-aware
  ingestion; per-resource costs and real rates.
- **Inventory and graph**: 24 resource types across all enabled regions, inferred relationships,
  interactive graph with filters, search, cost overlay and tag-based applications.
- **Findings**: unattached EBS, idle Elastic IPs, idle load balancers, stopped instances with EBS,
  old snapshots, idle NAT gateways, NAT data processing; anomaly detection; top movers, data
  transfer and Savings Plans / RI coverage; dismiss and snooze.
- **Assistant** (optional): Ollama/OpenAI-compatible, Anthropic and Bedrock; read-only tools,
  cited figures, redaction, request log, "Explain this".
- **Data**: encrypted local database, retention setting, password-encrypted backup and restore.
- **Release**: signed auto-updates, installers for macOS, Linux and Windows, SBOMs, read-only
  IAM role as CloudFormation StackSet and Terraform module.
