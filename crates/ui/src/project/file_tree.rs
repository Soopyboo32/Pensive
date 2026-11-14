//! Picks which of a worktree's files the Files panel lists, by its settings and the filter, and
//! lays them out as the tree's rows.

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use model::WorktreeFiles;

use super::{
    files_options::{IgnoredFiles, Sort},
    tree::{Entry, Open, Row, Shape},
};

/// What decides which rows show, read from the panel's settings.
pub(crate) struct TreeOptions {
    pub dotfiles: bool,
    pub ignored: IgnoredFiles,
    pub fold: bool,
    pub sort: Sort,
}

/// The rows for `files`, opening the folders in `open`. A non-empty `filter` keeps only files
/// whose path contains it, ignoring case, and opens every folder above them.
pub(crate) fn rows<'a>(
    files: &'a WorktreeFiles,
    open: &HashSet<PathBuf>,
    filter: &str,
    options: &TreeOptions,
) -> Vec<Row> {
    let filter = filter.to_lowercase();
    let shown = |path: &Path| {
        (options.dotfiles
            || !path
                .iter()
                .any(|part| part.to_string_lossy().starts_with('.')))
            && path.to_string_lossy().to_lowercase().contains(&filter)
    };
    let with_ignored = options.ignored != IgnoredFiles::Hidden;
    let dim = options.ignored == IgnoredFiles::Dimmed;
    let ignored = |path: &'a Path, sealed| Entry {
        path,
        ignored: dim,
        sealed,
    };
    let entries = files
        .files
        .iter()
        .map(|path| Entry::file(path))
        .chain(
            files
                .ignored_files
                .iter()
                .filter(|_| with_ignored)
                .map(|path| ignored(path, false)),
        )
        .chain(
            files
                .ignored_folders
                .iter()
                .filter(|_| with_ignored && filter.is_empty())
                .map(|path| ignored(path, true)),
        )
        .filter(|entry| shown(entry.path));
    let shape = Shape {
        open: if filter.is_empty() {
            Open::Only(open)
        } else {
            Open::All
        },
        fold: options.fold,
        sort: Some(options.sort),
    };
    super::tree::rows(entries, &shape)
}

/// Every folder above `path`, from the top down, so opening them all shows it.
pub(crate) fn ancestors(path: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    let mut above: Vec<_> = path
        .ancestors()
        .skip(1)
        .filter(|folder| !folder.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .collect();
    above.reverse();
    above.into_iter()
}

#[cfg(test)]
mod tests {
    use super::{super::tree::RowKind, *};

    fn files(paths: &[&str]) -> WorktreeFiles {
        WorktreeFiles {
            files: paths.iter().map(PathBuf::from).collect(),
            ..WorktreeFiles::default()
        }
    }

    fn options() -> TreeOptions {
        TreeOptions {
            dotfiles: true,
            ignored: IgnoredFiles::Dimmed,
            fold: true,
            sort: Sort::FoldersFirst,
        }
    }

    fn names(rows: &[Row]) -> Vec<(usize, &str)> {
        rows.iter()
            .map(|row| (row.depth, row.name.as_str()))
            .collect()
    }

    #[test]
    fn a_filter_keeps_matches_and_opens_the_folders_above_them() {
        let files = files(&["src/palette.rs", "src/root.rs", "docs/palette.md"]);
        let rows = rows(&files, &HashSet::new(), "PALETTE", &options());
        assert_eq!(
            names(&rows),
            [
                (0, "docs"),
                (1, "palette.md"),
                (0, "src"),
                (1, "palette.rs")
            ]
        );
    }

    #[test]
    fn dotfiles_and_ignored_entries_follow_their_settings() {
        let files = WorktreeFiles {
            files: vec![".gitignore".into(), "src/lib.rs".into()],
            ignored_files: vec!["notes.log".into()],
            ignored_folders: vec!["target".into()],
        };
        let rows = rows(&files, &HashSet::new(), "", &options());
        assert_eq!(
            names(&rows),
            [
                (0, "src"),
                (0, "target"),
                (0, ".gitignore"),
                (0, "notes.log")
            ]
        );
        let target = &rows[1];
        assert!(target.ignored);
        assert_eq!(
            target.kind,
            RowKind::Folder {
                open: false,
                openable: false
            }
        );

        let hidden = TreeOptions {
            dotfiles: false,
            ignored: IgnoredFiles::Hidden,
            ..options()
        };
        assert_eq!(
            names(&super::rows(&files, &HashSet::new(), "", &hidden)),
            [(0, "src")]
        );
    }

    #[test]
    fn ancestors_run_from_the_top_down() {
        let above: Vec<_> = ancestors(Path::new("a/b/c.rs")).collect();
        assert_eq!(above, [PathBuf::from("a"), PathBuf::from("a/b")]);
    }
}
