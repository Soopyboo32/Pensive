//! The settings sidebar: the category rail with the gear at its foot, the search field, and the
//! current category's pages, with the settings of the page being read under it.

use components::{Icon, IconName, Kbd, NavItem, NavList, Rail, RailButton, RailItem};
use gpui::{
    Animation, AnimationExt, AnyElement, Context, Focusable, Window, deferred, div, ease_out_quint,
    prelude::*,
};
use theme::{ActiveTheme, Typeset, dp};

use super::{CATEGORY_IN, CATEGORY_OUT, SettingsView, Spot, Tool, outline::Category, search::Hits};
use crate::{notices, rail_gear::rail_gear, view_callback, view_callback_with};

/// A row of a category's list.
#[derive(Clone, Copy, Debug, PartialEq)]
enum ListRow {
    Page(usize),
    Setting { page: usize, setting: usize },
}

/// `category`'s pages, with page `open`'s settings under it. A page with one setting lists none,
/// since its own row already leads there.
fn list_rows(category: &Category, open: Option<usize>) -> Vec<ListRow> {
    let mut rows = Vec::new();
    for (page, entry) in category.pages.iter().enumerate() {
        rows.push(ListRow::Page(page));
        if open == Some(page) && entry.settings.len() > 1 {
            rows.extend(
                (0..entry.settings.len()).map(|setting| ListRow::Setting { page, setting }),
            );
        }
    }
    rows
}

/// The row showing `setting` of `page`: its own, or its page's when it has none.
fn row_of(rows: &[ListRow], page: usize, setting: usize) -> Option<usize> {
    let own = ListRow::Setting { page, setting };
    rows.iter()
        .position(|row| *row == own)
        .or_else(|| rows.iter().position(|row| *row == ListRow::Page(page)))
}

