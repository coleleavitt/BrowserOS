mod fixtures;

use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use bpatch::cli::apply::{self as cli_apply, ApplyReport};
use bpatch::engine::apply::ApplyOptions;
use bpatch::engine::lock::CheckoutLock;
use bpatch::engine::progress;
use bpatch::engine::state::{self, StateContext, TRAILER_BASE, TRAILER_STORE_REV, TRAILER_TREE};
use bpatch::process::Git;
use fixtures::FixtureRepo;
use serde_json::Value;

struct ApplyScenario {
    checkout: FixtureRepo,
    store: FixtureRepo,
    store_dir: PathBuf,
    base: String,
    rev1_commit: String,
}

#[test]
fn behind_checkout_touches_only_store_delta() -> Result<()> {
    let scenario = applied_rev1_scenario()?;
    write_checkout_rev2(&scenario.checkout, false)?;
    let rev2_store = commit_store_from_index(
        &scenario.store,
        &scenario.checkout,
        &scenario.base,
        &[
            "chrome/browser/ui/llmchat/panel.cc",
            "chrome/browser/ui/llmchat/resize_util.cc",
        ],
        "store rev2",
    )?;
    scenario
        .checkout
        .git()
        .run(&["reset", "--hard", &scenario.rev1_commit])?;

    let kept_mtime = scenario
        .checkout
        .mtime("chrome/browser/ui/llmchat/panel.h")?;
    scenario
        .checkout
        .plant_untracked("out/Default_arm64/local.marker", "keep me\n")?;
    thread::sleep(Duration::from_millis(1100));

    let report = run_apply(&scenario.store_dir, &scenario.checkout, false);

    match report {
        ApplyReport::Applied {
            store_rev,
            files_changed,
            commits,
            ..
        } => {
            assert_eq!(
                store_rev,
                scenario
                    .store
                    .git()
                    .run_str(&["rev-parse", "--short", &rev2_store])?
            );
            assert_eq!(files_changed, 2);
            assert_eq!(commits.len(), 1);
            assert_eq!(commits[0].feature, "llmchat");
            assert_eq!(commits[0].seq, 2);
        }
        other => panic!("expected applied report, got {other:?}"),
    }

    assert_eq!(
        scenario
            .checkout
            .read_file("chrome/browser/ui/llmchat/panel.cc")?,
        "current panel\n"
    );
    assert_eq!(
        scenario
            .checkout
            .read_file("chrome/browser/ui/llmchat/panel.h")?,
        "applied header\n"
    );
    assert_eq!(
        scenario
            .checkout
            .mtime("chrome/browser/ui/llmchat/panel.h")?,
        kept_mtime
    );
    assert_eq!(
        scenario
            .checkout
            .read_file("chrome/browser/ui/llmchat/resize_util.cc")?,
        "resize\n"
    );
    assert_eq!(
        scenario
            .checkout
            .read_file("out/Default_arm64/local.marker")?,
        "keep me\n"
    );
    Ok(())
}

