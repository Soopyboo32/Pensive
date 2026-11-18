//! Sizes that change between the Calm and Dense presets. Spacing doesn't change, so it uses
//! GPUI's spacing methods (`gap_1`, `p_3`, ...) directly.

use gpui::{FontWeight, Global, Styled};
use serde::{Deserialize, Serialize};
use strum::{IntoStaticStr, VariantArray};

use crate::{Dp, dp};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, VariantArray, IntoStaticStr, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum DensityPreset {
    /// Larger text and 30 px rows. The default.
    Calm,
    /// Smaller text and 24 px rows, for more on screen.
    Dense,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, VariantArray, IntoStaticStr)]
pub enum TextRole {
    Heading,
    Title,
    Body,
    Small,
    Label,
    Code,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, VariantArray, IntoStaticStr)]
pub enum IconSize {
    /// Inside buttons and beside small text, such as chevrons.
    Small,
    /// Header and toolbar buttons.
    Medium,
    /// Rows and the default.
    Large,
    /// The rail.
    XLarge,
}

#[derive(Clone, Copy, Debug)]
pub struct TextSize {
    pub size: Dp,
    pub line_height: Dp,
    pub weight: FontWeight,
}

#[derive(Clone, Debug)]
pub struct Density {
    pub heading: TextSize,
    pub title: TextSize,
    pub body: TextSize,
    pub small: TextSize,
    pub label: TextSize,
    /// Code text. Its line height is also the diff line height.
    pub code: TextSize,
    /// List rows such as files and commits.
    pub row_height: Dp,
    pub row_padding: Dp,
    pub row_gap: Dp,
    pub radius_sm: Dp,
    pub radius_md: Dp,
    pub radius_lg: Dp,
    pub icon_sm: Dp,
    pub icon_md: Dp,
    pub icon_lg: Dp,
    pub icon_xl: Dp,
}

impl Global for Density {}

impl DensityPreset {
    pub fn density(self) -> Density {
        let text = |size: f32, line_height: f32, weight| TextSize {
            size: dp(size),
            line_height: dp(line_height),
            weight,
        };
        match self {
            Self::Calm => Density {
                heading: text(20., 26., FontWeight::SEMIBOLD),
                title: text(16., 22., FontWeight::SEMIBOLD),
                body: text(14., 22., FontWeight::NORMAL),
                small: text(13., 18., FontWeight::NORMAL),
                label: text(12., 16., FontWeight::MEDIUM),
                code: text(14., 22., FontWeight::NORMAL),
                row_height: dp(30.),
                row_padding: dp(13.),
                row_gap: dp(11.),
                radius_sm: dp(4.),
                radius_md: dp(6.),
                radius_lg: dp(8.),
                icon_sm: dp(13.),
                icon_md: dp(15.),
                icon_lg: dp(17.),
                icon_xl: dp(19.),
            },
            Self::Dense => Density {
                heading: text(18., 24., FontWeight::SEMIBOLD),
                title: text(15., 20., FontWeight::SEMIBOLD),
                body: text(12., 16., FontWeight::NORMAL),
                small: text(12., 16., FontWeight::NORMAL),
                label: text(11., 16., FontWeight::MEDIUM),
                code: text(12., 18., FontWeight::NORMAL),
                row_height: dp(24.),
                row_padding: dp(8.),
                row_gap: dp(8.),
                radius_sm: dp(3.),
                radius_md: dp(4.),
                radius_lg: dp(6.),
                icon_sm: dp(12.),
                icon_md: dp(14.),
                icon_lg: dp(16.),
                icon_xl: dp(18.),
            },
        }
    }
}

impl Density {
    pub fn text(&self, role: TextRole) -> TextSize {
        match role {
            TextRole::Heading => self.heading,
            TextRole::Title => self.title,
            TextRole::Body => self.body,
            TextRole::Small => self.small,
            TextRole::Label => self.label,
            TextRole::Code => self.code,
        }
    }

    pub fn icon(&self, size: IconSize) -> Dp {
        match size {
            IconSize::Small => self.icon_sm,
            IconSize::Medium => self.icon_md,
            IconSize::Large => self.icon_lg,
            IconSize::XLarge => self.icon_xl,
        }
    }
}

/// Sets font size, line height and weight from one `TextSize`, so text always matches a role.
pub trait Typeset: Styled + Sized {
    fn typeset(self, text: TextSize) -> Self {
        self.text_size(text.size)
            .line_height(text.line_height)
            .font_weight(text.weight)
    }
}

impl<E: Styled> Typeset for E {}
