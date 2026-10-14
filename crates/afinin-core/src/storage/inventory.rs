// SPDX-License-Identifier: MIT OR Apache-2.0

use std::collections::{BTreeMap, HashSet};

use duckdb::params;
use serde::Serialize;

use super::Storage;
use crate::error::{Error, Result};
use crate::inventory::{Collected, Relation, RelationKind, Resource, ResourceType};
use crate::model::now_rfc3339;

/// Result of scanning one region of one account.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InventoryScan {
    pub account_id: String,
    pub scan_region: String,
    pub scanned_at: String,
    pub resource_count: i64,
    pub warnings: Vec<String>,
}

fn json<T: Serialize>(v: &T) -> Result<String> {
    serde_json::to_string(v).map_err(|e| Error::Other(e.to_string()))
}

fn kind_id(kind: RelationKind) -> String {
    serde_json::to_value(kind)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

impl Storage {
    /// Replaces everything previously found in `scan_region` for this account.
    pub(crate) fn replace_inventory(
        &self,
        source: &str,
        scan_region: &str,
        found: &Collected,
    ) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        for table in ["resources", "relations"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE source_account = ? AND scan_region = ?"),
                params![source, scan_region],
            )?;
        }
        {
            let mut app = tx.appender("resources")?;
            for r in &found.resources {
                app.append_row(params![
                    source,
                    scan_region,
                    r.arn,
                    r.resource_type.id(),
                    r.account_id,
                    r.region,
                    r.native_id,
                    r.name,
                    json(&r.tags)?,
                    json(&r.attrs)?
                ])?;
            }
            app.flush()?;
            let mut app = tx.appender("relations")?;
            for rel in &found.relations {
                app.append_row(params![
                    source,
                    scan_region,
                    rel.from,
                    rel.to,
                    kind_id(rel.kind)
                ])?;
            }
            app.flush()?;
        }
        tx.execute(
            "INSERT OR REPLACE INTO inventory_scans VALUES (?, ?, ?, ?, ?)",
            params![
                source,
                scan_region,
                now_rfc3339(),
                found.resources.len() as i64,
                json(&found.warnings)?
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Drops regions no longer scanned (e.g. removed from settings).
    pub(crate) fn prune_inventory(&self, source: &str, keep_regions: &[String]) -> Result<()> {
        let keep: HashSet<&str> = keep_regions.iter().map(String::as_str).collect();
        let scanned: Vec<String> = self
            .conn
            .prepare("SELECT scan_region FROM inventory_scans WHERE source_account = ?")?
            .query_map(params![source], |r| r.get(0))?
            .collect::<duckdb::Result<_>>()?;
        for region in scanned.iter().filter(|r| !keep.contains(r.as_str())) {
            for table in ["resources", "relations", "inventory_scans"] {
                self.conn.execute(
                    &format!("DELETE FROM {table} WHERE source_account = ? AND scan_region = ?"),
                    params![source, region],
                )?;
            }
        }
        Ok(())
    }

    /// All resources and relations of a workspace. A resource seen through
    /// several connected profiles appears once.
    pub fn workspace_inventory(
        &self,
        workspace_id: &str,
    ) -> Result<(Vec<Resource>, Vec<Relation>)> {
        let mut stmt = self.conn.prepare(
            "SELECT arn, any_value(resource_type), any_value(account_id), any_value(region),
                    any_value(native_id), any_value(name), any_value(tags), any_value(attrs)
             FROM resources
             WHERE source_account IN (SELECT id FROM accounts WHERE workspace_id = ?)
             GROUP BY arn ORDER BY arn",
        )?;
        let rows = stmt
            .query_map(params![workspace_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, String>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, String>(7)?,
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?;
        let mut resources = Vec::with_capacity(rows.len());
        for (arn, t, account_id, region, native_id, name, tags, attrs) in rows {
            let Some(resource_type) = ResourceType::from_id(&t) else {
                continue;
            };
            resources.push(Resource {
                arn,
                resource_type,
                account_id,
                region,
                native_id,
                name,
                tags: serde_json::from_str::<BTreeMap<_, _>>(&tags).unwrap_or_default(),
                attrs: serde_json::from_str::<BTreeMap<_, _>>(&attrs).unwrap_or_default(),
            });
        }

        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT from_arn, to_ref, kind FROM relations
             WHERE source_account IN (SELECT id FROM accounts WHERE workspace_id = ?)",
        )?;
        let relations = stmt
            .query_map(params![workspace_id], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                ))
            })?
            .collect::<duckdb::Result<Vec<_>>>()?
            .into_iter()
            .filter_map(|(from, to, kind)| {
                let kind = serde_json::from_value(serde_json::Value::String(kind)).ok()?;
                Some(Relation { from, to, kind })
            })
            .collect();
        Ok((resources, relations))
    }

    pub fn inventory_scans(&self, workspace_id: &str) -> Result<Vec<InventoryScan>> {
        let mut stmt = self.conn.prepare(
            "SELECT s.source_account, s.scan_region, s.scanned_at, s.resource_count, s.warnings
             FROM inventory_scans s JOIN accounts a ON a.id = s.source_account
             WHERE a.workspace_id = ? ORDER BY 1, 2",
        )?;
        let rows = stmt.query_map(params![workspace_id], |r| {
            let warnings: String = r.get(4)?;
            Ok(InventoryScan {
                account_id: r.get(0)?,
                scan_region: r.get(1)?,
                scanned_at: r.get(2)?,
                resource_count: r.get(3)?,
                warnings: serde_json::from_str(&warnings).unwrap_or_default(),
            })
        })?;
        Ok(rows.collect::<duckdb::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{RelationKind as K, Resource, ResourceType as T};
    use crate::profiles::ProfileKind;

    #[test]
    fn replace_load_and_dedupe() {
        let db = Storage::open_in_memory().unwrap();
        let ws = db.create_workspace("W").unwrap();
        let a = db.add_account(&ws.id, "A", "a", ProfileKind::Sso).unwrap();
        let b = db.add_account(&ws.id, "B", "b", ProfileKind::Sso).unwrap();

        let mut found = Collected::default();
        let mut r = Resource::new(
            T::SqsQueue,
            "arn:aws:sqs:us-east-1:1:q",
            "1",
            "us-east-1",
            "q",
        );
        r.tags.insert("app".into(), "x".into());
        found.resources.push(r);
        found.relate(
            "arn:aws:sqs:us-east-1:1:q",
            "arn:aws:lambda:us-east-1:1:function:f",
            K::Triggers,
        );
        found.warnings.push("sns in us-east-1: AccessDenied".into());

        db.replace_inventory(&a.id, "us-east-1", &found).unwrap();
        db.replace_inventory(&b.id, "us-east-1", &found).unwrap(); // same account via another profile
        db.replace_inventory(&a.id, "us-east-1", &found).unwrap(); // rescan replaces

        let (res, rels) = db.workspace_inventory(&ws.id).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].tags["app"], "x");
        assert_eq!(rels.len(), 1);
        assert_eq!(db.inventory_scans(&ws.id).unwrap()[0].warnings.len(), 1);

        db.prune_inventory(&a.id, &[]).unwrap();
        db.remove_account(&b.id).unwrap();
        assert!(db.workspace_inventory(&ws.id).unwrap().0.is_empty());
    }
}