#[test]
fn feature_commits_group_paths_trailer_batch_and_increment_sequences() -> Result<()> {
    let scenario = applied_rev1_scenario()?;
    write_checkout_rev2(&scenario.checkout, true)?;
    let rev2_store = commit_store_from_index(
        &scenario.store,
        &scenario.checkout,
        &scenario.base,
        &[
            "chrome/browser/ui/llmchat/panel.cc",
            "chrome/browser/ui/llmchat/resize_util.cc",
            "chrome/BUILD.gn",
        ],
        "store rev2 grouped",
    )?;
    scenario
        .checkout
        .git()
        .run(&["reset", "--hard", &scenario.rev1_commit])?;

    let report = run_apply(&scenario.store_dir, &scenario.checkout, false);
    let commits = match report {
        ApplyReport::Applied { commits, .. } => commits,
        other => panic!("expected applied report, got {other:?}"),
    };
    assert_eq!(commits.len(), 2);
    assert_eq!(
        commits
            .iter()
            .map(|commit| (commit.feature.as_str(), commit.seq))
            .collect::<Vec<_>>(),
        vec![("bootstrap", 1), ("llmchat", 2)]
    );

    let git = scenario.checkout.git_adapter();
    assert_eq!(git.commit_subject("HEAD^")?, "feat: bootstrap");
    assert_eq!(git.commit_subject("HEAD")?, "feat: llmchat #2");
    assert_apply_trailers(&git, "HEAD^", &rev2_store, &scenario.base, None)?;
    let target_tree = git.tree_id("HEAD")?;
    assert_apply_trailers(
        &git,
        "HEAD",
        &rev2_store,
        &scenario.base,
        Some(target_tree.as_str()),
    )?;

    scenario
        .checkout
        .write_file("chrome/browser/ui/llmchat/panel.cc", "third panel\n")?;
    scenario.checkout.git().run(&["add", "-A"])?;
    let rev3_store = commit_store_from_index(
        &scenario.store,
        &scenario.checkout,
        &scenario.base,
        &["chrome/browser/ui/llmchat/panel.cc"],
        "store rev3 llmchat",
    )?;
    let head_before_repeat = scenario.checkout.git().run_str(&["rev-parse", "HEAD"])?;
    scenario
        .checkout
        .git()
        .run(&["reset", "--hard", &head_before_repeat])?;

    let repeat = run_apply(&scenario.store_dir, &scenario.checkout, false);
    match repeat {
        ApplyReport::Applied { commits, .. } => {
            assert_eq!(commits.len(), 1);
            assert_eq!(commits[0].feature, "llmchat");
            assert_eq!(commits[0].seq, 3);
        }
        other => panic!("expected repeat apply, got {other:?}"),
    }
    assert_eq!(git.commit_subject("HEAD")?, "feat: llmchat #3");
    assert_apply_trailers(
        &git,
        "HEAD",
        &rev3_store,
        &scenario.base,
        Some(git.tree_id("HEAD")?.as_str()),
    )?;
    Ok(())
}

#[test]
fn second_apply_is_idempotent_and_does_not_rewrite_index_or_worktree() -> Result<()> {
    let scenario = applied_rev1_scenario()?;
    write_checkout_rev2(&scenario.checkout, false)?;
    commit_store_from_index(
        &scenario.store,
        &scenario.checkout,
        &scenario.base,
        &[
            "chrome/browser/ui/llmchat/panel.cc",
            "chrome/browser/ui/llmchat/resize_util.cc",
        ],
        "store rev2",
    )?;
    scenario
        .checkout
        .git()
        .run(&["reset", "--hard", &scenario.rev1_commit])?;
    let first = run_apply(&scenario.store_dir, &scenario.checkout, false);
    assert!(matches!(first, ApplyReport::Applied { .. }));

    let head = scenario.checkout.git().run_str(&["rev-parse", "HEAD"])?;
    let snapshot = worktree_snapshot(scenario.checkout.path())?;
    let index = fs::read(scenario.checkout.path().join(".git/index"))?;
    thread::sleep(Duration::from_millis(1100));

    let second = run_apply(&scenario.store_dir, &scenario.checkout, false);

    assert!(matches!(second, ApplyReport::Converged { .. }));
    assert_eq!(
        scenario.checkout.git().run_str(&["rev-parse", "HEAD"])?,
        head
    );
    assert_eq!(worktree_snapshot(scenario.checkout.path())?, snapshot);
    assert_eq!(
        fs::read(scenario.checkout.path().join(".git/index"))?,
        index
    );
    Ok(())
}

