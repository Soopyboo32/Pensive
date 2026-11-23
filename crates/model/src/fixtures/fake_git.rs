//! A `GitBackend` that answers from the sample repositories instead of running Git, and keeps
//! saved files and staging in memory.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::Result;

use super::changes;
use crate::{
    Changes, FileKey, FileVersions, FixtureGap, GitBackend, PROJECT_FILE, Repository, Source,
    Version, WorktreeFiles,
};

/// A value for each file a sample worktree has one for, by worktree and path.
type PerFile<T> = Mutex<HashMap<(PathBuf, PathBuf), T>>;

/// The pensive sample's `.pensive/project.toml`. The other samples have none.
pub(crate) const SAMPLE_PROJECT_FILE: &str = r#"[[run]]
name = "pensive"
command = "cargo run"

[[run]]
name = "demo"
command = "cargo run -p pensive --features demo -- --size 1280x800"

[[run]]
name = "mocks"
command = "cargo mocks"
"#;

#[derive(Debug)]
pub struct FakeGit {
    repositories: Vec<Repository>,
    /// Whether the main worktrees have the sample changes. Without them nothing has changed.
    sample_changes: bool,
    /// Files written in the main worktrees, over the samples.
    saved: PerFile<String>,
    /// Files staged or unstaged in the main worktrees, over the samples.
    staged: PerFile<bool>,
}

impl Default for FakeGit {
    fn default() -> Self {
        Self {
            repositories: super::repositories()
                .into_iter()
                .map(|(repository, _)| repository)
                .collect(),
            sample_changes: true,
            saved: Mutex::default(),
            staged: Mutex::default(),
        }
    }
}

impl FakeGit {
    /// The sample repositories with nothing changed in any of them.
    pub fn clean() -> Self {
        Self {
            sample_changes: false,
            ..Self::default()
        }
    }

    fn sample_root(worktree: &Path, path: &Path) -> Result<()> {
        if changes::has_samples(worktree) {
            Ok(())
        } else {
            Err(FixtureGap(format!("{} has no sample versions", path.display())).into())
        }
    }
}

impl GitBackend for FakeGit {
    fn repository(&self, path: &Path) -> Result<Repository> {
        self.repositories
            .iter()
            .find(|repository| {
                repository
                    .worktrees
                    .iter()
                    .any(|worktree| worktree.path == path)
            })
            .cloned()
            .ok_or_else(|| {
                FixtureGap(format!("{} is not in a sample repository", path.display())).into()
            })
    }

    /// The sample changes in a main worktree, as staged since, and none in a linked worktree.
    fn changes(&self, worktree: &Path, earlier: usize) -> Result<Changes> {
        self.repository(worktree)?;
        let samples = changes::changes_in(worktree).filter(|_| self.sample_changes);
        let Some(mut changes) = samples else {
            return Ok(Changes::default());
        };
        changes.earlier.truncate(earlier);
        // Git lists an unstaged rename as its old path deleted and its new path untracked. This
        // keeps it one renamed file.
        for ((root, path), staged) in self.staged.lock().expect("not poisoned").iter() {
            if root == worktree {
                changes.set_staged(path, *staged);
            }
        }
        Ok(changes)
    }

    fn versions(
        &self,
        worktree: &Path,
        key: &FileKey,
        renamed_from: Option<&Path>,
    ) -> Result<FileVersions> {
        Self::sample_root(worktree, &key.path)?;
        let mut versions = changes::versions(worktree, key, renamed_from)
            .ok_or_else(|| FixtureGap(format!("{} has no sample versions", key.path.display())))?;
        if key.source == Source::Uncommitted {
            versions.new = self.read_file(worktree, &key.path)?;
        }
        Ok(versions)
    }

    /// Pensive's sample tree, or else the files the worktree's samples leave on disk.
    fn files(&self, worktree: &Path) -> Result<WorktreeFiles> {
        self.repository(worktree)?;
        if worktree == Path::new(super::PENSIVE_ROOT) {
            return Ok(super::worktree_files());
        }
        Ok(WorktreeFiles {
            files: changes::on_disk(worktree),
            ..WorktreeFiles::default()
        })
    }

