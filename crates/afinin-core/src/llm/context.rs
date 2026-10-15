// SPDX-License-Identifier: MIT OR Apache-2.0

//! Layered context for the assistant: built-in AWS billing notes, the
//! workspace context the user writes about their business, and preferences.
//! The workspace context is versioned so a past answer can be explained by
//! the revision it used.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{Error, Result};

/// Longest text accepted per section, so the context stays a small part of
/// every request.
pub const MAX_SECTION_CHARS: usize = 4000;
const MAX_ACCOUNT_NOTE_CHARS: usize = 500;
/// Limits for key → meaning lists (tags, glossary).
pub const MAX_ENTRIES: usize = 50;
const MAX_KEY_CHARS: usize = 120;
const MAX_MEANING_CHARS: usize = 500;

/// One key → meaning row: a tag (`team`, `env=prod`) or a business term.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Entry {
    pub key: String,
    pub meaning: String,
}

/// What the user tells the assistant about the business behind the AWS
/// accounts of one workspace.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceContext {
    /// What the business does: products, applications, environments.
    pub business: String,
    /// Linked AWS account ID → what it is used for.
    pub accounts: BTreeMap<String, String>,
    /// What tag keys and values mean, in the user's order.
    #[serde(deserialize_with = "entries")]
    pub tags: Vec<Entry>,
    /// Business terms and how they map to AWS, in the user's order.
    #[serde(deserialize_with = "entries")]
    pub glossary: Vec<Entry>,
    /// How answers should be given.
    pub instructions: String,
}

/// Accepts a list of entries, or the free text saved by the first version
/// (one `key: meaning` or `key = meaning` per line).
fn entries<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Vec<Entry>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        List(Vec<Entry>),
        Text(String),
    }
    Ok(match Stored::deserialize(d)? {
        Stored::List(list) => list,
        Stored::Text(text) => text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|line| {
                let line = line.trim_start_matches(['-', '*', ' ']);
                let split = [": ", " = ", "=", ":"]
                    .iter()
                    .find_map(|sep| line.split_once(sep));
                let (key, meaning) = split.unwrap_or((line, ""));
                Entry {
                    key: key.trim().trim_matches(['`', '"']).to_owned(),
                    meaning: meaning.trim().to_owned(),
                }
            })
            .collect(),
    })
}

/// The saved context with its revision (0 = never saved).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct StoredContext {
    pub context: WorkspaceContext,
    pub revision: i64,
    pub updated_at: Option<String>,
}

fn too_long(what: &str, limit: usize) -> Error {
    Error::Other(format!("{what} is longer than {limit} characters"))
}

/// Trims rows, drops incomplete ones and rejects duplicates and oversized rows.
fn normalize_entries(section: &str, list: Vec<Entry>) -> Result<Vec<Entry>> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in list {
        let (key, meaning) = (e.key.trim().to_owned(), e.meaning.trim().to_owned());
        if key.is_empty() || meaning.is_empty() {
            continue;
        }
        if key.chars().count() > MAX_KEY_CHARS {
            return Err(too_long(
                &format!("the {section} key \"{key}\""),
                MAX_KEY_CHARS,
            ));
        }
        if meaning.chars().count() > MAX_MEANING_CHARS {
            return Err(too_long(
                &format!("the meaning of {section} \"{key}\""),
                MAX_MEANING_CHARS,
            ));
        }
        if !seen.insert(key.to_lowercase()) {
            return Err(Error::Other(format!("{section} \"{key}\" appears twice")));
        }
        out.push(Entry { key, meaning });
    }
    if out.len() > MAX_ENTRIES {
        return Err(Error::Other(format!(
            "{section} has more than {MAX_ENTRIES} entries"
        )));
    }
    Ok(out)
}

impl WorkspaceContext {
    /// Trims every field, drops empty notes and rows, and enforces limits.
    pub fn normalized(mut self) -> Result<Self> {
        for (name, text) in [
            ("business", &mut self.business),
            ("instructions", &mut self.instructions),
        ] {
            *text = text.trim().to_owned();
            if text.chars().count() > MAX_SECTION_CHARS {
                return Err(too_long(&format!("the {name} section"), MAX_SECTION_CHARS));
            }
        }
        self.tags = normalize_entries("tag", self.tags)?;
        self.glossary = normalize_entries("term", self.glossary)?;
        let mut accounts = BTreeMap::new();
        for (id, note) in self.accounts {
            let (id, note) = (id.trim().to_owned(), note.trim().to_owned());
            if id.is_empty() || note.is_empty() {
                continue;
            }
            if note.chars().count() > MAX_ACCOUNT_NOTE_CHARS {
                return Err(too_long(
                    &format!("the note for account {id}"),
                    MAX_ACCOUNT_NOTE_CHARS,
                ));
            }
            accounts.insert(id, note);
        }
        self.accounts = accounts;
        Ok(self)
    }

    pub fn is_empty(&self) -> bool {
        self.business.is_empty()
            && self.accounts.is_empty()
            && self.tags.is_empty()
            && self.glossary.is_empty()
            && self.instructions.is_empty()
    }

