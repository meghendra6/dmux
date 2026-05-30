//! Lightweight, read-only git inspection for status-line tokens.
//!
//! The branch is derived by parsing `.git/HEAD` without shelling out, walking
//! up the directory tree to find the repository. `.git` is normally a
//! directory, but in worktrees and submodules it is a file containing
//! `gitdir: <path>`.
//!
//! Working-tree dirtiness needs a real `git status --porcelain`, which is
//! cached per repository with a short TTL so it never runs on every status
//! render.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long a cached dirty result is reused before `git status` runs again.
const DIRTY_TTL: Duration = Duration::from_secs(2);

/// Returns the current branch for `cwd`: the branch name for a normal checkout,
/// a short commit hash for a detached HEAD, or `None` when `cwd` is not inside
/// a git repository.
pub fn branch(cwd: &Path) -> Option<String> {
    let git_dir = find_git_dir(cwd)?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref: ") {
        // Branch names may contain '/', so strip the refs/heads/ prefix rather
        // than splitting on the last segment.
        let branch = reference.strip_prefix("refs/heads/").unwrap_or(reference);
        if branch.is_empty() {
            return None;
        }
        return Some(branch.to_string());
    }
    // Detached HEAD: a raw commit hash.
    if head.len() >= 7 && head.chars().all(|ch| ch.is_ascii_hexdigit()) {
        return Some(head[..7].to_string());
    }
    None
}

/// Walk up from `start` to locate the git directory, resolving the `gitdir:`
/// indirection when `.git` is a file (worktrees/submodules).
fn find_git_dir(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if dot_git.is_file() {
            let contents = std::fs::read_to_string(&dot_git).ok()?;
            let gitdir = contents.trim().strip_prefix("gitdir: ")?;
            let gitdir = Path::new(gitdir);
            return Some(if gitdir.is_absolute() {
                gitdir.to_path_buf()
            } else {
                dir.join(gitdir)
            });
        }
    }
    None
}

/// Returns whether the working tree containing `cwd` has uncommitted changes,
/// or `None` when `cwd` is not inside a git repository. The result is cached
/// per repository for `DIRTY_TTL`, so repeated status renders do not spawn a
/// `git` process each time.
pub fn is_dirty(cwd: &Path) -> Option<bool> {
    let root = repo_root(cwd)?;
    let now = Instant::now();
    {
        let cache = dirty_cache().lock().unwrap();
        if let Some((checked_at, dirty)) = cache.get(&root)
            && now.duration_since(*checked_at) < DIRTY_TTL
        {
            return Some(*dirty);
        }
    }
    let dirty = compute_dirty(&root)?;
    dirty_cache().lock().unwrap().insert(root, (now, dirty));
    Some(dirty)
}

/// Walk up from `start` to the work-tree root (the directory that holds `.git`).
fn repo_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
}

/// Run `git status --porcelain` in `root` and report whether the tree is dirty.
/// Returns `None` if git is unavailable or the command fails.
fn compute_dirty(root: &Path) -> Option<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(dirty_from_porcelain(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// A `git status --porcelain` output is dirty when it lists any entry. Empty or
/// whitespace-only output means a clean tree.
fn dirty_from_porcelain(output: &str) -> bool {
    !output.trim().is_empty()
}

fn dirty_cache() -> &'static Mutex<HashMap<PathBuf, (Instant, bool)>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, (Instant, bool)>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn unique_dir(label: &str) -> PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!(
            "dmux-git-test-{}-{}-{}",
            label,
            std::process::id(),
            n
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn branch_reads_ref_from_git_dir() {
        let repo = unique_dir("ref");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(repo.join(".git/HEAD"), "ref: refs/heads/feature/login\n").unwrap();

        assert_eq!(branch(&repo).as_deref(), Some("feature/login"));

        // A nested working directory resolves to the same repository.
        let nested = repo.join("src/inner");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(branch(&nested).as_deref(), Some("feature/login"));

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn branch_resolves_worktree_git_file() {
        // A worktree's `.git` is a file pointing at the real git dir.
        let real_git = unique_dir("worktree-gitdir");
        std::fs::write(real_git.join("HEAD"), "ref: refs/heads/wt\n").unwrap();
        let worktree = unique_dir("worktree");
        std::fs::write(
            worktree.join(".git"),
            format!("gitdir: {}\n", real_git.display()),
        )
        .unwrap();

        assert_eq!(branch(&worktree).as_deref(), Some("wt"));

        let _ = std::fs::remove_dir_all(&real_git);
        let _ = std::fs::remove_dir_all(&worktree);
    }

    #[test]
    fn branch_returns_short_hash_for_detached_head() {
        let repo = unique_dir("detached");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        std::fs::write(
            repo.join(".git/HEAD"),
            "1234567890abcdef1234567890abcdef12345678\n",
        )
        .unwrap();

        assert_eq!(branch(&repo).as_deref(), Some("1234567"));

        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn branch_is_none_outside_a_repository() {
        let plain = unique_dir("plain");
        assert_eq!(branch(&plain), None);
        let _ = std::fs::remove_dir_all(&plain);
    }

    #[test]
    fn dirty_from_porcelain_detects_changes() {
        assert!(!dirty_from_porcelain(""));
        assert!(!dirty_from_porcelain("   \n  \n"));
        assert!(dirty_from_porcelain(" M src/main.rs\n"));
        assert!(dirty_from_porcelain("?? new.txt\n"));
    }

    #[test]
    fn repo_root_finds_worktree_root() {
        let repo = unique_dir("root");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let nested = repo.join("a/b");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(repo_root(&nested), Some(repo.clone()));

        let plain = unique_dir("root-none");
        assert_eq!(repo_root(&plain), None);

        let _ = std::fs::remove_dir_all(&repo);
        let _ = std::fs::remove_dir_all(&plain);
    }

    #[test]
    fn compute_dirty_reports_clean_and_dirty_trees() {
        let clean = unique_dir("clean");
        assert!(git_init(&clean), "git must be available for this test");
        assert_eq!(compute_dirty(&clean), Some(false));
        // The cached entry point agrees on the first (cache-miss) lookup.
        assert_eq!(is_dirty(&clean), Some(false));

        let dirty = unique_dir("dirty");
        assert!(git_init(&dirty), "git must be available for this test");
        std::fs::write(dirty.join("change.txt"), "x").unwrap();
        assert_eq!(compute_dirty(&dirty), Some(true));

        let _ = std::fs::remove_dir_all(&clean);
        let _ = std::fs::remove_dir_all(&dirty);
    }

    fn git_init(dir: &Path) -> bool {
        Command::new("git")
            .arg("-C")
            .arg(dir)
            .arg("init")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }
}
