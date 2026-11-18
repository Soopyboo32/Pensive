//! One row of a list or tree: indent guides, a disclosure arrow, a leading mark, a label and
//! trailing content, with hover and selected states. Every level of a tree steps in by one arrow
//! and its gap, so a child starts where its parent's label starts.

use std::rc::Rc;

use gpui::{AnyElement, App, ElementId, SharedString, Window, deferred, div, prelude::*};
use theme::{ActiveTheme, ColorRole, Density, Dp, HAIRLINE, Typeset, dp};

use crate::{Callback, Icon, IconName, callback};

#[derive(IntoElement)]
pub struct ListRow {
    id: ElementId,
    depth: usize,
    guides: bool,
    disclosure: Option<bool>,
    leading: Option<AnyElement>,
    label: SharedString,
    label_color: Option<ColorRole>,
    code: bool,
    muted: bool,
    dimmed: bool,
    compact: bool,
    trailing: Option<AnyElement>,
    below: Option<AnyElement>,
    selected: bool,
    hovered: bool,
    on_click: Option<Callback<usize>>,
    on_disclosure: Option<Callback<()>>,
    on_hover: Option<Callback<bool>>,
}

impl ListRow {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            depth: 0,
            guides: true,
            disclosure: None,
            leading: None,
            label: label.into(),
            label_color: None,
            code: false,
            muted: false,
            dimmed: false,
            compact: false,
            trailing: None,
            below: None,
            selected: false,
            hovered: false,
            on_click: None,
            on_disclosure: None,
            on_hover: None,
        }
    }

    /// A disclosure arrow, and the size a leading mark is centered in: half a row, so it shrinks
    /// with the density.
    fn mark(density: &Density) -> Dp {
        density.row_height / 2.
    }

    /// The slot an arrow or a leading mark takes, with its gap to what follows, and how far each
    /// level of a tree steps in: 20 dp in Calm and 16 in Dense.
    pub fn step(density: &Density) -> Dp {
        density.row_height - dp(8.)
    }

    /// How many levels in, each one step.
    pub fn depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    /// A line under each ancestor's arrow. On by default.
    pub fn guides(mut self, guides: bool) -> Self {
        self.guides = guides;
        self
    }

    /// An arrow before the leading mark, pointing down when `open`.
    pub fn disclosure(mut self, open: bool) -> Self {
        self.disclosure = Some(open);
        self
    }

    /// A mark before the label, such as a status ring or an icon, centered in one step.
    pub fn leading(mut self, leading: impl IntoElement) -> Self {
        self.leading = Some(leading.into_any_element());
        self
    }

    /// The label's color, such as a file's Git status, unless the row is dimmed.
    pub fn label_color(mut self, color: ColorRole) -> Self {
        self.label_color = Some(color);
        self
    }

    /// Draws the label in the code font.
    pub fn code(mut self) -> Self {
        self.code = true;
        self
    }

    /// Small text in a row 4 px shorter, for rows nested under another, such as symbols.
    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    /// The label in `text_muted` unless the row is selected, such as for folders.
    pub fn muted(mut self) -> Self {
        self.muted = true;
        self
    }

    /// The label and arrow in `text_disabled`, for what `.gitignore` leaves out.
    pub fn dimmed(mut self, dimmed: bool) -> Self {
        self.dimmed = dimmed;
        self
    }

    pub fn trailing(mut self, trailing: impl IntoElement) -> Self {
        self.trailing = Some(trailing.into_any_element());
        self
    }

    /// Shown under the label and drawn above every pane, such as a tooltip.
    pub fn below(mut self, below: impl IntoElement) -> Self {
        self.below = Some(below.into_any_element());
        self
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Shows the hover state without a pointer, for mocks.
    pub fn hovered(mut self, hovered: bool) -> Self {
        self.hovered = hovered;
        self
    }

    /// Called with the click count, so a view can act on the second click.
    pub fn on_click(mut self, handler: impl Fn(usize, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Rc::new(handler));
        self
    }

    /// Called when the arrow is clicked, instead of the row's own click.
    pub fn on_disclosure(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_disclosure = Some(callback(handler));
        self
    }

    /// Called when the pointer enters or leaves the row.
    pub fn on_hover(mut self, handler: impl Fn(bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_hover = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ListRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let colors = &theme.colors;
        let density = cx.density();
        let (mark, step) = (Self::mark(density), Self::step(density));
        let height = if self.compact {
            density.row_height - dp(4.)
        } else {
            density.row_height
        };
        let label_color = match self.label_color {
            _ if self.dimmed => colors.text_disabled,
            Some(color) => color(colors),
            None if self.selected => colors.text_strong,
            None if self.muted => colors.text_muted,
            None => colors.text,
        };
        let slots = usize::from(self.disclosure.is_some()) + usize::from(self.leading.is_some());
        let label_start = step * (self.depth + slots) as f32;
        let slot = || div().flex().flex_none().items_center().w(step);

        div()
            .id(self.id)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h(height)
            .px_2()
            .rounded(density.radius_md)
            .map(|row| {
                if self.selected {
                    row.bg(colors.surface_selected)
                } else if self.hovered {
                    row.bg(colors.surface_raised)
                } else {
                    row.hover(|row| row.bg(colors.surface_raised))
                }
            })
            .child(
                div()
                    .relative()
                    .flex_none()
                    .self_stretch()
                    .w(step * self.depth as f32)
                    .when(self.guides, |indent| {
                        indent.children((0..self.depth).map(|level| {
                            div()
                                .absolute()
                                .top_0()
                                .bottom_0()
                                .left(step * level as f32 + mark / 2.)
                                .w(HAIRLINE)
                                .bg(colors.border_subtle)
                        }))
                    }),
            )
            .children(self.disclosure.map(|open| {
                let arrow = slot().id("disclosure").child(
                    Icon::new(if open {
                        IconName::ChevronDown
                    } else {
                        IconName::ChevronRight
                    })
                    .exact_size(mark)
                    .color(if self.dimmed {
                        colors.text_disabled
                    } else {
                        colors.text_faint
                    }),
                );
                match self.on_disclosure.clone() {
                    Some(handler) => arrow
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            handler((), window, cx)
                        })
                        .into_any_element(),
                    None => arrow.into_any_element(),
                }
            }))
            .children(self.leading.map(|leading| {
                slot().child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .w(mark)
                        .child(leading),
                )
            }))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(self.code, |label| {
                        label.font_family(theme.fonts.code.clone())
                    })
                    .typeset(if self.compact {
                        density.small
                    } else {
                        density.body
                    })
                    .text_color(label_color)
                    .child(self.label),
            )
            .children(
                self.trailing
                    .map(|trailing| div().flex_none().ml_2().child(trailing)),
            )
            .children(self.below.map(|below| {
                div()
                    .absolute()
                    .top_full()
                    .left(dp(8.) + label_start - dp(2.))
                    .mt_0p5()
                    .child(deferred(below).priority(1))
            }))
            .when_some(self.on_click, |row, handler| {
                row.on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    handler(event.click_count(), window, cx)
                })
            })
            .when_some(self.on_hover, |row, handler| {
                row.on_hover(move |hovered, window, cx| handler(*hovered, window, cx))
            })
    }
}

