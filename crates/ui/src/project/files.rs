//! The rows of a list of changed files, as a list, a tree or by symbol.

use components::{DiffStat, Icon, IconName, ListRow, RingFill, StatusRing, Tone, Tooltip};
use gpui::{AnyElement, App, Context, ElementId, Window, div, prelude::*};
use model::{FileChange, FileStatus, Staging, SymbolKind};
use theme::{ActiveTheme, ColorRole, IconSize};

use crate::{view_callback, view_callback_with};

use super::{
    FileKey, FileLayout, ProjectView, Source,
    tree::{Entry, Open, RowKind, Shape, rows},
};

/// One row of a list of changed files in the current layout. Rows are laid out once and drawn
/// only while they're on screen, so a commit with thousands of files stays quick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum FileRow {
    /// The file at this index of the list, `depth` folders in.
    File { index: usize, depth: usize },
    /// A folder of the tree layout, at `position` among its rows.
    Folder {
        name: String,
        depth: usize,
        position: usize,
    },
    /// A symbol of the file at index `file`.
    Symbol { file: usize, symbol: usize },
}

impl ProjectView {
    /// The rows `files` from `source` lay out as.
    pub(super) fn file_rows(&self, source: &Source, files: &[FileChange]) -> Vec<FileRow> {
        match self.layout {
            FileLayout::List => (0..files.len())
                .map(|index| FileRow::File { index, depth: 0 })
                .collect(),
            FileLayout::Tree => {
                let shape = Shape {
                    open: Open::All,
                    fold: true,
                    sort: None,
                };
                rows(files.iter().map(|file| Entry::file(&file.path)), &shape)
                    .into_iter()
                    .enumerate()
                    .map(|(position, row)| match row.kind {
                        RowKind::Folder { .. } => FileRow::Folder {
                            name: row.name,
                            depth: row.depth,
                            position,
                        },
                        RowKind::File { index } => FileRow::File {
                            index,
                            depth: row.depth,
                        },
                    })
                    .collect()
            }
            FileLayout::Symbols => {
                let mut rows = Vec::new();
                for (index, file) in files.iter().enumerate() {
                    rows.push(FileRow::File { index, depth: 0 });
                    if !self.folded.contains(&file_key(source, file)) {
                        rows.extend((0..file.symbols.len()).map(|symbol| FileRow::Symbol {
                            file: index,
                            symbol,
                        }));
                    }
                }
                rows
            }
        }
    }

    /// Draws `row` of the list `files` from `source`.
    pub(super) fn render_file_row(
        &self,
        source: &Source,
        files: &[FileChange],
        row: &FileRow,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match row {
            FileRow::Folder {
                name,
                depth,
                position,
            } => ListRow::new(("folder", *position), name.clone())
                .depth(*depth)
                .disclosure(true)
                .muted()
                .into_any_element(),
            FileRow::File { index, depth } => {
                let Some(file) = files.get(*index) else {
                    return div().into_any_element();
                };
                let key = file_key(source, file);
                let row = self.file_row(key.clone(), file, *depth, cx);
                if self.layout != FileLayout::Symbols {
                    return row.into_any_element();
                }
                let open = !self.folded.contains(&key);
                let toggle = view_callback(
                    cx,
                    move |this: &mut Self, _: &mut Window, cx: &mut Context<Self>| {
                        this.toggle_folded(key.clone(), cx)
                    },
                );
                row.disclosure(open)
                    .on_disclosure(toggle)
                    .into_any_element()
            }
            FileRow::Symbol { file, symbol } => match files.get(*file) {
                Some(file_change) if *symbol < file_change.symbols.len() => {
                    self.symbol_row(&file_key(source, file_change), file_change, *symbol, cx)
                }
                _ => div().into_any_element(),
            },
        }
    }