#[test]
fn drift_refuses_apply_and_leaves_worktree_unchanged() -> Result<()> {
    let scenario = applied_rev1_scenario()?;
    write_checkout_rev2(&scenario.checkout, false)?;
    commit_store_from_index(
        &scenario.store,
        &scenario.checkout,
        &scenario.base,
        &[
            "chrome/browser/ui/llmchat/panel.cc",
            "chrome/browser/ui/llmchat/resize_util.cc",
        ],
        "store rev2",
    )?;
    scenario
        .checkout
        .git()
        .run(&["reset", "--hard", &scenario.rev1_commit])?;
    scenario
        .checkout
        .write_file("chrome/browser/ui/llmchat/panel.cc", "manual drift\n")?;
    let before = worktree_snapshot(scenario.checkout.path())?;

    let report = run_apply(&scenario.store_dir, &scenario.checkout, false);

    match report {
        ApplyReport::Drift { files, exit } => {
            assert_eq!(exit, 3);
            assert_eq!(files.len(), 1);
            assert_eq!(
                files[0].path,
                PathBuf::from("chrome/browser/ui/llmchat/panel.cc")
            );
            assert_eq!(files[0].annotation, "modified, uncommitted");
        }
        other => panic!("expected drift report, got {other:?}"),
    }
    assert_eq!(worktree_snapshot(scenario.checkout.path())?, before);
    assert_eq!(
        scenario
            .checkout
            .read_file("chrome/browser/ui/llmchat/panel.cc")?,
        "manual drift\n"
    );
    Ok(())
}

#[test]
fn authoring_failure_happens_before_checkout_mutation() -> Result<()> {
    let scenario = applied_rev1_scenario()?;
    write_checkout_rev2(&scenario.checkout, false)?;
    commit_store_from_index(
        &scenario.store,
        &scenario.checkout,
        &scenario.base,
        &[
            "chrome/browser/ui/llmchat/panel.cc",
            "chrome/browser/ui/llmchat/resize_util.cc",
        ],
        "store rev2",
    )?;
    scenario
        .checkout
        .git()
        .run(&["reset", "--hard", &scenario.rev1_commit])?;
    scenario.checkout.git().run(&["config", "user.name", ""])?;
    scenario.checkout.git().run(&["config", "user.email", ""])?;
    scenario.checkout.git_adapter().refresh_index()?;

    let head_before = scenario.checkout.git().run_str(&["rev-parse", "HEAD"])?;
    let index_before = fs::read(scenario.checkout.path().join(".git/index"))?;
    let worktree_before = worktree_snapshot(scenario.checkout.path())?;

    let report = run_apply(&scenario.store_dir, &scenario.checkout, false);

    match report {
        ApplyReport::Error { reason, exit } => {
            assert_eq!(exit, 1);
            assert!(reason.contains("Author identity unknown"));
        }
        other => panic!("expected authoring error report, got {other:?}"),
    }
    assert_eq!(
        scenario.checkout.git().run_str(&["rev-parse", "HEAD"])?,
        head_before
    );
    assert_eq!(
        fs::read(scenario.checkout.path().join(".git/index"))?,
        index_before
    );
    assert_eq!(
        worktree_snapshot(scenario.checkout.path())?,
        worktree_before
    );
    Ok(())
}

#[test]
fn untracked_added_path_refuses_before_authoring_or_materialization() -> Result<()> {
    let scenario = applied_rev1_scenario()?;
    write_checkout_rev2(&scenario.checkout, false)?;
    commit_store_from_index(
        &scenario.store,
        &scenario.checkout,
        &scenario.base,
        &[
            "chrome/browser/ui/llmchat/panel.cc",
            "chrome/browser/ui/llmchat/resize_util.cc",
        ],
        "store rev2",
    )?;
    scenario
        .checkout
        .git()
        .run(&["reset", "--hard", &scenario.rev1_commit])?;
    scenario.checkout.plant_untracked(
        "chrome/browser/ui/llmchat/resize_util.cc",
        "local untracked\n",
    )?;
    let head_before = scenario.checkout.git().run_str(&["rev-parse", "HEAD"])?;
    let worktree_before = worktree_snapshot(scenario.checkout.path())?;

    let report = run_apply(&scenario.store_dir, &scenario.checkout, false);

    match report {
        ApplyReport::Drift { files, exit } => {
            assert_eq!(exit, 3);
            assert_eq!(files.len(), 1);
            assert_eq!(
                files[0].path,
                PathBuf::from("chrome/browser/ui/llmchat/resize_util.cc")
            );
            assert_eq!(files[0].status, "??");
            assert_eq!(files[0].annotation, "untracked, would be overwritten");
        }
        other => panic!("expected drift report, got {other:?}"),
    }
    assert_eq!(
        scenario.checkout.git().run_str(&["rev-parse", "HEAD"])?,
        head_before
    );
    assert_eq!(
        worktree_snapshot(scenario.checkout.path())?,
        worktree_before
    );
    assert_eq!(
        scenario
            .checkout
            .read_file("chrome/browser/ui/llmchat/resize_util.cc")?,
        "local untracked\n"
    );
    Ok(())
}