    /// A saved file, or else the sample's new version, or else missing.
    fn read_file(&self, worktree: &Path, path: &Path) -> Result<Version> {
        if path == Path::new(PROJECT_FILE) {
            self.repository(worktree)?;
            return Ok(if worktree == Path::new(super::PENSIVE_ROOT) {
                Version::Text(SAMPLE_PROJECT_FILE.into())
            } else {
                Version::Missing
            });
        }
        Self::sample_root(worktree, path)?;
        let entry = (worktree.to_path_buf(), path.to_path_buf());
        if let Some(text) = self.saved.lock().expect("not poisoned").get(&entry) {
            return Ok(Version::Text(text.clone()));
        }
        let key = FileKey {
            source: Source::Uncommitted,
            path: path.to_path_buf(),
        };
        Ok(changes::versions(worktree, &key, None)
            .map_or(Version::Missing, |versions| versions.new))
    }

    fn write_file(&self, worktree: &Path, path: &Path, text: &str) -> Result<()> {
        Self::sample_root(worktree, path)?;
        self.saved.lock().expect("not poisoned").insert(
            (worktree.to_path_buf(), path.to_path_buf()),
            text.to_owned(),
        );
        Ok(())
    }

    fn set_staged(&self, worktree: &Path, paths: &[PathBuf], staged: bool) -> Result<()> {
        let mut staging = self.staged.lock().expect("not poisoned");
        for path in paths {
            Self::sample_root(worktree, path)?;
            staging.insert((worktree.to_path_buf(), path.clone()), staged);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        FileStatus,
        fixtures::{PENSIVE_ROOT, contract, repositories},
    };

    const HARBOR_ROOT: &str = "~/code/harbor";

    fn roots() -> Vec<PathBuf> {
        repositories()
            .into_iter()
            .map(|(repository, _)| repository.root)
            .collect()
    }

    #[test]
    fn keeps_the_git_contract() {
        contract::git(&FakeGit::default(), Path::new(PENSIVE_ROOT));
    }

    #[test]
    fn keeps_the_git_contract_mid_merge() {
        contract::git(&FakeGit::default(), Path::new(HARBOR_ROOT));
    }

    #[test]
    fn every_sample_project_has_changes_with_versions() {
        let git = FakeGit::default();
        for root in roots() {
            let changes = git.changes(&root, 0).unwrap();
            let name = root.display();
            assert!(!changes.uncommitted.is_empty(), "{name} has no files");
            assert!(!changes.commits.is_empty(), "{name} has no commits");
            let uncommitted = changes
                .uncommitted
                .iter()
                .map(|file| (Source::Uncommitted, file));
            let committed = changes.commits.iter().flat_map(|commit| {
                let source = Source::Commit(commit.sha.clone());
                commit.files.iter().map(move |file| (source.clone(), file))
            });
            for (source, file) in uncommitted.chain(committed) {
                let key = FileKey {
                    source,
                    path: file.path.clone(),
                };
                git.versions(&root, &key, file.renamed_from.as_deref())
                    .unwrap();
            }
        }
    }

    #[test]
    fn harbor_has_a_renamed_and_a_conflicted_file() {
        let changes = FakeGit::default()
            .changes(Path::new(HARBOR_ROOT), 0)
            .unwrap();
        let has = |status| {
            changes
                .uncommitted
                .iter()
                .any(|file| file.status() == status)
        };
        assert!(has(FileStatus::Renamed));
        assert!(has(FileStatus::Conflicted));
    }

    #[test]
    fn clean_has_no_changes_in_any_project() {
        let git = FakeGit::clean();
        for root in roots() {
            let changes = git.changes(&root, 0).unwrap();
            assert!(changes.uncommitted.is_empty() && changes.commits.is_empty());
        }
    }

    #[test]
    fn saved_files_and_staging_stay_in_their_worktree() {
        let git = FakeGit::default();
        let (harbor, lantern) = (Path::new(HARBOR_ROOT), Path::new("~/code/lantern"));
        let path = git.changes(harbor, 0).unwrap().uncommitted[0].path.clone();

        git.write_file(lantern, &path, "lantern's\n").unwrap();
        git.set_staged(lantern, std::slice::from_ref(&path), true)
            .unwrap();

        assert_ne!(
            git.read_file(harbor, &path).unwrap(),
            Version::Text("lantern's\n".into())
        );
        let file = git.changes(harbor, 0).unwrap();
        assert_eq!(
            file.file(&path).unwrap().staging(),
            crate::Staging::Unstaged
        );
    }
}
