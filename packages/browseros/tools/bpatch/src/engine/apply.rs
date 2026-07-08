use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};

use crate::engine::progress::ProgressEvent;
use crate::engine::state::{
    DriftFile, DriftSource, StateContext, format_apply_trailers, parse_apply_trailers,
    unassigned_feature_name,
};
use crate::git::{GitAdapter, TreeDiffEntry};
use crate::process::Git;
use crate::store::{FeatureMatch, Store};

/// Options controlling a same-base apply run.
#[derive(Clone, Copy, Debug, Default)]
pub struct ApplyOptions {
    /// Fast-forward the store repository before resolving state.
    pub pull: bool,
}

/// Result of planning and applying the current store state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ApplyOutcome {
    /// The checkout already matches the current store tree.
    Converged(ConvergedApply),
    /// Store changes were materialized and feature commits were authored.
    Applied(AppliedApply),
    /// Store and checkout base pins differ; Task 6 owns this path.
    BaseMismatch(BaseMismatch),
    /// The checkout has committed or tracked uncommitted drift.
    Drift(DriftApply),
}

/// No-op same-base apply result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConvergedApply {
    /// Store repository HEAD applied by this state.
    pub store_rev: String,
    /// Short store repository HEAD.
    pub store_short_rev: String,
    /// Target tree built from the store patches.
    pub target_tree: String,
}

/// Successful apply result with authored commits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppliedApply {
    /// Store repository HEAD used for trailers.
    pub store_rev: String,
    /// Short store repository HEAD.
    pub store_short_rev: String,
    /// Chromium base commit used for convergence.
    pub base: String,
    /// Human base display string.
    pub base_display: String,
    /// Applied store revision before this run, when any.
    pub previous_store_short_rev: Option<String>,
    /// Files changed between the applied tree and target tree.
    pub files_changed: usize,
    /// Store-managed file count loaded from the store.
    pub store_managed_files: usize,
    /// Final target tree carried by the batch's last commit.
    pub target_tree: String,
    /// Feature commits authored by this run.
    pub commits: Vec<AuthoredCommit>,
}

/// Checkout/store base mismatch details.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BaseMismatch {
    /// Base commit recorded by the checkout's current applied state.
    pub checkout_base: String,
    /// Base commit pinned in .store.yaml.
    pub store_base: String,
}

/// Drift refusal result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DriftApply {
    /// Drift files reported by state resolution.
    pub files: Vec<DriftFile>,
}

/// Input for reusable commit-tree authoring.
pub struct AuthorCommitsInput<'a> {
    /// Chromium checkout root.
    pub checkout: &'a Path,
    /// Loaded patch store used for feature grouping.
    pub store: &'a Store,
    /// Chromium base commit to write into trailers.
    pub base: &'a str,
    /// Tree currently represented by the parent apply state.
    pub applied_tree: &'a str,
    /// Final tree that the authored commit chain must reach.
    pub target_tree: &'a str,
    /// Store repository commit to write into trailers.
    pub store_rev: &'a str,
    /// Parent commit for the first authored feature commit.
    pub parent_commit: &'a str,
    /// Files changed between `applied_tree` and `target_tree`.
    pub delta: &'a [TreeDiffEntry],
}

/// Commit authored for one feature group.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoredCommit {
    /// Feature name, or `(unassigned)` for unmatched files.
    pub feature: String,
    /// Sequence number derived from previous apply-authored commits.
    pub seq: usize,
    /// Full commit sha.
    pub sha: String,
    /// Short commit sha.
    pub short_sha: String,
    /// Commit subject written by the authoring chain.
    pub subject: String,
}

/// Object-only authored commit chain ready to become HEAD.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AuthoredCommitChain {
    /// Feature commits authored by this run.
    pub commits: Vec<AuthoredCommit>,
    /// Full sha of the last authored commit.
    pub final_sha: String,
}

struct CommitGroup {
    feature: String,
    seq: usize,
    subject: String,
    files: Vec<TreeDiffEntry>,
}

