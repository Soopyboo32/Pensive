//! The Changes panel: pull and push in its header, the branch as a timeline, and the commit box
//! pinned under it.

use std::sync::Arc;

use components::{
    Button, ButtonStyle, FadeText, Icon, IconButton, IconName, RingFill, SegmentedControl,
    StatusRing, TimelineLine, TimelineNode, TimelineRow,
};
use gpui::{
    AnyElement, App, Context, ListAlignment, ListState, Pixels, SharedString, Window, deferred,
    div, list, prelude::*, px,
};
use model::{Changes, Commit, FileKey, Staging, short_sha};
use strum::VariantArray;
use theme::{ActiveTheme, Dp, IconSize, Typeset, dp};

use super::{Control, EARLIER_FOLDED, FileLayout, ProjectView, Source, files::FileRow};
use crate::{PANEL, list_splice::splice_keyed, notices, view_callback, view_callback_with};

/// How far past the panel the timeline lays out rows, so scrolling doesn't show gaps.
#[expect(
    clippy::disallowed_methods,
    reason = "measured against the viewport in pixels"
)]
const OVERDRAW: Pixels = px(200.);

/// One row of the timeline. The list draws only the rows on screen, so thousands of files cost
/// no more than a screenful. Rows that compare equal keep their height, so a change measures
/// only the rows that differ.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Item {
    /// The node that stages everything, the label that folds the files, and the layout switcher.
    Uncommitted,
    UncommittedFile(FileRow),
    /// The marker where committed work starts.
    Head,
    /// The marker where pushed work starts.
    Origin,
    Commit {
        sha: SharedString,
        era: Era,
        open: bool,
    },
    /// A row of the open commit's files.
    CommitFile {
        sha: SharedString,
        era: Era,
        row: FileRow,
    },
    /// The space under the open commit's files.
    CommitFilesEnd {
        era: Era,
    },
    /// The line fading out under the last commit.
    End,
    /// Expand or Collapse under the earlier commits.
    EarlierToggle,
}

/// The timeline's rows, and what they were laid out from.
pub(super) struct Timeline {
    list: ListState,
    items: Vec<Item>,
    /// What `items` came from, to lay them out again only when it changes.
    inputs: Option<TimelineInputs>,
    /// The sizes the rows were measured at: the rem size, row height and body line height.
    metrics: Option<(Pixels, Dp, Dp)>,
}

impl Timeline {
    pub(super) fn new() -> Self {
        Self {
            list: ListState::new(0, ListAlignment::Top, OVERDRAW),
            items: Vec::new(),
            inputs: None,
            metrics: None,
        }
    }
}

/// Everything the timeline's rows depend on, besides hover and selection, which only change how
/// a row looks.
struct TimelineInputs {
    changes: Arc<Changes>,
    layout: FileLayout,
    uncommitted_open: bool,
    earlier_expanded: bool,
    open_commit: Option<SharedString>,
    folded: Vec<FileKey>,
}

impl PartialEq for TimelineInputs {
    fn eq(&self, other: &Self) -> bool {
        // Changes are replaced, never changed in place, while anything else holds them.
        Arc::ptr_eq(&self.changes, &other.changes)
            && self.layout == other.layout
            && self.uncommitted_open == other.uncommitted_open
            && self.earlier_expanded == other.earlier_expanded
            && self.open_commit == other.open_commit
            && self.folded == other.folded
    }
}

impl ProjectView {
    pub(super) fn render_changes_panel(
        &mut self,
        changes: &Arc<Changes>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        self.update_timeline(changes, window, cx);
        let colors = &cx.theme().colors;
        div()
            .flex()
            .flex_col()
            .flex_none()
            .w(PANEL)
            .border_r_1()
            .border_color(colors.border_subtle)
            .bg(colors.surface_panel)
            .child(self.header(changes, cx))
            .child(
                div().flex_1().min_h_0().pl_1().pr_2().child(
                    list(
                        self.timeline.list.clone(),
                        cx.processor(|this, index, _, cx| {
                            // The list lays each row out on its own, so it must ask for the width.
                            div()
                                .flex()
                                .flex_col()
                                .w_full()
                                .child(this.render_item(index, cx))
                                .into_any_element()
                        }),
                    )
                    .size_full(),
                ),
            )
            .child(self.commit_box(changes, cx))
    }