    /// Markdown for the system prompt. `account_name` labels account IDs.
    pub fn render(&self, account_name: impl Fn(&str) -> Option<String>) -> String {
        let mut out = String::new();
        let mut section = |title: &str, body: &str| {
            if !body.is_empty() {
                out.push_str(&format!("## {title}\n{body}\n\n"));
            }
        };
        let list = |items: Vec<String>| items.join("\n");
        section("Business", &self.business);
        section(
            "AWS accounts",
            &list(
                self.accounts
                    .iter()
                    .map(|(id, note)| match account_name(id) {
                        Some(name) => format!("- {id} ({name}): {note}"),
                        None => format!("- {id}: {note}"),
                    })
                    .collect(),
            ),
        );
        section(
            "Tags",
            &list(
                self.tags
                    .iter()
                    .map(|e| format!("- `{}`: {}", e.key, e.meaning))
                    .collect(),
            ),
        );
        section(
            "Glossary",
            &list(
                self.glossary
                    .iter()
                    .map(|e| format!("- \"{}\": {}", e.key, e.meaning))
                    .collect(),
            ),
        );
        section("Instructions for answers", &self.instructions);
        out.trim_end().to_owned()
    }
}

/// What the user is looking at when asking, so "this view" or "here" has a
/// precise meaning. Reported by the UI; rendered into the prompt.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenContext {
    /// The screen, e.g. "Costs" or "Inventory".
    pub view: String,
    /// What it shows, in the screen's order: period, metric, filters,
    /// selected resource…
    pub details: Vec<Entry>,
}

impl ScreenContext {
    /// Markdown for the prompt; empty when the UI reported nothing.
    pub fn render(&self) -> String {
        let view = self.view.trim();
        if view.is_empty() {
            return String::new();
        }
        let mut out = format!("Screen: {view}");
        for d in self.details.iter().take(MAX_ENTRIES) {
            let (k, v) = (d.key.trim(), d.meaning.trim());
            if !k.is_empty() && !v.is_empty() {
                let v: String = v.chars().take(MAX_MEANING_CHARS).collect();
                out.push_str(&format!("\n- {k}: {v}"));
            }
        }
        out
    }
}

/// Built-in notes about AWS billing that apply to every workspace.
pub const AWS_BILLING_NOTES: &str = "\
- \"EC2 - Other\" is a separate Cost Explorer service holding EBS volumes and snapshots, NAT gateways, Elastic IPs and EC2 data transfer. \
When the user asks about \"EC2\" costs, say whether you included it.
- Unblended cost is what was billed when usage happened; amortized spreads upfront Savings Plan and Reserved Instance fees over the period; net unblended is after discounts.
- Forecasts, percentages, shares and Savings Plans or RI coverage cannot be added together.
- Some global services report no region.
- Totals include every charge type: credits and refunds are negative amounts, tax is its own \"Tax\" service, and AWS \
Support and Marketplace purchases are their own services. Mention them when they move a total noticeably.
- In per-resource (CUR) breakdowns, charges AWS doesn't tie to a resource (support, tax, some data transfer) have no \
resource ID: don't attribute them to a resource.";

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: &str, meaning: &str) -> Entry {
        Entry {
            key: key.into(),
            meaning: meaning.into(),
        }
    }

    #[test]
    fn normalizes_and_renders_sections_in_order() {
        let ctx = WorkspaceContext {
            business: "  Online store selling furniture.  ".into(),
            accounts: BTreeMap::from([
                (
                    "111111111111".into(),
                    "Production (checkout, catalog)".into(),
                ),
                ("222222222222".into(), "   ".into()),
            ]),
            tags: vec![
                entry(" team ", "owning squad"),
                entry("env", ""),
                entry("env=prod", "production"),
            ],
            glossary: vec![entry("The data lake", "S3 buckets prefixed lake-")],
            instructions: "Report in amortized cost.".into(),
        }
        .normalized()
        .unwrap();
        assert_eq!(ctx.accounts.len(), 1, "empty notes are dropped");
        assert_eq!(ctx.tags.len(), 2, "rows without a meaning are dropped");
        let text = ctx.render(|id| (id == "111111111111").then(|| "Prod".to_owned()));
        assert_eq!(
            text,
            "## Business\nOnline store selling furniture.\n\n\
             ## AWS accounts\n- 111111111111 (Prod): Production (checkout, catalog)\n\n\
             ## Tags\n- `team`: owning squad\n- `env=prod`: production\n\n\
             ## Glossary\n- \"The data lake\": S3 buckets prefixed lake-\n\n\
             ## Instructions for answers\nReport in amortized cost."
        );
        assert!(WorkspaceContext::default().is_empty());
    }

    #[test]
    fn rejects_oversized_and_duplicate_entries() {
        let long = WorkspaceContext {
            business: "x".repeat(MAX_SECTION_CHARS + 1),
            ..Default::default()
        };
        assert!(long.normalized().is_err());
        let dup = WorkspaceContext {
            glossary: vec![entry("Search", "OpenSearch"), entry("search", "Kendra")],
            ..Default::default()
        };
        assert!(dup.normalized().is_err());
    }

    #[test]
    fn renders_the_current_screen() {
        let screen = ScreenContext {
            view: "Costs".into(),
            details: vec![
                entry("period", "2026-10-01 to 2026-10-13 (month to date)"),
                entry("accounts", ""),
                entry("group by", "service"),
            ],
        };
        assert_eq!(
            screen.render(),
            "Screen: Costs\n- period: 2026-10-01 to 2026-10-13 (month to date)\n- group by: service"
        );
        assert_eq!(ScreenContext::default().render(), "");
    }

    #[test]
    fn reads_free_text_saved_by_the_first_version() {
        let ctx: WorkspaceContext = serde_json::from_str(
            r#"{"tags": "`team`: owning squad\nenv = prod | staging\n\n- app", "glossary": "\"Search\": OpenSearch in us-east-1"}"#,
        )
        .unwrap();
        assert_eq!(
            ctx.tags,
            [
                entry("team", "owning squad"),
                entry("env", "prod | staging"),
                entry("app", "")
            ]
        );
        assert_eq!(ctx.glossary, [entry("Search", "OpenSearch in us-east-1")]);
    }
}