/// Runs same-base convergence against the current store.
pub fn apply(
    ctx: &StateContext,
    options: ApplyOptions,
    progress: &mut dyn FnMut(ProgressEvent<'_>),
) -> Result<ApplyOutcome> {
    if options.pull {
        pull_store(&ctx.store_dir, progress)?;
    }

    let state = crate::engine::state::resolve(ctx)?;
    let store = Store::load(&ctx.store_dir)?;
    if store.metadata().base_commit != state.base.sha {
        return Ok(ApplyOutcome::BaseMismatch(BaseMismatch {
            checkout_base: state.base.sha,
            store_base: store.metadata().base_commit.clone(),
        }));
    }

    let checkout = GitAdapter::new(&ctx.checkout);
    let target_tree =
        build_target_tree(&checkout, &ctx.store_dir, &store, &state.base.sha, progress)
            .context("building target tree from store patches")?;

    checkout.refresh_index()?;
    if state.head_tree == target_tree && !checkout.diff_index_has_changes("HEAD")? {
        return Ok(ApplyOutcome::Converged(ConvergedApply {
            store_rev: state.store.head_rev,
            store_short_rev: state.store.short_head_rev,
            target_tree,
        }));
    }

    if !state.drift.is_clean() {
        return Ok(ApplyOutcome::Drift(DriftApply {
            files: state.drift.files().to_vec(),
        }));
    }

    let applied_tree = state
        .applied
        .as_ref()
        .map(|applied| applied.tree.as_str())
        .unwrap_or(&state.head_tree);
    let delta = checkout.diff_tree_name_status(applied_tree, &target_tree)?;
    let collisions = untracked_add_collisions(&checkout, &delta)?;
    if !collisions.is_empty() {
        return Ok(ApplyOutcome::Drift(DriftApply { files: collisions }));
    }

    let chain = author_feature_commits(
        AuthorCommitsInput {
            checkout: &ctx.checkout,
            store: &store,
            base: &state.base.sha,
            applied_tree,
            target_tree: &target_tree,
            store_rev: &state.store.head_rev,
            parent_commit: &state.head_rev,
            delta: &delta,
        },
        progress,
    )?;

    progress(ProgressEvent::Start {
        phase: "materialize",
        total: Some(delta.len()),
    });
    checkout
        .materialize_tree_delta(applied_tree, &target_tree)
        .with_context(|| {
            format!(
                "materializing target tree failed; recover with `git read-tree -m -u {applied_tree} {target_tree}`"
            )
        })?;
    progress(ProgressEvent::End {
        phase: "materialize",
    });
    finalize_head(
        &ctx.checkout,
        &state.head_rev,
        &chain.final_sha,
        &target_tree,
    )?;

    Ok(ApplyOutcome::Applied(AppliedApply {
        store_rev: state.store.head_rev,
        store_short_rev: state.store.short_head_rev,
        base: state.base.sha,
        base_display: state.base.display,
        previous_store_short_rev: state.applied.map(|applied| applied.short_store_rev),
        files_changed: delta.len(),
        store_managed_files: store.patches().len(),
        target_tree,
        commits: chain.commits,
    }))
}

/// Authors grouped feature commits with commit-tree without moving refs.
pub fn author_feature_commits(
    input: AuthorCommitsInput<'_>,
    progress: &mut dyn FnMut(ProgressEvent<'_>),
) -> Result<AuthoredCommitChain> {
    if input.delta.is_empty() {
        return Ok(AuthoredCommitChain {
            commits: Vec::new(),
            final_sha: input.parent_commit.to_string(),
        });
    }

    let git = GitAdapter::new(input.checkout);
    let groups = plan_commit_groups(
        &git,
        input.store,
        input.base,
        input.parent_commit,
        input.delta,
    )?;
    let git_dir = git_dir(git.process())?;
    let temp = tempfile::Builder::new()
        .prefix("bpatch-author-index-")
        .tempfile_in(git_dir)?;
    let index_path = temp.into_temp_path();
    fs::remove_file(&index_path)?;
    let indexed = git
        .process()
        .with_env("GIT_INDEX_FILE", index_path.as_os_str().to_os_string());

    progress(ProgressEvent::Start {
        phase: "commit",
        total: Some(groups.len()),
    });

    let mut authored = Vec::with_capacity(groups.len());
    let mut current_tree = input.applied_tree.to_string();
    let mut parent = input.parent_commit.to_string();
    let last_index = groups.len().saturating_sub(1);

    for (index, group) in groups.iter().enumerate() {
        indexed.run(&["read-tree", &current_tree])?;
        let index_info = index_info_for_group(&git, input.target_tree, &group.files)?;
        indexed.run_with_stdin(&["update-index", "--index-info"], index_info.as_bytes())?;
        let next_tree = indexed.run_str(&["write-tree"])?;
        let tree_trailer = (index == last_index).then_some(input.target_tree);
        let message = commit_message(&group.subject, input.store_rev, input.base, tree_trailer);
        let sha = git.process().run_with_stdin(
            &["commit-tree", &next_tree, "-p", &parent],
            message.as_bytes(),
        )?;
        let sha = String::from_utf8(sha)
            .context("commit-tree output was not UTF-8")?
            .trim()
            .to_string();
        let short_sha = git.short_rev(&sha)?;
        authored.push(AuthoredCommit {
            feature: group.feature.clone(),
            seq: group.seq,
            sha: sha.clone(),
            short_sha,
            subject: group.subject.clone(),
        });
        parent = sha;
        current_tree = next_tree;
        progress(ProgressEvent::Tick {
            phase: "commit",
            done: index + 1,
            total: Some(groups.len()),
            item: Some(&group.feature),
        });
    }

    if current_tree != input.target_tree {
        bail!(
            "authored commit chain ended at tree {current_tree}, expected {}",
            input.target_tree
        );
    }
    progress(ProgressEvent::End { phase: "commit" });

    Ok(AuthoredCommitChain {
        commits: authored,
        final_sha: parent,
    })
}

/// Moves HEAD to an already-authored chain tip and syncs the real index.
pub fn finalize_head(
    checkout: impl AsRef<Path>,
    old_head: &str,
    final_sha: &str,
    final_tree: &str,
) -> Result<()> {
    let git = GitAdapter::new(checkout.as_ref());
    git.process()
        .run(&["update-ref", "HEAD", final_sha, old_head])
        .with_context(|| {
            format!(
                "finalizing HEAD failed; recover with `git update-ref HEAD {final_sha} {old_head}`"
            )
        })?;
    git.process()
        .run(&["read-tree", final_tree])
        .with_context(|| {
            format!("syncing index failed; recover with `git read-tree {final_tree}`")
        })?;
    git.refresh_index()
        .context("refreshing index failed; recover with `git update-index -q --refresh`")?;
    Ok(())
}

pub(crate) fn untracked_add_collisions(
    git: &GitAdapter,
    delta: &[TreeDiffEntry],
) -> Result<Vec<DriftFile>> {
    let added = delta
        .iter()
        .filter(|entry| entry.status == "A")
        .map(|entry| entry.path.clone())
        .collect::<BTreeSet<_>>();
    if added.is_empty() {
        return Ok(Vec::new());
    }

    git.refresh_index()?;
    let untracked = untracked_paths(&git.status_porcelain_z()?)?;
    Ok(added
        .into_iter()
        .filter(|path| untracked.contains(path))
        .map(|path| DriftFile {
            path,
            status: "??".to_string(),
            source: DriftSource::Uncommitted,
            annotation: "untracked, would be overwritten".to_string(),
        })
        .collect())
}

fn untracked_paths(bytes: &[u8]) -> Result<BTreeSet<PathBuf>> {
    let mut parts = bytes.split(|byte| *byte == 0);
    let mut paths = BTreeSet::new();
    while let Some(record) = parts.next() {
        if record.is_empty() {
            break;
        }
        let text = std::str::from_utf8(record)?;
        if text.len() < 4 {
            continue;
        }
        let status = &text[..2];
        let path = &text[3..];
        if status == "??" {
            paths.insert(PathBuf::from(path));
        } else if status.starts_with('R') || status.starts_with('C') {
            let _old_path = parts.next();
        }
    }
    Ok(paths)
}

fn pull_store(store_dir: &Path, progress: &mut dyn FnMut(ProgressEvent<'_>)) -> Result<()> {
    progress(ProgressEvent::Start {
        phase: "pull",
        total: None,
    });
    Git::new(store_dir).run(&["pull", "--ff-only"])?;
    progress(ProgressEvent::End { phase: "pull" });
    Ok(())
}

fn build_target_tree(
    git: &GitAdapter,
    store_dir: &Path,
    store: &Store,
    base: &str,
    progress: &mut dyn FnMut(ProgressEvent<'_>),
) -> Result<String> {
    let git_dir = git_dir(git.process())?;
    let temp = tempfile::Builder::new()
        .prefix("bpatch-tree-index-")
        .tempfile_in(git_dir)?;
    let index_path = temp.into_temp_path();
    fs::remove_file(&index_path)?;
    let indexed = git
        .process()
        .with_env("GIT_INDEX_FILE", index_path.as_os_str().to_os_string());
    indexed.run(&["read-tree", base])?;

    progress(ProgressEvent::Start {
        phase: "tree",
        total: Some(store.patches().len()),
    });
    for (index, patch) in store.patches().values().enumerate() {
        let patch_path = store_dir.join(&patch.path);
        let patch_arg = path_arg(&patch_path)?;
        indexed.run(&["apply", "--cached", "--whitespace=nowarn", patch_arg])?;
        progress(ProgressEvent::Tick {
            phase: "tree",
            done: index + 1,
            total: Some(store.patches().len()),
            item: Some(&patch.path),
        });
    }
    let tree = indexed.run_str(&["write-tree"])?;
    progress(ProgressEvent::End { phase: "tree" });
    Ok(tree)
}

fn plan_commit_groups(
    git: &GitAdapter,
    store: &Store,
    base: &str,
    parent_commit: &str,
    delta: &[TreeDiffEntry],
) -> Result<Vec<CommitGroup>> {
    let mut grouped = BTreeMap::<String, Vec<TreeDiffEntry>>::new();
    for entry in delta {
        let path = entry
            .path
            .to_str()
            .ok_or_else(|| anyhow!("diff path is not UTF-8: {}", entry.path.display()))?;
        let feature = match store.match_path(path) {
            FeatureMatch::Matched { feature, .. } => feature,
            FeatureMatch::Unmatched { .. } => unassigned_feature_name().to_string(),
        };
        grouped.entry(feature).or_default().push(entry.clone());
    }

    let existing = existing_subject_counts(git, base, parent_commit)?;
    grouped
        .into_iter()
        .map(|(feature, files)| {
            let subject_base = subject_base(&feature);
            let seq = existing.get(&subject_base).copied().unwrap_or(0) + 1;
            let subject = if seq == 1 {
                subject_base.clone()
            } else {
                format!("{subject_base} #{seq}")
            };
            Ok(CommitGroup {
                feature,
                seq,
                subject,
                files,
            })
        })
        .collect()
}

fn existing_subject_counts(
    git: &GitAdapter,
    base: &str,
    parent_commit: &str,
) -> Result<BTreeMap<String, usize>> {
    let mut counts = BTreeMap::new();
    let range = format!("{base}..{parent_commit}");
    for commit in git.first_parent_commits(Some(&range), None)? {
        if parse_apply_trailers(&git.commit_trailers(&commit)?)?.is_none() {
            continue;
        }
        let subject = git.commit_subject(&commit)?;
        if let Some(base_subject) = apply_subject_base(&subject) {
            *counts.entry(base_subject).or_insert(0) += 1;
        }
    }
    Ok(counts)
}

fn subject_base(feature: &str) -> String {
    if feature == unassigned_feature_name() {
        "chore: unassigned store patches".to_string()
    } else {
        format!("feat: {feature}")
    }
}

fn apply_subject_base(subject: &str) -> Option<String> {
    let without_digits = subject.trim_end_matches(|ch: char| ch.is_ascii_digit());
    let base = without_digits.strip_suffix(" #").unwrap_or(subject);
    if base.starts_with("feat: ") || base == "chore: unassigned store patches" {
        return Some(base.to_string());
    }
    None
}

fn index_info_for_group(
    git: &GitAdapter,
    target_tree: &str,
    files: &[TreeDiffEntry],
) -> Result<String> {
    let mut out = String::new();
    for entry in files {
        if let Some(old_path) = &entry.old_path {
            append_index_info_line(&mut out, git, target_tree, old_path)?;
        }
        append_index_info_line(&mut out, git, target_tree, &entry.path)?;
    }
    Ok(out)
}

fn append_index_info_line(
    out: &mut String,
    git: &GitAdapter,
    target_tree: &str,
    path: &Path,
) -> Result<()> {
    let path_arg = path_arg(path)?;
    let raw = git
        .process()
        .run(&["ls-tree", "-z", target_tree, "--", path_arg])?;
    if raw.is_empty() {
        out.push_str("0 0000000000000000000000000000000000000000\t");
        out.push_str(path_arg);
        out.push('\n');
        return Ok(());
    }

    let first = raw
        .split(|byte| *byte == 0)
        .find(|field| !field.is_empty())
        .ok_or_else(|| anyhow!("ls-tree returned empty record for {path_arg}"))?;
    let record = std::str::from_utf8(first).context("ls-tree output was not UTF-8")?;
    let (metadata, _) = record
        .split_once('\t')
        .ok_or_else(|| anyhow!("malformed ls-tree record for {path_arg}"))?;
    let mut parts = metadata.split_whitespace();
    let mode = parts
        .next()
        .ok_or_else(|| anyhow!("missing mode in ls-tree record for {path_arg}"))?;
    let _kind = parts
        .next()
        .ok_or_else(|| anyhow!("missing kind in ls-tree record for {path_arg}"))?;
    let oid = parts
        .next()
        .ok_or_else(|| anyhow!("missing object id in ls-tree record for {path_arg}"))?;
    out.push_str(mode);
    out.push(' ');
    out.push_str(oid);
    out.push('\t');
    out.push_str(path_arg);
    out.push('\n');
    Ok(())
}

fn commit_message(subject: &str, store_rev: &str, base: &str, tree: Option<&str>) -> String {
    let mut message = String::new();
    message.push_str(subject);
    message.push_str("\n\n");
    message.push_str(&format_apply_trailers(store_rev, base, tree));
    message
}

fn git_dir(git: &Git) -> Result<PathBuf> {
    let git_dir = PathBuf::from(git.run_str(&["rev-parse", "--git-dir"])?);
    if git_dir.is_absolute() {
        Ok(git_dir)
    } else {
        Ok(git.repo().join(git_dir))
    }
}

fn path_arg(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("path is not UTF-8: {}", path.display()))
}