#[cfg(feature = "mocks")]
pub(crate) fn mocks() -> Vec<mock::Mock> {
    use theme::IconSize;

    use crate::{DiffStat, RingFill, StatusMark, StatusRing, Tooltip, gallery::Panel};

    fn tree() -> Vec<ListRow> {
        let folder = |id: &'static str, name: &'static str, depth, open| {
            ListRow::new(id, name).depth(depth).disclosure(open).muted()
        };
        let file =
            |id: &'static str, name: &'static str, depth| ListRow::new(id, name).depth(depth);
        vec![
            folder("assets", "assets", 0, false),
            folder("crates", "crates", 0, true)
                .trailing(StatusMark::Dot(|colors| colors.diff_modified)),
            folder("src", "components/src", 1, true)
                .trailing(StatusMark::Dot(|colors| colors.diff_modified)),
            file("icon", "icon.rs", 2),
            file("palette", "palette.rs", 2)
                .label_color(|colors| colors.diff_modified)
                .selected(true),
            file("stepper", "stepper.rs", 2).label_color(|colors| colors.diff_added),
            file("toggle", "toggle.rs", 2).hovered(true),
            folder("theme", "theme", 1, false),
            file("cargo", "Cargo.toml", 1),
            folder("target", "target", 0, false).dimmed(true),
            file("lock", "Cargo.lock", 0)
                .trailing(StatusMark::Letter('M', |colors| colors.diff_modified)),
            file("readme", "README.md", 0),
        ]
    }

    vec![
        mock::Mock::component("list_row", "gallery", || {
            let file = |id: &'static str, name: &'static str, fill| {
                ListRow::new(id, name)
                    .leading(StatusRing::new("ring", fill).color(|colors| colors.diff_modified))
                    .trailing(DiffStat::new(4, 1))
            };
            let column = |rows: Vec<ListRow>| Panel::new(dp(256.), rows).into_any_element();
            crate::Gallery::new("ListRow")
                .section(
                    "States",
                    [
                        (
                            "Resting",
                            column(vec![file("a", "palette.rs", RingFill::Full)]),
                        ),
                        (
                            "Hovered",
                            column(vec![file("b", "palette.rs", RingFill::Empty).hovered(true)]),
                        ),
                        (
                            "Selected",
                            column(vec![file("c", "palette.rs", RingFill::Full).selected(true)]),
                        ),
                    ],
                )
                .section(
                    "Nested, with guides",
                    [
                        (
                            "Folders and files",
                            column(vec![
                                ListRow::new("d", "crates/ui/src").disclosure(true).muted(),
                                ListRow::new("e", "palette")
                                    .depth(1)
                                    .disclosure(true)
                                    .muted(),
                                file("f", "fuzzy.rs", RingFill::Empty).depth(2),
                                file("g", "root.rs", RingFill::Empty).depth(1),
                            ]),
                        ),
                        (
                            "A file's symbols",
                            column(vec![
                                file("h", "root.rs", RingFill::Empty).disclosure(true),
                                ListRow::new("i", "Pensive::render")
                                    .depth(1)
                                    .compact()
                                    .code()
                                    .leading(
                                        crate::Icon::new(IconName::Braces).size(IconSize::Small),
                                    )
                                    .trailing(DiffStat::new(9, 2)),
                            ]),
                        ),
                    ],
                )
                .section(
                    "A project's tree",
                    [
                        ("Status as color, a letter on Cargo.lock", column(tree())),
                        (
                            "Without guides",
                            column(tree().into_iter().map(|row| row.guides(false)).collect()),
                        ),
                    ],
                )
                .section(
                    "With a tooltip below",
                    [(
                        "Hovered, path shown",
                        div()
                            .h(dp(64.))
                            .child(column(vec![
                                file("j", "row.rs", RingFill::Full).hovered(true).below(
                                    Tooltip::new("crates/components/src/palette/row.rs")
                                        .shortcut("Copy", "Ctrl Shift C"),
                                ),
                            ]))
                            .into_any_element(),
                    )],
                )
                .into_any_element()
        })
        .size(960., 980.),
    ]
}
