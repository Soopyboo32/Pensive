//! The Files panel: the project's tree under a header with new file, select the open file and
//! collapse all, and a filter field.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    sync::Arc,
};

use components::{Icon, IconButton, IconName, Kbd, ListRow, StatusMark};
use gpui::{
    AnyElement, App, Context, Focusable, ScrollStrategy, UniformListScrollHandle, Window, div,
    prelude::*, uniform_list,
};
use model::{Changes, FileKey, FileStatus, Source, WorktreeFiles};
use theme::{ActiveTheme, ColorRole, IconSize, Typeset, dp};

use super::{
    Control, ProjectView,
    file_tree::{TreeOptions, ancestors, rows},
    files_options::{
        DOTFILES, FOLD, FOLLOW, GIT_STATUS, GUIDES, GitStatus, Guides, IGNORED, OPEN_WITH,
        OpenWith, SORT,
    },
    tree::{Row, RowKind},
};
use crate::{PANEL, smooth_scroll::SmoothScroll, view_callback, view_callback_with};

/// The Files panel's rows, laid out again only when what they show changes, and drawn only
/// while they're on screen, so a worktree with tens of thousands of files stays quick.
pub(super) struct FileTree {
    scroll: UniformListScrollHandle,
    /// Glides `scroll` for the wheel.
    glide: SmoothScroll,
    rows: Vec<Row>,
    /// What `rows` came from.
    inputs: Option<TreeInputs>,
    /// Each uncommitted file's status, by its path.
    statuses: HashMap<PathBuf, FileStatus>,
    /// The changes `statuses` came from.
    statuses_of: Option<Arc<Changes>>,
}

impl Default for FileTree {
    fn default() -> Self {
        let scroll = UniformListScrollHandle::new();
        Self {
            glide: SmoothScroll::uniform_list(&scroll),
            scroll,
            rows: Vec::new(),
            inputs: None,
            statuses: HashMap::new(),
            statuses_of: None,
        }
    }
}

struct TreeInputs {
    files: Arc<WorktreeFiles>,
    open: HashSet<PathBuf>,
    filter: String,
    options: TreeOptions,
}

impl PartialEq for TreeInputs {
    fn eq(&self, other: &Self) -> bool {
        // The files are replaced, never changed in place.
        Arc::ptr_eq(&self.files, &other.files)
            && self.open == other.open
            && self.filter == other.filter
            && self.options == other.options
    }
}

