//! A tree row's Git status after its name: a dot on a folder with changes inside, or a letter
//! such as M.

use gpui::{App, SharedString, Window, div, prelude::*};
use theme::{ActiveTheme, ColorRole, Typeset, dp};

#[derive(IntoElement, Clone, Copy)]
pub enum StatusMark {
    Dot(ColorRole),
    Letter(char, ColorRole),
}

impl RenderOnce for StatusMark {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = &cx.theme().colors;
        match self {
            Self::Dot(color) => div().size(dp(6.)).rounded_full().bg(color(colors)),
            Self::Letter(letter, color) => div()
                .font_family(cx.theme().fonts.code.clone())
                .typeset(cx.density().label)
                .text_color(color(colors))
                .child(SharedString::from(letter.to_string())),
        }
    }
}