#[test]
fn store_base_pin_move_refuses_when_checkout_still_has_applied_history() -> Result<()> {
    let scenario = applied_rev1_scenario()?;
    scenario
        .checkout
        .git()
        .run(&["checkout", "-B", "base-149", &scenario.base])?;
    scenario.checkout.write_file(
        "chrome/VERSION",
        "MAJOR=149\nMINOR=0\nBUILD=7250\nPATCH=0\n",
    )?;
    let new_base = scenario.checkout.commit("Chromium 149.0.7250.0")?;
    scenario
        .checkout
        .git()
        .run(&["checkout", "-B", "main", &scenario.rev1_commit])?;
    scenario.store.write_file(
        "chromium_patches/.store.yaml",
        format!("base_commit: {new_base}\nbase_version: \"149.0.7250.0\"\n"),
    )?;
    scenario.store.commit("repin store")?;

    let report = run_apply(&scenario.store_dir, &scenario.checkout, false);

    match &report {
        ApplyReport::BasePinMoved {
            store_base,
            store_base_display,
            checkout_base_display,
            exit,
            ..
        } => {
            assert_eq!(store_base, &new_base);
            assert_eq!(store_base_display, "149.0.7250.0");
            assert_eq!(checkout_base_display, "148.0.7204.1");
            assert_eq!(*exit, 3);
        }
        other => panic!("expected base-pin-moved report, got {other:?}"),
    }
    let human = cli_apply::render_human(&report);
    assert!(human.contains("store base pin moved to 149.0.7250.0"));
    assert!(human.contains("check out the new base first"));
    assert!(human.contains(&format!("git checkout {new_base} && gclient sync")));
    assert!(!bpatch::engine::conflict::session_path(scenario.checkout.path())?.exists());
    Ok(())
}

#[test]
fn held_lock_returns_error_report_with_holder() -> Result<()> {
    let scenario = applied_rev1_scenario()?;
    let _held = CheckoutLock::acquire(scenario.checkout.path())?;

    let report = run_apply(&scenario.store_dir, &scenario.checkout, false);

    match report {
        ApplyReport::Error { reason, exit } => {
            assert_eq!(exit, 1);
            assert!(reason.contains("lock held by pid"));
            assert!(reason.contains("(started "));
        }
        other => panic!("expected lock error, got {other:?}"),
    }
    Ok(())
}

