# Read-only IAM policies

Policies Afinín needs in each connected account (see `requirement.md` §7).
Templates (CloudFormation StackSet, Terraform module) are added in M6.

- `afinin-readonly.json` — custom read-only policy covering identity, Cost Explorer and
  inventory. Alternatively, the AWS managed `ViewOnlyAccess` policy covers most inventory
  calls; Afinín reports any denied service as a warning instead of failing.

`apigateway:GET` is API Gateway's read-only action (its IAM model uses HTTP verbs).

- `cur-export.yaml` — CloudFormation that creates an S3 bucket and a CUR 2.0 Data Export
  (daily, Parquet, resource IDs). Deploy it yourself in the management account, `us-east-1`;
  Afinín only reads it. The connected role then needs `s3:ListBucket` and `s3:GetObject` on
  that bucket (the setup wizard shows the exact statement).