impl ProjectView {
    pub(super) fn render_files_panel(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.update_tree(cx);
        let colors = &cx.theme().colors;
        div()
            .id("files-panel")
            .flex()
            .flex_col()
            .flex_none()
            .w(PANEL)
            .border_r_1()
            .border_color(colors.border_subtle)
            .bg(colors.surface_panel)
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                this.files_hovered = *hovered;
                cx.notify();
            }))
            .child(self.files_header(cx))
            .child(self.file_filter(window, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "file-tree",
                            self.tree.rows.len(),
                            cx.processor(|this, range, _, cx| this.tree_rows(range, cx)),
                        )
                        .size_full()
                        .px_2()
                        .pt_1()
                        .track_scroll(&self.tree.scroll),
                    )
                    .child(self.tree.glide.wheel_layer(cx)),
            )
    }

    /// Lays out the tree's rows again when the files, open folders, filter or settings changed,
    /// and the statuses again when the changes did.
    fn update_tree(&mut self, cx: &App) {
        let Some(files) = self.projects.read(cx).files(&self.worktree).cloned() else {
            self.tree.rows.clear();
            self.tree.inputs = None;
            return;
        };
        let inputs = TreeInputs {
            files,
            open: self.files_open.clone(),
            filter: self.file_filter.read(cx).text().to_string(),
            options: tree_options(cx),
        };
        if self.tree.inputs.as_ref() != Some(&inputs) {
            self.tree.rows = rows(&inputs.files, &inputs.open, &inputs.filter, &inputs.options);
            self.tree.inputs = Some(inputs);
        }
        let changes = self.changes(cx);
        if !self
            .tree
            .statuses_of
            .as_ref()
            .is_some_and(|of| Arc::ptr_eq(of, &changes))
        {
            self.tree.statuses = file_statuses(&changes);
            self.tree.statuses_of = Some(changes);
        }
    }

    /// The worktree's name, then new file, select the open file and collapse all.
    fn files_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let density = cx.density();
        let name = self
            .worktree
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let button = |id: &'static str, icon: IconName| {
            IconButton::new(id, icon).color(|colors| colors.text_faint)
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_0p5()
            .h(density.row_height + dp(12.))
            .pl_4()
            .pr_2()
            .border_b_1()
            .border_color(colors.border_subtle)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .typeset(density.body)
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(colors.text_strong)
                    .child(name),
            )
            .child(self.with_notice(
                Control::NewFile,
                button("new-file", IconName::Plus).on_click(self.on_use(Control::NewFile, cx)),
            ))
            .child(
                button("locate", IconName::LocateFixed)
                    .on_click(view_callback(cx, |this: &mut Self, _, cx| {
                        this.locate_open_file(cx)
                    })),
            )
            .child(
                button("collapse", IconName::ChevronsDownUp).on_click(view_callback(
                    cx,
                    |this: &mut Self, _, cx| {
                        this.files_open.clear();
                        cx.notify();
                    },
                )),
            )
    }

    /// The filter, with its icon in the same column as the tree's top-level arrows.
    fn file_filter(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let density = cx.density();
        let focused = self.file_filter.focus_handle(cx).is_focused(window);
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(density.row_height)
            .mx_2()
            .mt_2()
            .mb_1()
            .pl(dp(7.))
            .pr_2()
            .rounded(density.radius_md)
            .border_1()
            .border_color(if focused {
                colors.border_focus
            } else {
                colors.border_subtle
            })
            .bg(colors.surface_app)
            .typeset(density.small)
            .child(
                Icon::new(IconName::Search)
                    .size(IconSize::Medium)
                    .color(colors.text_faint),
            )
            .child(div().flex_1().min_w_0().child(self.file_filter.clone()))
            .child(Kbd::new("Ctrl P"))
    }

    /// Draws the tree's rows in `range`, which the list asks for only while they're on screen.
    fn tree_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let statuses = &self.tree.statuses;
        let style = GIT_STATUS.get(cx);
        let guides = match GUIDES.get(cx) {
            Guides::Always => true,
            Guides::OnHover => self.files_hovered,
            Guides::Off => false,
        };
        let selected = self
            .selected
            .as_ref()
            .filter(|key| key.source == Source::Uncommitted)
            .map(|key| key.path.clone());
        let start = range.start;
        self.tree.rows[range.start..range.end.min(self.tree.rows.len())]
            .iter()
            .enumerate()
            .map(|(offset, row)| {
                let index = start + offset;
                let folder = matches!(row.kind, RowKind::Folder { .. });
                let status = if folder {
                    folder_status(statuses, &row.path)
                } else {
                    statuses.get(&row.path).copied()
                };
                let path = row.path.clone();
                let base = ListRow::new(index, row.name.clone())
                    .depth(row.depth)
                    .dimmed(row.ignored)
                    .guides(guides);
                let base = match status.and_then(|status| status_color(status, style)) {
                    Some(TreeColor::Name(color)) if !folder => base.label_color(color),
                    Some(TreeColor::Name(color)) => base.trailing(StatusMark::Dot(color)),
                    Some(TreeColor::Letter(letter, color)) => {
                        base.trailing(StatusMark::Letter(letter, color))
                    }
                    None => base,
                };
                let row = match row.kind {
                    RowKind::Folder { open, openable } => {
                        base.disclosure(open).muted().when(openable, |folder| {
                            folder.on_click(view_callback_with(
                                cx,
                                move |this: &mut Self, _: usize, _, cx| {
                                    this.toggle_folder(path.clone(), cx)
                                },
                            ))
                        })
                    }
                    RowKind::File { .. } => base
                        .selected(selected.as_ref() == Some(&row.path))
                        .on_click(view_callback_with(
                            cx,
                            move |this: &mut Self, clicks: usize, _, cx| {
                                let needed = match OPEN_WITH.get(cx) {
                                    OpenWith::OneClick => 1,
                                    OpenWith::DoubleClick => 2,
                                };
                                if clicks >= needed {
                                    this.open_file(path.clone(), cx)
                                }
                            },
                        )),
                };
                // The list lays each row out on its own, so it must ask for the width.
                div()
                    .flex()
                    .flex_col()
                    .w_full()
                    .child(row)
                    .into_any_element()
            })
            .collect()
    }

    fn toggle_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !self.files_open.remove(&path) {
            self.files_open.insert(path);
        }
        cx.notify();
    }

    fn open_file(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        let key = FileKey {
            source: Source::Uncommitted,
            path,
        };
        self.select(key, None, cx);
    }

    /// Opens the folders down to the file the editor shows, and scrolls the tree to it.
    pub(super) fn locate_open_file(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self
            .selected
            .as_ref()
            .filter(|key| key.source == Source::Uncommitted)
            .map(|key| key.path.clone())
        else {
            return;
        };
        self.files_open.extend(ancestors(&path));
        let Some(files) = self.projects.read(cx).files(&self.worktree) else {
            return;
        };
        let options = tree_options(cx);
        let index = rows(files, &self.files_open, "", &options)
            .iter()
            .position(|row| row.path == path);
        if let Some(index) = index {
            self.tree.glide.stop();
            self.tree
                .scroll
                .scroll_to_item(index, ScrollStrategy::Nearest);
        }
        cx.notify();
    }

    #[cfg(feature = "mocks")]
    pub fn set_file_filter(&mut self, text: &str, cx: &mut Context<Self>) {
        self.file_filter
            .update(cx, |input, cx| input.set_text(text.to_string(), cx));
    }

    /// When following the open file, opens the folders down to the file just selected.
    pub(super) fn follow(&mut self, path: &Path, cx: &App) {
        if FOLLOW.get(cx) {
            self.files_open.extend(ancestors(path));
        }
    }
}