    /// Lays out the timeline's rows again when what they show changed, and tells the list which
    /// ones to measure again.
    fn update_timeline(&mut self, changes: &Arc<Changes>, window: &Window, cx: &App) {
        let density = cx.density();
        let metrics = (
            window.rem_size(),
            density.row_height,
            density.body.line_height,
        );
        if self
            .timeline
            .metrics
            .replace(metrics)
            .is_some_and(|measured| measured != metrics)
        {
            self.timeline.list.remeasure();
        }
        let inputs = TimelineInputs {
            changes: changes.clone(),
            layout: self.layout,
            uncommitted_open: self.uncommitted_open,
            earlier_expanded: self.earlier_expanded,
            open_commit: self.open_commit.clone(),
            folded: self.folded.clone(),
        };
        if self.timeline.inputs.as_ref() == Some(&inputs) {
            return;
        }
        let items = self.timeline_items(changes);
        splice_keyed(&self.timeline.list, &self.timeline.items, &items);
        self.timeline.items = items;
        self.timeline.inputs = Some(inputs);
    }

    /// The uncommitted work and its files, then HEAD, the branch's commits newest first with
    /// origin where pushed work starts, then the base's from before the branch, as the line
    /// fades out.
    fn timeline_items(&self, changes: &Changes) -> Vec<Item> {
        let mut items = vec![Item::Uncommitted];
        if self.uncommitted_open && !changes.uncommitted.is_empty() {
            items.extend(
                self.file_rows(&Source::Uncommitted, &changes.uncommitted)
                    .into_iter()
                    .map(Item::UncommittedFile),
            );
        }
        items.push(Item::Head);
        let mut origin_shown = changes.upstream.is_none();
        for commit in &changes.commits {
            if commit.pushed && !origin_shown {
                items.push(Item::Origin);
                origin_shown = true;
            }
            self.commit_items(commit, Era::Branch, &mut items);
        }
        let folded = !self.earlier_expanded;
        let more = changes.earlier.len() > EARLIER_FOLDED;
        let shown = if folded {
            EARLIER_FOLDED
        } else {
            changes.earlier.len()
        };
        for (index, commit) in changes.earlier.iter().take(shown).enumerate() {
            let era = if folded && more && index + 1 == shown {
                Era::Fading
            } else {
                Era::Earlier
            };
            self.commit_items(commit, era, &mut items);
        }
        // A fading commit already ends the line.
        if !(folded && more) {
            items.push(Item::End);
        }
        if more {
            items.push(Item::EarlierToggle);
        }
        items
    }

    /// The commit's row, then its files while it's open.
    fn commit_items(&self, commit: &Commit, era: Era, items: &mut Vec<Item>) {
        let open = self.open_commit.as_ref() == Some(&commit.sha);
        items.push(Item::Commit {
            sha: commit.sha.clone(),
            era,
            open,
        });
        if open {
            let source = Source::Commit(commit.sha.clone());
            items.extend(
                self.file_rows(&source, &commit.files)
                    .into_iter()
                    .map(|row| Item::CommitFile {
                        sha: commit.sha.clone(),
                        era,
                        row,
                    }),
            );
            items.push(Item::CommitFilesEnd { era });
        }
    }

