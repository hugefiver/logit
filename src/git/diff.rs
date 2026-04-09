use std::collections::HashMap;

use anyhow::{Context, Result};
use git2::{Commit, DiffFormat, DiffOptions, Repository};

use crate::stats::models::FileChange;

/// Analyze the diff of a commit against its first parent.
/// For initial commits (no parents), diffs against an empty tree.
/// Returns per-file addition/deletion counts.
/// The `language` field is left as `None` — caller should apply language classification.
pub fn analyze_commit_diff(repo: &Repository, commit: &Commit) -> Result<Vec<FileChange>> {
    let commit_tree = commit.tree().context("Failed to get commit tree")?;

    let parent_tree = if commit.parent_count() > 0 {
        Some(
            commit
                .parent(0)
                .context("Failed to get parent commit")?
                .tree()
                .context("Failed to get parent tree")?,
        )
    } else {
        None
    };

    let mut opts = DiffOptions::new();
    opts.ignore_submodules(true);

    let diff = repo
        .diff_tree_to_tree(parent_tree.as_ref(), Some(&commit_tree), Some(&mut opts))
        .context("Failed to compute diff")?;

    let mut file_stats: HashMap<String, (u64, u64)> = HashMap::new();

    diff.print(DiffFormat::Patch, |delta, _hunk, line| {
        if !delta.flags().is_binary() {
            let path = delta.new_file().path().or_else(|| delta.old_file().path());
            if let Some(p) = path {
                let path_str = p.to_string_lossy().to_string();
                let stats = file_stats.entry(path_str).or_insert((0, 0));
                match line.origin() {
                    '+' => stats.0 += 1,
                    '-' => stats.1 += 1,
                    _ => {}
                }
            }
        }
        true
    })
    .context("Failed to iterate diff")?;

    let mut changes: Vec<FileChange> = file_stats
        .into_iter()
        .map(|(path, (additions, deletions))| FileChange {
            path,
            language: None,
            additions,
            deletions,
        })
        .collect();

    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Repository, Signature, Time};
    use tempfile::TempDir;

    fn create_initial_commit(repo: &Repository, files: &[(&str, &str)]) -> git2::Oid {
        let sig = Signature::new("Test", "test@test.com", &Time::new(1_705_312_800, 0)).unwrap();
        let mut root_tb = repo.treebuilder(None).unwrap();

        for (name, content) in files {
            let blob = repo.blob(content.as_bytes()).unwrap();
            if let Some((dir, file)) = name.split_once('/') {
                let mut sub_tb = repo.treebuilder(None).unwrap();
                sub_tb.insert(file, blob, 0o100644).unwrap();
                let sub_oid = sub_tb.write().unwrap();
                root_tb.insert(dir, sub_oid, 0o040000).unwrap();
            } else {
                root_tb.insert(*name, blob, 0o100644).unwrap();
            }
        }

        let tree_oid = root_tb.write().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "Initial", &tree, &[])
            .unwrap()
    }

    #[test]
    fn initial_commit_all_additions() {
        let tmp = TempDir::new().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let oid = create_initial_commit(&repo, &[("file.txt", "line1\nline2\nline3\n")]);
        let commit = repo.find_commit(oid).unwrap();
        let changes = analyze_commit_diff(&repo, &commit).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "file.txt");
        assert_eq!(changes[0].additions, 3);
        assert_eq!(changes[0].deletions, 0);
    }

    #[test]
    fn normal_commit_additions_and_deletions() {
        let tmp = TempDir::new().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let oid1 = create_initial_commit(&repo, &[("file.txt", "line1\nline2\nline3\n")]);

        let sig = Signature::new("Test", "test@test.com", &Time::new(1_705_413_600, 0)).unwrap();
        let parent = repo.find_commit(oid1).unwrap();
        let parent_tree = parent.tree().unwrap();
        let blob = repo.blob(b"line1\nmodified\nline3\nnewline\n").unwrap();
        let mut tb = repo.treebuilder(Some(&parent_tree)).unwrap();
        tb.insert("file.txt", blob, 0o100644).unwrap();
        let tree_oid = tb.write().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let oid2 = repo
            .commit(Some("HEAD"), &sig, &sig, "Modify", &tree, &[&parent])
            .unwrap();

        let commit = repo.find_commit(oid2).unwrap();
        let changes = analyze_commit_diff(&repo, &commit).unwrap();
        assert_eq!(changes.len(), 1);
        assert!(changes[0].additions > 0);
        assert!(changes[0].deletions > 0);
    }

    #[test]
    fn delete_file_commit() {
        let tmp = TempDir::new().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let oid1 = create_initial_commit(&repo, &[("a.txt", "aaa\n"), ("b.txt", "bbb\n")]);

        let sig = Signature::new("Test", "test@test.com", &Time::new(1_705_413_600, 0)).unwrap();
        let parent = repo.find_commit(oid1).unwrap();
        let parent_tree = parent.tree().unwrap();
        let mut tb = repo.treebuilder(Some(&parent_tree)).unwrap();
        tb.remove("b.txt").unwrap();
        let tree_oid = tb.write().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let oid2 = repo
            .commit(Some("HEAD"), &sig, &sig, "Delete b.txt", &tree, &[&parent])
            .unwrap();

        let commit = repo.find_commit(oid2).unwrap();
        let changes = analyze_commit_diff(&repo, &commit).unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].path, "b.txt");
        assert_eq!(changes[0].additions, 0);
        assert_eq!(changes[0].deletions, 1);
    }

    #[test]
    fn language_field_is_none() {
        let tmp = TempDir::new().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let oid = create_initial_commit(&repo, &[("main.rs", "fn main() {}\n")]);
        let commit = repo.find_commit(oid).unwrap();
        let changes = analyze_commit_diff(&repo, &commit).unwrap();
        assert!(changes[0].language.is_none());
    }

    #[test]
    fn multiple_files_in_commit() {
        let tmp = TempDir::new().unwrap();
        let repo = Repository::init(tmp.path()).unwrap();
        let oid = create_initial_commit(&repo, &[("a.txt", "aaa\n"), ("b.txt", "bbb\nccc\n")]);
        let commit = repo.find_commit(oid).unwrap();
        let changes = analyze_commit_diff(&repo, &commit).unwrap();
        assert_eq!(changes.len(), 2);
        assert_eq!(changes[0].path, "a.txt");
        assert_eq!(changes[0].additions, 1);
        assert_eq!(changes[1].path, "b.txt");
        assert_eq!(changes[1].additions, 2);
    }
}