#[test]
fn pull_fast_forwards_store_before_applying_and_json_uses_sim_fields() -> Result<()> {
    let checkout = FixtureRepo::new()?;
    let base = write_base_checkout(&checkout)?;
    let remote_store = FixtureRepo::new()?;
    let remote_store_dir = seed_store(&remote_store, &base)?;

    write_checkout_rev1(&checkout)?;
    checkout.git().run(&["add", "-A"])?;
    let rev1_tree = checkout.git().run_str(&["write-tree"])?;
    let rev1_store = commit_store_from_index(
        &remote_store,
        &checkout,
        &base,
        &[
            "chrome/browser/ui/llmchat/panel.cc",
            "chrome/browser/ui/llmchat/panel.h",
        ],
        "store rev1",
    )?;
    let rev1_commit = checkout.commit_with_trailers(
        "feat: llmchat",
        &[
            (TRAILER_STORE_REV, rev1_store.as_str()),
            (TRAILER_BASE, base.as_str()),
            (TRAILER_TREE, rev1_tree.as_str()),
        ],
    )?;

    let local_store_root = tempfile::tempdir()?;
    Git::new(local_store_root.path()).run(&[
        "clone",
        remote_store.path().to_str().expect("utf-8 path"),
        "store",
    ])?;
    let local_store_dir = local_store_root.path().join("store/chromium_patches");

    write_checkout_rev2(&checkout, false)?;
    let rev2_store = commit_store_from_index(
        &remote_store,
        &checkout,
        &base,
        &[
            "chrome/browser/ui/llmchat/panel.cc",
            "chrome/browser/ui/llmchat/resize_util.cc",
        ],
        "store rev2",
    )?;
    checkout.git().run(&["reset", "--hard", &rev1_commit])?;

    let report = run_apply(&local_store_dir, &checkout, true);
    let json: Value = serde_json::from_str(&cli_apply::render_json(&report)?)?;

    assert_eq!(json["result"], "applied");
    assert_eq!(
        json["store_rev"],
        remote_store
            .git()
            .run_str(&["rev-parse", "--short", &rev2_store])?
    );
    assert_eq!(json["base"], "148.0.7204.1");
    assert_eq!(json["files_changed"], 2);
    assert_eq!(json["commits"][0]["feature"], "llmchat");
    assert_eq!(json["commits"][0]["seq"], 2);
    assert!(json["commits"][0].get("sha").is_some());
    assert_eq!(json["exit"], 0);
    assert_eq!(
        Git::new(remote_store_dir).run_str(&["rev-parse", "--short", "HEAD"])?,
        json["store_rev"].as_str().expect("store rev string")
    );
    Ok(())
}

#[test]
fn hand_committed_target_tree_reports_converged_before_drift() -> Result<()> {
    let scenario = applied_rev1_scenario()?;
    write_checkout_rev2(&scenario.checkout, false)?;
    let rev2_store = commit_store_from_index(
        &scenario.store,
        &scenario.checkout,
        &scenario.base,
        &[
            "chrome/browser/ui/llmchat/panel.cc",
            "chrome/browser/ui/llmchat/resize_util.cc",
        ],
        "store rev2",
    )?;
    scenario.checkout.commit("hand applied store target")?;
    let head_before = scenario.checkout.git().run_str(&["rev-parse", "HEAD"])?;

    let report = run_apply(&scenario.store_dir, &scenario.checkout, false);

    match report {
        ApplyReport::Converged {
            store_rev,
            files_changed,
            exit,
        } => {
            assert_eq!(
                store_rev,
                scenario
                    .store
                    .git()
                    .run_str(&["rev-parse", "--short", &rev2_store])?
            );
            assert_eq!(files_changed, 0);
            assert_eq!(exit, 0);
        }
        other => panic!("expected converged report, got {other:?}"),
    }
    assert_eq!(
        scenario.checkout.git().run_str(&["rev-parse", "HEAD"])?,
        head_before
    );
    Ok(())
}

fn run_apply(store_dir: &Path, checkout: &FixtureRepo, pull: bool) -> ApplyReport {
    let mut progress = progress::noop();
    cli_apply::run(
        &StateContext::new(checkout.path(), store_dir),
        ApplyOptions { pull },
        &mut progress,
    )
}

fn applied_rev1_scenario() -> Result<ApplyScenario> {
    let checkout = FixtureRepo::new()?;
    let base = write_base_checkout(&checkout)?;
    let store = FixtureRepo::new()?;
    let store_dir = seed_store(&store, &base)?;

    write_checkout_rev1(&checkout)?;
    checkout.git().run(&["add", "-A"])?;
    let rev1_tree = checkout.git().run_str(&["write-tree"])?;
    let rev1_store = commit_store_from_index(
        &store,
        &checkout,
        &base,
        &[
            "chrome/browser/ui/llmchat/panel.cc",
            "chrome/browser/ui/llmchat/panel.h",
        ],
        "store rev1",
    )?;
    let rev1_commit = checkout.commit_with_trailers(
        "feat: llmchat",
        &[
            (TRAILER_STORE_REV, rev1_store.as_str()),
            (TRAILER_BASE, base.as_str()),
            (TRAILER_TREE, rev1_tree.as_str()),
        ],
    )?;

    Ok(ApplyScenario {
        checkout,
        store,
        store_dir,
        base,
        rev1_commit,
    })
}