    /// Draws the timeline's row at `index`, which the list asks for only while it's on screen.
    fn render_item(&mut self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let Some(item) = self.timeline.items.get(index).cloned() else {
            return div().into_any_element();
        };
        let changes = self.changes(cx);
        match item {
            Item::Uncommitted => self.uncommitted(&changes, cx).into_any_element(),
            Item::UncommittedFile(row) => file_line(
                TimelineLine::Dashed,
                self.render_file_row(&Source::Uncommitted, &changes.uncommitted, &row, cx),
            )
            .into_any_element(),
            Item::Head => marker("HEAD", TimelineLine::Dashed, cx).into_any_element(),
            Item::Origin => {
                let remote = changes
                    .upstream
                    .as_ref()
                    .map(|upstream| upstream.remote.clone());
                marker(remote.unwrap_or_default(), TimelineLine::Solid, cx).into_any_element()
            }
            Item::Commit { sha, era, .. } => match changes.commit(&sha) {
                Some(commit) => self.commit_row(commit, era, cx),
                None => div().into_any_element(),
            },
            Item::CommitFile { sha, era, row } => {
                let files = changes.commit(&sha).map_or(&[][..], |commit| &commit.files);
                let source = Source::Commit(sha);
                file_line(
                    TimelineLine::Solid,
                    self.render_file_row(&source, files, &row, cx),
                )
                .when(era == Era::Fading, |row| row.opacity(0.4))
                .into_any_element()
            }
            // A fading commit's line fades out under its files instead.
            Item::CommitFilesEnd { era: Era::Fading } => TimelineRow::new()
                .h(dp(24.))
                .above(TimelineLine::Fade)
                .opacity(0.4)
                .into_any_element(),
            Item::CommitFilesEnd { .. } => TimelineRow::new()
                .h(dp(8.))
                .line(TimelineLine::Solid)
                .into_any_element(),
            Item::End => TimelineRow::new()
                .h(dp(24.))
                .above(TimelineLine::Fade)
                .node_y(dp(24.))
                .into_any_element(),
            Item::EarlierToggle => self.earlier_toggle(cx),
        }
    }

    /// How far the branch is from its base, then fetch, pull and push.
    fn header(&self, changes: &Changes, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let density = cx.density();
        let (behind, ahead) = match &changes.upstream {
            Some(upstream) => (upstream.behind, changes.ahead_of_upstream()),
            None => (0, 0),
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .h(density.row_height + dp(12.))
            .pl_3()
            .pr_2()
            .border_b_1()
            .border_color(colors.border_subtle)
            .child(
                Icon::new(IconName::Cloud)
                    .size(IconSize::Medium)
                    .color(colors.text_faint),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .ml_1()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .typeset(density.small)
                    .text_color(colors.text_faint)
                    .children(changes.base.as_ref().map(|base| {
                        format!("{} ahead of {}", changes.ahead_of_base(), base.branch)
                    })),
            )
            .child(
                self.with_notice(
                    Control::Fetch,
                    IconButton::new("fetch", IconName::RefreshCw)
                        .on_click(self.on_use(Control::Fetch, cx)),
                ),
            )
            .when(behind > 0, |header| {
                header.child(
                    self.with_notice(
                        Control::Pull,
                        Button::new("pull", behind.to_string())
                            .icon(IconName::ArrowDownToLine)
                            .small()
                            .on_click(self.on_use(Control::Pull, cx)),
                    ),
                )
            })
            .when(ahead > 0, |header| {
                header.child(
                    self.with_notice(
                        Control::Push,
                        Button::new("push", ahead.to_string())
                            .icon(IconName::ArrowUpFromLine)
                            .small()
                            .on_click(self.on_use(Control::Push, cx)),
                    ),
                )
            })
    }

    pub(super) fn on_use(
        &self,
        control: Control,
        cx: &mut Context<Self>,
    ) -> impl Fn(&mut Window, &mut App) + 'static {
        view_callback(cx, move |this: &mut Self, _, cx| {
            this.use_control(control, cx)
        })
    }

    /// `control` with its notice in a tooltip, right-aligned to it: below in the header, above at
    /// the bottom of the panel.
    pub(super) fn with_notice(
        &self,
        control: Control,
        element: impl IntoElement,
    ) -> impl IntoElement {
        let notice = self
            .notice
            .as_ref()
            .filter(|(shown, ..)| *shown == control)
            .map(|(_, notice, _)| notices::tooltip(notice));
        div()
            .relative()
            .child(element)
            .children(notice.map(|tooltip| {
                div()
                    .absolute()
                    .right_0()
                    .map(|anchor| {
                        if control == Control::Commit {
                            anchor.bottom_full().mb_1()
                        } else {
                            anchor.top_full().mt_1()
                        }
                    })
                    .child(deferred(tooltip).priority(1))
            }))
    }