impl SettingsView {
    pub(super) fn rail(
        &self,
        outline: &[Category],
        hits: Option<&Hits>,
        cx: &mut Context<Self>,
    ) -> Rail {
        let items = outline
            .iter()
            .enumerate()
            .map(|(index, category)| RailItem {
                icon: category.icon,
                name: category.name.into(),
                dimmed: hits.is_some_and(|hits| !hits.in_category(index)),
            })
            .collect();
        let selected = if hits.is_some() {
            self.home()
        } else {
            self.current.category
        };
        let named = self.names_shown();
        let toggle_names = match self.names {
            true => "Hide names",
            false => "Show names",
        };
        let tool = |tool: Tool, icon: IconName, name: &'static str| {
            let notice = self
                .notice
                .as_ref()
                .filter(|(shown, ..)| *shown == tool)
                .map(|(_, notice, _)| notices::tooltip(notice));
            div()
                .relative()
                .child(
                    RailButton::new(("tool", tool as usize), icon, name)
                        .named(named)
                        .hovered(self.hovered_tool == Some(tool) && notice.is_none())
                        .on_hover(view_callback_with(
                            cx,
                            move |view: &mut Self, on: bool, _, cx| {
                                view.hovered_tool = on.then_some(tool);
                                cx.notify();
                            },
                        ))
                        .on_click(view_callback(cx, move |view: &mut Self, _, cx| {
                            view.use_tool(tool, cx)
                        })),
                )
                .children(notice.map(|tooltip| {
                    div()
                        .absolute()
                        .bottom_0()
                        .left_full()
                        .ml_1p5()
                        .child(deferred(tooltip).priority(1))
                }))
                .into_any_element()
        };
        Rail::new(items, selected)
            .previous(
                self.previous
                    .as_ref()
                    .filter(|_| hits.is_none())
                    .map(|(spot, ..)| spot.category),
            )
            .hovered(self.hovered)
            .named(self.names)
            .names_toggled(self.names_sliding.is_some().then_some(self.names_toggles))
            .tools([
                tool(Tool::OpenFile, IconName::FileCog, "Open settings.toml"),
                tool(Tool::ShowNames, IconName::PanelLeft, toggle_names),
                rail_gear(
                    true,
                    named,
                    self.gear_hovered,
                    |view: &mut Self, hovered| view.gear_hovered = hovered,
                    cx,
                ),
            ])
            .on_select(view_callback_with(cx, |view: &mut Self, index, _, cx| {
                view.show_category(index, cx)
            }))
            .on_hover(view_callback_with(cx, |view: &mut Self, index, _, cx| {
                view.hovered = index;
                cx.notify();
            }))
    }

    pub(super) fn search_field(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let colors = &cx.theme().colors;
        let density = cx.density();
        let focused = self.search.focus_handle(cx).is_focused(window);
        let hint = if self.query(cx).is_empty() {
            "/"
        } else {
            "Esc"
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .h(density.row_height)
            .mx_2p5()
            .mb_2p5()
            .px_2()
            .rounded(density.radius_md)
            .border_1()
            .border_color(if focused {
                colors.border_focus
            } else {
                colors.border
            })
            .bg(colors.surface_app)
            .typeset(density.body)
            .child(
                Icon::new(IconName::Search)
                    .size(dp(14.))
                    .color(colors.text_faint),
            )
            .child(div().flex_1().min_w_0().child(self.search.clone()))
            .child(Kbd::new(hint))
            .into_any_element()
    }

    /// The current category's pages, crossfading from the last category's.
    pub(super) fn pages_list(&self, outline: &[Category], cx: &mut Context<Self>) -> AnyElement {
        let list = |category: usize, cx: &mut Context<Self>| {
            let Some(entry) = outline.get(category) else {
                return div().into_any_element();
            };
            let current =
                (self.current.category == category).then_some((self.current.page, self.reading));
            let previous = self
                .previous
                .as_ref()
                .filter(|(spot, ..)| spot.category == category)
                .map(|(spot, setting, _)| (spot.page, *setting));
            let open = current.or(previous).map(|(page, _)| page);
            let rows = list_rows(entry, open);
            let row = |(page, setting)| row_of(&rows, page, setting);
            let selected = current.and_then(row);
            let previous = previous.and_then(row);
            let items = rows
                .iter()
                .map(|row| match *row {
                    ListRow::Page(page) => NavItem::new(entry.pages[page].name),
                    ListRow::Setting { page, setting } => {
                        NavItem::new(entry.pages[page].settings[setting].name()).nested()
                    }
                })
                .collect();
            NavList::new(("pages", category), items)
                .selected(selected)
                .previous(previous)
                .on_select(view_callback_with(
                    cx,
                    move |view: &mut Self, index: usize, window, cx| match rows[index] {
                        ListRow::Page(page) => view.show_page(Spot { category, page }, cx),
                        ListRow::Setting { page, setting } => {
                            view.show_setting(Spot { category, page }, setting, window, cx)
                        }
                    },
                ))
                .into_any_element()
        };
        let to = self.current.category;
        let Some(from) = self.fading.as_ref().map(|(from, ..)| *from) else {
            return list(to, cx);
        };
        let total = (CATEGORY_OUT + CATEGORY_IN).as_secs_f32();
        let out_share = CATEGORY_OUT.as_secs_f32() / total;
        let ease = ease_out_quint();
        div()
            .relative()
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .child(list(from, cx))
                    .with_animation(
                        ("list-out", self.changes),
                        Animation::new(CATEGORY_OUT),
                        |layer, delta| layer.opacity(1. - delta),
                    ),
            )
            .child(div().child(list(to, cx)).with_animation(
                ("list-in", self.changes),
                Animation::new(CATEGORY_OUT + CATEGORY_IN),
                move |layer, delta| {
                    let fading_in = ((delta - out_share) / (1. - out_share)).max(0.);
                    layer.opacity(ease(fading_in))
                },
            ))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use components::IconName;
    use settings::{AnySetting, Control, Setting};

    use super::*;
    use crate::settings::outline::Page;

    static ONE: Setting<bool> = Setting {
        key: "one",
        name: "One",
        description: "",
        default: true,
        control: Control::Toggle,
    };
    static TWO: Setting<bool> = Setting {
        key: "two",
        name: "Two",
        description: "",
        default: true,
        control: Control::Toggle,
    };

    fn category() -> Category {
        let page = |name, settings: Vec<&'static dyn AnySetting>| Page { name, settings };
        Category {
            name: "Category",
            icon: IconName::Settings,
            description: "",
            pages: vec![page("Single", vec![&ONE]), page("Double", vec![&ONE, &TWO])],
        }
    }

    #[test]
    fn only_the_open_page_lists_its_settings() {
        let double = ListRow::Setting {
            page: 1,
            setting: 1,
        };
        let rows = list_rows(&category(), Some(1));
        assert_eq!(rows.len(), 4);
        assert_eq!(row_of(&rows, 1, 1), rows.iter().position(|r| *r == double));
        assert_eq!(list_rows(&category(), Some(0)).len(), 2);
        assert_eq!(list_rows(&category(), None).len(), 2);
    }

    #[test]
    fn a_setting_without_a_row_shows_on_its_page() {
        let rows = list_rows(&category(), Some(0));
        assert_eq!(row_of(&rows, 1, 1), Some(1));
        assert_eq!(row_of(&rows, 0, 0), Some(0));
    }
}
