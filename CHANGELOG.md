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
- **Assistant quality and cost**: workspace context (accounts, tags, glossary, instructions;
  versioned), current-screen awareness, saved and resumable conversations, ratings, reference
  answers that guide similar questions and can be re-checked with expected values,
  missing-data reports with fixes, unverified-figure warnings; per-answer traces (rounds, tool
  calls, tokens, cache, latency) with waste warnings, Anthropic prompt caching, de-duplicated
  tool calls, per-model prices, cost estimates and a monthly budget.
- **Lightsail**: instances, databases, load balancers, disks and static IPs in the inventory and
  graph, bundle list prices, and waste findings (unattached static IPs and disks, stopped
  instances). The IAM policies gain the matching `lightsail:Get*` actions.
- **Graph**: Flow and Network layouts, untagged-resource filter, minimap.
- **More services**: Secrets Manager secrets, KMS customer-managed keys, ECR repositories,
  CloudWatch log groups, EFS file systems and Route 53 hosted zones, with public monthly prices
  and waste rules: unread secrets, disabled keys, idle repositories and untagged images without a
  lifecycle policy, log groups that never expire, file systems without mount targets, empty
  hosted zones. Only metadata is read (never secret values, key material, images or log events).
- **Infrastructure as code**: which tool made each resource (CloudFormation, CDK, SAM, Serverless,
  Beanstalk, Copilot, Amplify, Terraform, Pulumi) from stacks, tags and generated names, with
  confidence and evidence; "Managed by" graph filter, coverage card in Findings, assistant tool. The
  IAM policies gain `cloudformation:ListStacks` and `ListStackResources`.
- **Data**: encrypted local database, retention setting, password-encrypted backup and restore.
- **Release**: signed auto-updates, installers for macOS, Linux and Windows, SBOMs, read-only
  IAM role as CloudFormation StackSet and Terraform module.
