# Read-only IAM policies

Policies Afinín needs in each connected account (see `requirement.md` §7).
Templates (CloudFormation StackSet, Terraform module) are added in M6.

- `afinin-readonly.json` — custom read-only policy for cost and identity (M0/M1 scope).
  Attach together with the AWS managed `ViewOnlyAccess` policy for inventory.
