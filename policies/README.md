# Read-only IAM policies

Policies Afinín needs in each connected account (see `requirement.md` §7).
Templates (CloudFormation StackSet, Terraform module) are added in M6.

- `afinin-readonly.json` — custom read-only policy covering identity, Cost Explorer and
  inventory. Alternatively, the AWS managed `ViewOnlyAccess` policy covers most inventory
  calls; Afinín reports any denied service as a warning instead of failing.

`apigateway:GET` is API Gateway's read-only action (its IAM model uses HTTP verbs).
