use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use serde::Serialize;

use crate::engine::lock::CheckoutLock;
use crate::engine::state::{StateContext, parse_apply_trailers};
use crate::git::GitAdapter;
use crate::store::{FEATURES_FILE, FeatureMatch, Store};

/// Serializable feature command report.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum FeatureReport {
    /// Feature inventory for the current store.
    Features {
        /// Features sorted by name.
        features: Vec<FeatureRow>,
        /// Process exit code for this result.
        exit: i32,
    },
    /// A feature block was appended to .features.yaml.
    FeatureAdded {
        /// Feature name.
        name: String,
        /// Owned path prefix.
        path: String,
        /// Description written to .features.yaml.
        description: String,
        /// Process exit code for this result.
        exit: i32,
    },
}

/// One row in feature list output.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FeatureRow {
    /// Feature name.
    pub name: String,
    /// Number of store patch paths owned by this feature.
    pub patches: usize,
    /// Highest apply-authored sequence seen since base.
    pub last_sequence: Option<usize>,
    /// Feature description.
    pub description: String,
}

impl FeatureReport {
    /// Returns the process exit code represented by the report.
    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Features { exit, .. } | Self::FeatureAdded { exit, .. } => *exit,
        }
    }
}

/// Builds the feature inventory report.
pub fn list(ctx: &StateContext) -> Result<FeatureReport> {
    let store = Store::load(&ctx.store_dir)?;
    let patch_counts = patch_counts(&store);
    let last_sequences = last_sequences(ctx)?;
    let features = store
        .features()
        .features
        .iter()
        .map(|(name, feature)| FeatureRow {
            name: name.clone(),
            patches: patch_counts.get(name).copied().unwrap_or(0),
            last_sequence: last_sequences.get(name).copied(),
            description: feature.description.clone(),
        })
        .collect();
    Ok(FeatureReport::Features { features, exit: 0 })
}

/// Appends a feature block to .features.yaml.
pub fn add(
    store_dir: impl Into<PathBuf>,
    name: &str,
    path: &str,
    description: Option<&str>,
) -> Result<FeatureReport> {
    let store_dir = store_dir.into();
    let _store_lock = CheckoutLock::acquire_store_repo(&store_dir)?;
    let description = description
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("feat: {name}"));
    let mut store = Store::load(&store_dir)?;
    store.add_feature(name, &description, vec![path.to_string()])?;
    store.save()?;
    Ok(FeatureReport::FeatureAdded {
        name: name.to_string(),
        path: path.to_string(),
        description,
        exit: 0,
    })
}

/// Renders a human feature report.
pub fn render_human(report: &FeatureReport) -> String {
    match report {
        FeatureReport::Features { features, .. } => {
            let mut out = String::new();
            out.push_str(&format!(
                "{:<24} {:>7} {:>5}  {}\n",
                "feature", "patches", "last", "description"
            ));
            for feature in features {
                out.push_str(&format!(
                    "{:<24} {:>7} {:>5}  {}\n",
                    feature.name,
                    feature.patches,
                    feature
                        .last_sequence
                        .map(|seq| seq.to_string())
                        .unwrap_or_default(),
                    feature.description
                ));
            }
            out
        }
        FeatureReport::FeatureAdded { name, path, .. } => {
            format!("created feature \"{name}\" (path: {path}) in {FEATURES_FILE}\n")
        }
    }
}

/// Renders a JSON feature report.
pub fn render_json(report: &FeatureReport) -> Result<String> {
    Ok(serde_json::to_string(report)?)
}

fn patch_counts(store: &Store) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for path in store.patches().keys() {
        if let FeatureMatch::Matched { feature, .. } = store.match_path(path) {
            *counts.entry(feature).or_insert(0) += 1;
        }
    }
    counts
}

fn last_sequences(ctx: &StateContext) -> Result<BTreeMap<String, usize>> {
    let state = crate::engine::state::resolve(ctx)?;
    let git = GitAdapter::new(&ctx.checkout);
    let mut sequences = BTreeMap::new();
    let range = format!("{}..HEAD", state.base.sha);
    for commit in git.first_parent_commits(Some(&range), None)? {
        if parse_apply_trailers(&git.commit_trailers(&commit)?)?.is_none() {
            continue;
        }
        if let Some((feature, seq)) = subject_sequence(&git.commit_subject(&commit)?) {
            let entry = sequences.entry(feature).or_insert(0);
            *entry = (*entry).max(seq);
        }
    }
    Ok(sequences)
}

fn subject_sequence(subject: &str) -> Option<(String, usize)> {
    let rest = subject.strip_prefix("feat: ")?;
    if let Some((feature, seq)) = rest.rsplit_once(" #") {
        return seq
            .parse::<usize>()
            .ok()
            .map(|seq| (feature.to_string(), seq));
    }
    Some((rest.to_string(), 1))
}