/// Each uncommitted file's status, by its path: the worktree's side, or else the index's.
fn file_statuses(changes: &Changes) -> HashMap<PathBuf, FileStatus> {
    changes
        .uncommitted
        .iter()
        .filter_map(|file| Some((file.path.clone(), file.worktree.or(file.index)?)))
        .collect()
}

/// The tree's options, from the panel's settings.
fn tree_options(cx: &App) -> TreeOptions {
    TreeOptions {
        dotfiles: DOTFILES.get(cx),
        ignored: IGNORED.get(cx),
        fold: FOLD.get(cx),
        sort: SORT.get(cx),
    }
}

/// The status a folder shows for the changes inside it: modified when any file changed, or added
/// when every change is new.
fn folder_status(statuses: &HashMap<PathBuf, FileStatus>, folder: &Path) -> Option<FileStatus> {
    let inside: Vec<FileStatus> = statuses
        .iter()
        .filter(|(path, _)| path.starts_with(folder))
        .map(|(_, status)| *status)
        .collect();
    if inside.is_empty() {
        None
    } else if inside
        .iter()
        .all(|status| matches!(status, FileStatus::Added | FileStatus::Untracked))
    {
        Some(FileStatus::Added)
    } else {
        Some(FileStatus::Modified)
    }
}

/// How a status shows in the tree.
enum TreeColor {
    /// A file's name in the status color, or a dot after a folder's.
    Name(ColorRole),
    /// A letter after the name, such as M.
    Letter(char, ColorRole),
}

/// How `status` shows in the tree, or nothing with Git status off.
fn status_color(status: FileStatus, style: GitStatus) -> Option<TreeColor> {
    let (letter, color): (char, ColorRole) = match status {
        FileStatus::Added => ('A', |colors| colors.diff_added),
        FileStatus::Untracked => ('U', |colors| colors.diff_added),
        FileStatus::Modified => ('M', |colors| colors.diff_modified),
        FileStatus::Renamed => ('R', |colors| colors.diff_modified),
        FileStatus::Deleted => ('D', |colors| colors.diff_removed),
        FileStatus::Conflicted => ('C', |colors| colors.diff_removed),
    };
    match style {
        GitStatus::Color => Some(TreeColor::Name(color)),
        GitStatus::Letter => Some(TreeColor::Letter(letter, color)),
        GitStatus::Off => None,
    }
}
