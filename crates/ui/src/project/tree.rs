//! Folds paths into a tree's rows: chains of single-child folders joined into one row, closed
//! folders' contents left out, and each folder's children sorted or kept in the order they came.

use std::{
    collections::{BTreeMap, HashSet},
    path::{Path, PathBuf},
};

use super::files_options::Sort;

/// One path to place in the tree.
pub(crate) struct Entry<'a> {
    pub path: &'a Path,
    /// Drawn dimmed, for what `.gitignore` leaves out.
    pub ignored: bool,
    /// A folder whose contents aren't listed, so it can't open.
    pub sealed: bool,
}

impl<'a> Entry<'a> {
    pub fn file(path: &'a Path) -> Self {
        Self {
            path,
            ignored: false,
            sealed: false,
        }
    }
}

pub(crate) enum Open<'a> {
    All,
    Only(&'a HashSet<PathBuf>),
}

/// How the entries become rows.
pub(crate) struct Shape<'a> {
    pub open: Open<'a>,
    pub fold: bool,
    /// `None` keeps the order in which files and folders first appear.
    pub sort: Option<Sort>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RowKind {
    Folder {
        open: bool,
        openable: bool,
    },
    /// The file at this index of the entries.
    File {
        index: usize,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Row {
    /// A folded chain's path is its deepest folder's.
    pub path: PathBuf,
    /// The last part of the path, or a folded chain joined with `/`.
    pub name: String,
    pub depth: usize,
    pub kind: RowKind,
    pub ignored: bool,
}

#[derive(Default)]
struct Node {
    children: BTreeMap<String, Node>,
    /// The index of the first entry at or under this node, which orders it when nothing sorts.
    first: usize,
    /// Set on a file.
    index: Option<usize>,
    ignored: bool,
    sealed: bool,
}

pub(crate) fn rows<'a>(entries: impl IntoIterator<Item = Entry<'a>>, shape: &Shape) -> Vec<Row> {
    let mut root = Node::default();
    for (index, entry) in entries.into_iter().enumerate() {
        root.insert(entry, index);
    }
    let mut rows = Vec::new();
    root.flatten(Path::new(""), 0, shape, &mut rows);
    rows
}

impl Node {
    fn insert(&mut self, entry: Entry, index: usize) {
        let mut node = self;
        for part in entry.path.iter() {
            node = node
                .children
                .entry(part.to_string_lossy().into_owned())
                .or_insert_with(|| Node {
                    first: index,
                    ..Node::default()
                });
        }
        node.index = (!entry.sealed).then_some(index);
        node.ignored = entry.ignored;
        node.sealed = entry.sealed;
    }

    fn is_folder(&self) -> bool {
        self.index.is_none()
    }

    /// Adds the rows for this folder's children, which sit `depth` levels in under `path`.
    fn flatten(&self, path: &Path, depth: usize, shape: &Shape, rows: &mut Vec<Row>) {
        let mut children: Vec<_> = self.children.iter().collect();
        match shape.sort {
            None => children.sort_by_key(|(_, node)| node.first),
            Some(sort) => children.sort_by_cached_key(|(name, node)| {
                let files_after = sort == Sort::FoldersFirst && !node.is_folder();
                (files_after, name.to_lowercase())
            }),
        }
        for (name, mut node) in children {
            let mut name = name.clone();
            let mut full = path.join(&name);
            if node.is_folder() && shape.fold {
                while let [(next, inner)] = node.children.iter().collect::<Vec<_>>()[..]
                    && inner.is_folder()
                    && !inner.sealed
                {
                    name = format!("{name}/{next}");
                    full = full.join(next);
                    node = inner;
                }
            }
            let kind = match node.index {
                Some(index) => RowKind::File { index },
                None => RowKind::Folder {
                    open: !node.sealed
                        && match shape.open {
                            Open::All => true,
                            Open::Only(open) => open.contains(&full),
                        },
                    openable: !node.sealed,
                },
            };
            let open = matches!(kind, RowKind::Folder { open: true, .. });
            rows.push(Row {
                path: full.clone(),
                name,
                depth,
                kind,
                ignored: node.ignored,
            });
            if open {
                node.flatten(&full, depth + 1, shape, rows);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows_of(paths: &[&str], shape: &Shape) -> Vec<(usize, String)> {
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        rows(paths.iter().map(|path| Entry::file(path)), shape)
            .into_iter()
            .map(|row| (row.depth, row.name))
            .collect()
    }

    fn expected(rows: &[(usize, &str)]) -> Vec<(usize, String)> {
        rows.iter()
            .map(|(depth, name)| (*depth, name.to_string()))
            .collect()
    }

    const AS_GIVEN: Shape = Shape {
        open: Open::All,
        fold: true,
        sort: None,
    };

    #[test]
    fn a_chain_of_single_child_folders_folds_into_one_row() {
        let paths = ["crates/ui/src/lib.rs", "crates/ui/Cargo.toml"];
        assert_eq!(
            rows_of(&paths, &AS_GIVEN),
            expected(&[
                (0, "crates/ui"),
                (1, "src"),
                (2, "lib.rs"),
                (1, "Cargo.toml")
            ])
        );
        let unfolded = Shape {
            fold: false,
            ..AS_GIVEN
        };
        assert_eq!(
            rows_of(&paths, &unfolded)[..2],
            expected(&[(0, "crates"), (1, "ui")])
        );
    }

    #[test]
    fn a_folder_with_siblings_keeps_its_own_row() {
        let paths = [
            "crates/ui/src/root.rs",
            "crates/theme/src/colors.rs",
            "docs/design.md",
        ];
        assert_eq!(
            rows_of(&paths, &AS_GIVEN),
            expected(&[
                (0, "crates"),
                (1, "ui/src"),
                (2, "root.rs"),
                (1, "theme/src"),
                (2, "colors.rs"),
                (0, "docs"),
                (1, "design.md"),
            ])
        );
    }

    #[test]
    fn nothing_sorts_without_a_sort() {
        assert_eq!(
            rows_of(
                &["src/palette.rs", "src/palette/row.rs", "README.md"],
                &AS_GIVEN
            ),
            expected(&[
                (0, "src"),
                (1, "palette.rs"),
                (1, "palette"),
                (2, "row.rs"),
                (0, "README.md"),
            ])
        );
    }

    #[test]
    fn folders_first_puts_folders_above_files_and_closed_folders_hide_their_contents() {
        let open = HashSet::from([PathBuf::from("src")]);
        let shape = Shape {
            open: Open::Only(&open),
            fold: true,
            sort: Some(Sort::FoldersFirst),
        };
        let paths = [
            "README.md",
            "src/lib.rs",
            "src/a.rs",
            "docs/b.md",
            "Cargo.toml",
        ];
        assert_eq!(
            rows_of(&paths, &shape),
            expected(&[
                (0, "docs"),
                (0, "src"),
                (1, "a.rs"),
                (1, "lib.rs"),
                (0, "Cargo.toml"),
                (0, "README.md"),
            ])
        );
    }

    #[test]
    fn mixed_sorts_folders_among_files() {
        let shape = Shape {
            sort: Some(Sort::Mixed),
            ..AS_GIVEN
        };
        assert_eq!(
            rows_of(&["b.rs", "a/x.rs", "c.rs"], &shape),
            expected(&[(0, "a"), (1, "x.rs"), (0, "b.rs"), (0, "c.rs")])
        );
    }

    #[test]
    fn a_file_row_points_back_at_its_entry() {
        let paths = [PathBuf::from("b.rs"), PathBuf::from("a/x.rs")];
        let kinds: Vec<_> = rows(paths.iter().map(|path| Entry::file(path)), &AS_GIVEN)
            .into_iter()
            .map(|row| row.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                RowKind::File { index: 0 },
                RowKind::Folder {
                    open: true,
                    openable: true
                },
                RowKind::File { index: 1 },
            ]
        );
    }

    #[test]
    fn a_sealed_folder_stays_closed() {
        let entries = [Entry {
            path: Path::new("target"),
            ignored: true,
            sealed: true,
        }];
        let rows = rows(entries, &AS_GIVEN);
        assert_eq!(
            rows[0].kind,
            RowKind::Folder {
                open: false,
                openable: false
            }
        );
        assert!(rows[0].ignored);
    }
}