fn write_base_checkout(repo: &FixtureRepo) -> Result<String> {
    repo.write_file(
        "chrome/VERSION",
        "MAJOR=148\nMINOR=0\nBUILD=7204\nPATCH=1\n",
    )?;
    repo.write_file("chrome/browser/ui/llmchat/panel.cc", "base panel\n")?;
    repo.write_file("chrome/browser/ui/llmchat/panel.h", "base header\n")?;
    repo.write_file("chrome/BUILD.gn", "base build\n")?;
    repo.commit("Chromium 148.0.7204.1")
}

fn write_checkout_rev1(repo: &FixtureRepo) -> Result<()> {
    repo.write_file("chrome/browser/ui/llmchat/panel.cc", "applied panel\n")?;
    repo.write_file("chrome/browser/ui/llmchat/panel.h", "applied header\n")
}

fn write_checkout_rev2(repo: &FixtureRepo, include_build: bool) -> Result<()> {
    repo.write_file("chrome/browser/ui/llmchat/panel.cc", "current panel\n")?;
    repo.write_file("chrome/browser/ui/llmchat/resize_util.cc", "resize\n")?;
    if include_build {
        repo.write_file("chrome/BUILD.gn", "current build\n")?;
    }
    repo.git().run(&["add", "-A"])?;
    Ok(())
}

fn seed_store(store: &FixtureRepo, base: &str) -> Result<PathBuf> {
    store.write_file(
        "chromium_patches/.store.yaml",
        format!("base_commit: {base}\nbase_version: \"148.0.7204.1\"\n"),
    )?;
    store.write_file(
        "chromium_patches/.features.yaml",
        r#"version: "1.0"
features:
  llmchat:
    description: "feat: llmchat"
    files:
      - chrome/browser/ui/llmchat/
  bootstrap:
    description: "chore: bootstrap"
    files:
      - chrome/BUILD.gn
"#,
    )?;
    store.commit("seed store")?;
    Ok(store.path().join("chromium_patches"))
}

fn commit_store_from_index(
    store: &FixtureRepo,
    checkout: &FixtureRepo,
    base: &str,
    paths: &[&str],
    message: &str,
) -> Result<String> {
    for path in paths {
        let diff = checkout
            .git()
            .run(&["diff", "--binary", "--cached", base, "--", path])?;
        store.write_file(Path::new("chromium_patches").join(path), diff)?;
    }
    store.commit(message)
}

fn assert_apply_trailers(
    git: &bpatch::git::GitAdapter,
    rev: &str,
    store_rev: &str,
    base: &str,
    tree: Option<&str>,
) -> Result<()> {
    let trailers =
        state::parse_apply_trailers(&git.commit_trailers(rev)?)?.expect("apply trailers");
    assert_eq!(trailers.store_rev, store_rev);
    assert_eq!(trailers.base, base);
    assert_eq!(trailers.tree.as_deref(), tree);
    Ok(())
}

fn worktree_snapshot(root: &Path) -> Result<u64> {
    let mut files = Vec::new();
    collect_files(root, root, &mut files)?;
    files.sort();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    for path in files {
        path.hash(&mut hasher);
        fs::read(root.join(&path))
            .with_context(|| format!("reading {}", root.join(&path).display()))?
            .hash(&mut hasher);
    }
    Ok(hasher.finish())
}

fn collect_files(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("reading dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, files)?;
        } else if path.is_file() {
            files.push(path.strip_prefix(root)?.to_owned());
        }
    }
    Ok(())
}
