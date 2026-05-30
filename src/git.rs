//! Lightweight, read-only git inspection for status-line tokens.
//!
//! This intentionally never shells out: the branch is derived by parsing
//! `.git/HEAD`, walking up the directory tree to find the repository. `.git`
//! is normally a directory, but in worktrees and submodules it is a file
//! containing `gitdir: <path>`.

use std::path::{Path, PathBuf};

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
}
