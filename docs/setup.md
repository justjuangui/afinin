# Setting up Afinín

## 1. Give Afinín read-only access

Afinín uses your existing AWS CLI profiles (`~/.aws/config`) and only ever calls read APIs.

**Recommended: IAM Identity Center (SSO).** Create a permission set with the AWS managed
`ViewOnlyAccess` policy plus an inline policy from
[`policies/afinin-readonly.json`](../policies/afinin-readonly.json), assign it to the accounts
you want to analyze, then configure a profile:

```sh
aws configure sso    # creates [profile …] with sso_session, sso_account_id, sso_role_name
```

**Many accounts in an AWS Organization:** deploy the read-only role everywhere with the
StackSet template [`policies/cloudformation/afinin-readonly-role.yaml`](../policies/cloudformation/afinin-readonly-role.yaml)
or the Terraform module [`policies/terraform/afinin-readonly-role`](../policies/terraform/afinin-readonly-role),
then add one profile per account with `role_arn` and `source_profile`:

```ini
[profile acme-prod]
role_arn = arn:aws:iam::111122223333:role/AfininReadOnly
source_profile = acme-sso
region = us-east-1
```

Long-lived access keys work but are discouraged.

## 2. Connect accounts

Create a workspace, then **Add account** and pick the profile. **Test** checks the identity and
lists any missing read permissions. For SSO profiles, **Sign in** opens the AWS approval page;
the token is stored in the OS keychain.

Connect the **management (payer) account** to see costs for the whole organization. Connecting
member accounts too is fine: Afinín counts each account's costs once.

## 3. Sync

**Sync now** downloads Cost Explorer data and scans resources in every enabled region (or the
regions chosen in Settings). Cost Explorer charges $0.01 per request: a first sync makes a few
dozen requests, later syncs about six per account. Settings shows what Afinín has cost you this
month. Resource discovery uses free `Describe`/`List` calls.

## 4. Optional: CUR 2.0 for per-resource costs

**Accounts → Set up CUR** walks you through creating a CUR 2.0 export in the management account
with [`policies/cur-export.yaml`](../policies/cur-export.yaml) and validates it. With CUR, the
graph shows each resource's own cost, costs can be broken down by resource, and savings use
your actual rates.

## 5. Optional: assistant

**Settings → Assistant** connects a local model (Ollama, LM Studio) or a cloud provider
(Anthropic, Amazon Bedrock, any OpenAI-compatible API). See [privacy.md](privacy.md).

- **Price and budget**: enter the model's price per million tokens (from your provider's
  pricing page) to see the estimated cost of each answer, of re-checks and of the month. An
  optional monthly budget stops the assistant once it is reached.
- **Workspace context** (the workspace's **Context** tab): say what each account is for, what
  your tags and terms mean and how you want answers. Answers use your terms instead of guessing.
- **Reference answers**: mark answers you trust with **Use as reference**. Similar questions
  follow the same queries, and **History → Reference answers → Re-check all** asks them again
  after a sync, a context edit or a model change. Add expected values (figures for closed
  periods) to make a re-check fail when an answer goes wrong.
- **Trace** under each answer shows its rounds, queries, tokens, cache use and cost, with
  warnings when a question wasted tokens.
