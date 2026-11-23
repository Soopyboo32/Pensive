//! A square button that shows only an icon.

use gpui::{App, ElementId, SharedString, Window, div, prelude::*};
use theme::{ActiveTheme, ColorRole, Dp, IconSize, dp};

use crate::{Callback, Icon, IconName, OnClick as _, callback, tooltip::text_tooltip};

#[derive(IntoElement)]
pub struct IconButton {
    id: ElementId,
    icon: IconName,
    size: Dp,
    color: ColorRole,
    hover_fill: ColorRole,
    tooltip: Option<SharedString>,
    on_click: Option<Callback<()>>,
}

impl IconButton {
    /// A 24 px button with a 14 px icon in `text_muted`.
    pub fn new(id: impl Into<ElementId>, icon: IconName) -> Self {
        Self {
            id: id.into(),
            icon,
            size: dp(24.),
            color: |colors| colors.text_muted,
            hover_fill: |colors| colors.surface_raised,
            tooltip: None,
            on_click: None,
        }
    }

    /// The button's side. The icon stays 14 px.
    pub fn size(mut self, size: Dp) -> Self {
        self.size = size;
        self
    }

    /// Picks the icon's color role.
    pub fn color(mut self, color: ColorRole) -> Self {
        self.color = color;
        self
    }

    /// The fill under the pointer, `surface_raised` by default. A button on a raised surface
    /// needs a step above it.
    pub fn hover_fill(mut self, fill: ColorRole) -> Self {
        self.hover_fill = fill;
        self
    }

    pub fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(callback(handler));
        self
    }
}

impl RenderOnce for IconButton {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = &cx.theme().colors;
        let hover = (self.hover_fill)(colors);
        div()
            .id(self.id)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .size(self.size)
            .rounded(cx.density().radius_md)
            .hover(move |button| button.bg(hover))
            .child(
                Icon::new(self.icon)
                    .size(IconSize::Medium)
                    .color((self.color)(colors)),
            )
            .when_some(self.tooltip, |button, text| {
                button.tooltip(text_tooltip(text))
            })
            .on_click_callback(self.on_click)
    }
}

#[cfg(feature = "mocks")]
pub(crate) fn mocks() -> Vec<mock::Mock> {
    vec![
        mock::Mock::component("icon_button", "gallery", || {
            crate::Gallery::new("IconButton")
                .section(
                    "Icons",
                    [
                        ("Fetch", IconName::RefreshCw),
                        ("Settings", IconName::Settings),
                        ("Minimize", IconName::WindowMinimize),
                        ("Maximize", IconName::WindowMaximize),
                        ("Close", IconName::WindowClose),
                    ]
                    .map(|(label, icon)| (label, IconButton::new(label, icon).into_any_element())),
                )
                .into_any_element()
        })
        .size(640., 200.),
    ]
}