    /// The node that stages everything, the label that folds the files, and the layout switcher.
    fn uncommitted(&self, changes: &Changes, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let density = cx.density();
        let total = changes.uncommitted.len();
        let fill = match changes.staging() {
            Staging::Unstaged => RingFill::Empty,
            Staging::Partly => RingFill::Half,
            Staging::Staged => RingFill::Full,
        };
        let ring = StatusRing::new("stage-all", fill)
            .color(|colors| colors.diff_modified)
            .size(dp(14.));
        let ring = if total > 0 {
            ring.on_click(view_callback(cx, |this: &mut Self, _, cx| {
                this.toggle_all_staged(cx)
            }))
        } else {
            ring
        };
        let open = self.uncommitted_open;
        let layout = FileLayout::VARIANTS
            .iter()
            .position(|layout| *layout == self.layout);
        TimelineRow::new()
            .items_center()
            .h(density.row_height + dp(8.))
            .below(TimelineLine::Dashed)
            .node(TimelineNode::Mark(ring.into_any_element()))
            .child(
                div()
                    .id("uncommitted")
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .h(density.row_height)
                    .ml_1()
                    .px_1()
                    .rounded(density.radius_md)
                    .hover(|label| label.bg(colors.surface_raised))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.set_uncommitted_open(!open, cx)),
                    )
                    .child(
                        div()
                            .typeset(density.body)
                            .text_color(colors.text_strong)
                            .child("Uncommitted"),
                    )
                    .when(total > 0, |label| {
                        label.child(
                            Icon::new(if open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size(IconSize::Small)
                            .color(colors.text_faint),
                        )
                    })
                    .when(!open || total == 0, |label| {
                        label.child(
                            div()
                                .ml_0p5()
                                .typeset(density.small)
                                .text_color(colors.text_faint)
                                .child(match total {
                                    0 => "none".to_string(),
                                    1 => "1 file".to_string(),
                                    total => format!("{total} files"),
                                }),
                        )
                    }),
            )
            .child(
                SegmentedControl::icons(
                    "file-layout",
                    FileLayout::VARIANTS.iter().map(|layout| layout.icon()),
                    layout,
                )
                .tooltips(FileLayout::VARIANTS.iter().map(|layout| layout.label()))
                .on_select(view_callback_with(
                    cx,
                    |this: &mut Self, index: usize, _, cx| {
                        this.set_layout(FileLayout::VARIANTS[index], cx)
                    },
                )),
            )
    }

    /// Expand under the first few earlier commits, or Collapse under all of them.
    fn earlier_toggle(&self, cx: &mut Context<Self>) -> AnyElement {
        let expanded = self.earlier_expanded;
        TimelineRow::new()
            .pt_1()
            .pb_2()
            .child(
                div().ml_1().child(
                    Button::new("earlier", if expanded { "Collapse" } else { "Expand" })
                        .style(ButtonStyle::Outline)
                        .small()
                        .on_click(view_callback(cx, move |this: &mut Self, _, cx| {
                            this.set_earlier_expanded(!expanded, cx)
                        })),
                ),
            )
            .into_any_element()
    }

    fn commit_row(&self, commit: &Commit, era: Era, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let density = cx.density().clone();
        let open = self.open_commit.as_ref() == Some(&commit.sha);
        let hovered = self.hovered_commit.as_ref() == Some(&commit.sha);
        let sha = commit.sha.clone();
        let title = div()
            .id(sha.clone())
            .flex()
            .gap_1p5()
            .ml_1()
            .px_1()
            .rounded(density.radius_md)
            .hover(|title| title.bg(colors.surface_raised))
            .on_click(cx.listener({
                let sha = sha.clone();
                move |this, _, _, cx| this.toggle_commit(sha.clone(), cx)
            }))
            .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                this.hover_commit(sha.clone(), *hovered, cx)
            }))
            .typeset(density.body)
            .text_color(match era {
                Era::Branch if !commit.pushed => colors.text,
                Era::Branch => colors.text_muted,
                Era::Earlier | Era::Fading => colors.text_faint,
            })
            .map(|title| {
                if open {
                    // Open, the whole title shows, wrapped.
                    title.items_start().min_h(density.row_height).py_1().child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(colors.text_strong)
                            .child(commit.title.clone()),
                    )
                } else {
                    let fade = FadeText::new(commit.title.clone());
                    title
                        .items_center()
                        .h(density.row_height)
                        .child(if hovered {
                            fade.background(|colors| colors.surface_raised)
                        } else {
                            fade
                        })
                        .when(commit.description.is_some(), |title| {
                            title.child(description_chip(cx))
                        })
                        .child(div().flex_1())
                }
            })
            .child(
                div()
                    .flex_none()
                    .font_family(cx.theme().fonts.code.clone())
                    .typeset(density.small)
                    .line_height(density.body.line_height)
                    .text_color(colors.text_faint)
                    .child(short_sha(&commit.sha).to_owned()),
            );

        let row = TimelineRow::new().line(TimelineLine::Solid);
        let row = match era {
            // Open, its files carry the line on and fade it out.
            Era::Fading if open => row.opacity(0.4),
            Era::Fading => row.below(TimelineLine::Fade).opacity(0.4),
            Era::Branch | Era::Earlier => row,
        };
        row.node(match era {
            _ if open => TimelineNode::Open,
            Era::Branch => TimelineNode::Commit {
                pushed: commit.pushed,
            },
            Era::Earlier | Era::Fading => TimelineNode::Older,
        })
        .node_y(density.row_height / 2.)
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .child(title)
                .when(open, |body| {
                    body.children(commit.description.clone().map(|description| {
                        div()
                            .ml_1()
                            .px_1()
                            .pb_1p5()
                            .typeset(density.small)
                            .text_color(colors.text_muted)
                            .child(description)
                    }))
                }),
        )
        .into_any_element()
    }

    /// The message box and the Commit button, which stays on screen however long the list gets.
    fn commit_box(&self, changes: &Changes, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let density = cx.density();
        let staged = changes.staged_count();
        let label = match staged {
            0 => "Nothing staged".to_string(),
            1 => "Commit 1 file".to_string(),
            staged => format!("Commit {staged} files"),
        };
        div()
            .flex()
            .flex_col()
            .flex_none()
            .gap_2()
            .p_2()
            .border_t_1()
            .border_color(colors.border_subtle)
            .child(
                div()
                    .h(density.row_height * 2. - dp(4.))
                    .px_2()
                    .py_1p5()
                    .rounded(density.radius_md)
                    .border_1()
                    .border_color(colors.border_subtle)
                    .bg(colors.surface_raised)
                    .typeset(density.body)
                    .text_color(colors.text_strong)
                    .child(self.message.clone()),
            )
            .child(
                div().flex().justify_end().child(
                    self.with_notice(
                        Control::Commit,
                        Button::new("commit", label)
                            .style(ButtonStyle::Outline)
                            .shortcut("Ctrl Enter")
                            .disabled(staged == 0)
                            .on_click(|window: &mut Window, cx| {
                                window.dispatch_action(Box::new(super::CommitStaged), cx)
                            }),
                    ),
                ),
            )
    }
}

