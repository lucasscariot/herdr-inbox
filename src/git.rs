//! Reads a checkout's repository name and branch straight from `.git`, without
//! running git: this runs for every thread on every refresh.

use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    /// The main repository's directory name, also for linked worktrees.
    pub repo: String,
    /// The branch, or a short commit id when HEAD is detached.
    pub branch: Option<String>,
    pub root: PathBuf,
}

/// Finds the checkout containing `path`, walking up to the filesystem root.
pub fn checkout(path: &Path) -> Option<Checkout> {
    let mut dir = Some(path);
    while let Some(current) = dir {
        let dot_git = current.join(".git");
        if let Some((git_dir, common_dir)) = git_dirs(&dot_git) {
            let repo = repo_name(&common_dir)?;
            return Some(Checkout { repo, branch: read_branch(&git_dir), root: current.to_path_buf() });
        }
        dir = current.parent();
    }
    None
}

/// `(git dir, common dir)` for a `.git` directory or a `gitdir:` pointer file.
fn git_dirs(dot_git: &Path) -> Option<(PathBuf, PathBuf)> {
    let meta = fs::metadata(dot_git).ok()?;
    if meta.is_dir() {
        return Some((dot_git.to_path_buf(), dot_git.to_path_buf()));
    }
    let content = fs::read_to_string(dot_git).ok()?;
    let pointer = content.lines().find_map(|line| line.strip_prefix("gitdir:"))?.trim();
    let base = dot_git.parent()?;
    let git_dir = absolute(base, Path::new(pointer));
    let common_dir = match fs::read_to_string(git_dir.join("commondir")) {
        Ok(common) => absolute(&git_dir, Path::new(common.trim())),
        Err(_) => git_dir.clone(),
    };
    Some((git_dir, common_dir))
}

fn absolute(base: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() { path.to_path_buf() } else { base.join(path) };
    normalize(&joined)
}

/// Resolves `.` and `..` lexically, so `repo/.git/worktrees/x/../..` is `repo/.git`.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// A repository is named after the directory holding its common `.git`
/// directory; a bare common dir `name.git` is named `name`.
fn repo_name(common_dir: &Path) -> Option<String> {
    if common_dir.file_name()? == ".git" {
        return common_dir.parent()?.file_name()?.to_str().map(str::to_string);
    }
    let name = common_dir.file_name()?.to_str()?;
    Some(name.strip_suffix(".git").unwrap_or(name).to_string())
}

fn read_branch(git_dir: &Path) -> Option<String> {
    let head = fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref:") {
        let reference = reference.trim();
        return Some(reference.strip_prefix("refs/heads/").unwrap_or(reference).to_string());
    }
    let is_commit = head.len() >= 7 && head.chars().all(|c| c.is_ascii_hexdigit());
    is_commit.then(|| head[..7].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn a_plain_repository_reports_its_branch_from_any_subdirectory() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("cockpit");
        write(&repo.join(".git/HEAD"), "ref: refs/heads/main\n");
        fs::create_dir_all(repo.join("src/deep")).unwrap();
        let found = checkout(&repo.join("src/deep")).unwrap();
        assert_eq!(found, Checkout { repo: "cockpit".into(), branch: Some("main".into()), root: repo });
    }

    #[test]
    fn branches_with_slashes_keep_them() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("r/.git/HEAD"), "ref: refs/heads/feature/login-fix");
        assert_eq!(checkout(&dir.path().join("r")).unwrap().branch.as_deref(), Some("feature/login-fix"));
    }

    #[test]
    fn a_detached_head_reports_a_short_commit() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("r/.git/HEAD"), "0123456789abcdef0123456789abcdef01234567\n");
        assert_eq!(checkout(&dir.path().join("r")).unwrap().branch.as_deref(), Some("0123456"));
    }

    #[test]
    fn a_garbled_head_has_no_branch_but_still_names_the_repo() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("r/.git/HEAD"), "nonsense");
        let found = checkout(&dir.path().join("r")).unwrap();
        assert_eq!(found.repo, "r");
        assert_eq!(found.branch, None);
    }

    #[test]
    fn a_linked_worktree_is_named_after_its_main_repository() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("Work/cockpit");
        write(&main.join(".git/HEAD"), "ref: refs/heads/main");
        let admin = main.join(".git/worktrees/fix-login");
        write(&admin.join("HEAD"), "ref: refs/heads/fix-login");
        write(&admin.join("commondir"), "../..\n");
        let linked = dir.path().join(".herdr/worktrees/cockpit/fix-login");
        write(&linked.join(".git"), &format!("gitdir: {}\n", admin.display()));
        let found = checkout(&linked).unwrap();
        assert_eq!(found.repo, "cockpit");
        assert_eq!(found.branch.as_deref(), Some("fix-login"));
        assert_eq!(found.root, linked);
    }

    #[test]
    fn a_relative_gitdir_pointer_is_resolved_from_the_checkout() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("api");
        let admin = main.join(".git/worktrees/w");
        write(&main.join(".git/HEAD"), "ref: refs/heads/main");
        write(&admin.join("HEAD"), "ref: refs/heads/w");
        write(&admin.join("commondir"), "../..");
        let linked = main.join(".claude/worktrees/w");
        write(&linked.join(".git"), "gitdir: ../../../.git/worktrees/w");
        let found = checkout(&linked).unwrap();
        assert_eq!(found.repo, "api");
        assert_eq!(found.branch.as_deref(), Some("w"));
    }

    #[test]
    fn a_worktree_of_a_bare_repository_drops_the_dot_git_suffix() {
        let dir = tempfile::tempdir().unwrap();
        let bare = dir.path().join("site.git");
        let admin = bare.join("worktrees/main");
        write(&admin.join("HEAD"), "ref: refs/heads/main");
        write(&admin.join("commondir"), "../..");
        let linked = dir.path().join("site-main");
        write(&linked.join(".git"), &format!("gitdir: {}", admin.display()));
        assert_eq!(checkout(&linked).unwrap().repo, "site");
    }

    #[test]
    fn a_gitdir_file_without_a_pointer_is_not_a_checkout() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("x/.git"), "garbage");
        assert_eq!(checkout(&dir.path().join("x")), None);
    }

    #[test]
    fn a_path_outside_any_repository_has_no_checkout() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(checkout(dir.path()), None);
        assert_eq!(checkout(&dir.path().join("missing/path")), None);
    }
}