    fn file_row(
        &self,
        key: FileKey,
        file: &FileChange,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> ListRow {
        let fill = match file.staging() {
            Staging::Unstaged => RingFill::Empty,
            Staging::Partly => RingFill::Half,
            Staging::Staged => RingFill::Full,
        };
        let ring =
            StatusRing::new(element_id("ring", &key), fill).color(status_color(file.status()));
        let ring = if key.source == Source::Uncommitted {
            let path = key.path.clone();
            ring.on_click(view_callback(cx, move |this: &mut Self, _, cx| {
                this.toggle_staged(&path, cx)
            }))
        } else {
            ring
        };
        let selected = self.selected.as_ref() == Some(&key) && self.selected_symbol.is_none();
        let row = ListRow::new(element_id("file", &key), file.name())
            .depth(depth)
            .leading(ring)
            .trailing(DiffStat::new(file.added, file.removed))
            .selected(selected)
            .hovered(self.hovered.as_ref() == Some(&key))
            .on_click(view_callback_with(cx, {
                let key = key.clone();
                move |this: &mut Self, _: usize, _: &mut Window, cx: &mut Context<Self>| {
                    this.select(key.clone(), None, cx)
                }
            }))
            .on_hover(view_callback_with(cx, {
                let key = key.clone();
                move |this: &mut Self, hovered: bool, _: &mut Window, cx: &mut Context<Self>| {
                    this.hover(key.clone(), hovered, cx)
                }
            }));
        if self.tooltip.as_ref() != Some(&key) {
            row
        } else if self.copied.is_some() {
            row.below(Tooltip::new("Copied path").tone(Tone::Success))
        } else {
            row.below(Tooltip::new(path_label(file, cx)).shortcut("Copy", "Ctrl Shift C"))
        }
    }

    fn symbol_row(
        &self,
        key: &FileKey,
        file: &FileChange,
        index: usize,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let symbol = &file.symbols[index];
        let icon = match symbol.kind {
            SymbolKind::Function => IconName::Braces,
            SymbolKind::Type => IconName::Box,
            SymbolKind::Heading => IconName::Hash,
        };
        let selected = self.selected.as_ref() == Some(key) && self.selected_symbol == Some(index);
        ListRow::new(
            ElementId::Name(format!("{}-symbol-{index}", element_name(key)).into()),
            symbol.name.clone(),
        )
        // One step in, past the file's arrow, so the symbol's icon lines up under the file's ring.
        .depth(1)
        .compact()
        .muted()
        .when(symbol.kind != SymbolKind::Heading, ListRow::code)
        .leading(
            Icon::new(icon)
                .size(IconSize::Small)
                .color(cx.theme().colors.text_faint),
        )
        .trailing(DiffStat::new(symbol.added, symbol.removed))
        .selected(selected)
        .on_click(view_callback_with(cx, {
            let key = key.clone();
            move |this: &mut Self, _: usize, _: &mut Window, cx: &mut Context<Self>| {
                this.select(key.clone(), Some(index), cx)
            }
        }))
        .into_any_element()
    }
}

/// New files are green, changed or moved ones amber, and deleted or conflicted ones red.
fn status_color(status: FileStatus) -> ColorRole {
    match status {
        FileStatus::Added | FileStatus::Untracked => |colors| colors.diff_added,
        FileStatus::Modified | FileStatus::Renamed => |colors| colors.diff_modified,
        FileStatus::Deleted | FileStatus::Conflicted => |colors| colors.diff_removed,
    }
}

/// The file's folder in faint text, then its name.
fn path_label(file: &FileChange, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let folder = file.folder();
    div()
        .flex()
        .font_family(theme.fonts.code.clone())
        .when(!folder.is_empty(), |label| {
            label.child(
                div()
                    .text_color(theme.colors.text_faint)
                    .child(format!("{folder}/")),
            )
        })
        .child(
            div()
                .text_color(theme.colors.text_strong)
                .child(file.name()),
        )
}

fn file_key(source: &Source, file: &FileChange) -> FileKey {
    FileKey {
        source: source.clone(),
        path: file.path.clone(),
    }
}

fn element_name(key: &FileKey) -> String {
    let path = key.path.display();
    match &key.source {
        Source::Uncommitted => format!("uncommitted-{path}"),
        Source::Commit(sha) => format!("{sha}-{path}"),
    }
}

fn element_id(kind: &str, key: &FileKey) -> ElementId {
    ElementId::Name(format!("{}-{kind}", element_name(key)).into())
}