/// A row of a file list on the timeline, beside `line`.
fn file_line(line: TimelineLine, row: AnyElement) -> TimelineRow {
    TimelineRow::new()
        .line(line)
        .child(div().flex().flex_col().flex_1().min_w_0().child(row))
}

/// Where a commit sits on the timeline, which sets how it's drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Era {
    /// The branch's own.
    Branch,
    /// The base's, from before the branch started.
    Earlier,
    /// The last earlier commit shown while more are folded away.
    Fading,
}

/// A tick on the line with a label, such as HEAD where committed work starts.
fn marker(
    label: impl Into<SharedString>,
    above: TimelineLine,
    cx: &mut Context<ProjectView>,
) -> impl IntoElement {
    let colors = &cx.theme().colors;
    let density = cx.density();
    TimelineRow::new()
        .items_center()
        .h(density.row_height + dp(4.))
        .above(above)
        .below(TimelineLine::Solid)
        .node(TimelineNode::Tick)
        .child(
            div()
                .ml_2()
                .font_family(cx.theme().fonts.code.clone())
                .typeset(density.small)
                .text_color(colors.text_muted)
                .child(label.into()),
        )
}

/// Marks a commit whose message has more than its title.
fn description_chip(cx: &mut Context<ProjectView>) -> impl IntoElement {
    let colors = &cx.theme().colors;
    div()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(dp(18.))
        .h(dp(12.))
        .rounded(cx.density().radius_sm)
        .bg(colors.surface_selected)
        .child(
            Icon::new(IconName::Ellipsis)
                .size(IconSize::Small)
                .color(colors.text_muted),
        )
}
