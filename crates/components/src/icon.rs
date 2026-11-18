//! A bundled icon, tinted like text.

use gpui::{App, Hsla, IntoElement, RenderOnce, SharedString, Styled, Window, svg};
use strum::{IntoStaticStr, VariantArray};
use theme::{ActiveTheme, Dp, IconSize};

/// A Lucide icon, named as Lucide names it, or a window control, which are drawn to match each
/// other in size where Lucide's minus, square and x don't. Each one is a file in `assets/icons`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, VariantArray, IntoStaticStr)]
#[strum(serialize_all = "kebab-case")]
pub enum IconName {
    Archive,
    ArrowDown,
    ArrowDownToLine,
    ArrowUp,
    ArrowUpFromLine,
    Bell,
    Box,
    Braces,
    Check,
    ChevronDown,
    ChevronRight,
    ChevronUp,
    ChevronsDownUp,
    ChevronsUpDown,
    CircleHelpFilled,
    CircleX,
    Cloud,
    Command,
    Download,
    Ellipsis,
    FileCog,
    FileDiff,
    FileText,
    Files,
    FoldVertical,
    Folder,
    #[strum(serialize = "folder-git-2")]
    FolderGit2,
    GitBranch,
    GitCommitHorizontal,
    GitMerge,
    GitPullRequest,
    Hash,
    List,
    ListTree,
    LocateFixed,
    Minus,
    Palette,
    PanelLeft,
    Play,
    Plus,
    RefreshCw,
    RotateCcw,
    ScrollText,
    Search,
    SearchX,
    Settings,
    Square,
    TriangleAlert,
    WindowClose,
    WindowMaximize,
    WindowMinimize,
    X,
}

impl IconName {
    pub fn path(self) -> SharedString {
        format!("icons/{}.svg", <&str>::from(self)).into()
    }
}

#[derive(IntoElement)]
pub struct Icon {
    name: IconName,
    size: Size,
    color: Option<Hsla>,
}

enum Size {
    Density(IconSize),
    Exact(Dp),
}

impl Icon {
    /// A large icon in `text_muted`.
    pub fn new(name: IconName) -> Self {
        Self {
            name,
            size: Size::Density(IconSize::Large),
            color: None,
        }
    }

    /// A size that follows the density.
    pub fn size(mut self, size: IconSize) -> Self {
        self.size = Size::Density(size);
        self
    }

    /// An exact size, for an icon sized by its own layout or one that stays put in every density.
    pub fn exact_size(mut self, size: Dp) -> Self {
        self.size = Size::Exact(size);
        self
    }

    pub fn color(mut self, color: Hsla) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for Icon {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let size = match self.size {
            Size::Density(size) => cx.density().icon(size),
            Size::Exact(size) => size,
        };
        svg()
            .path(self.name.path())
            .size(size)
            .flex_none()
            .text_color(self.color.unwrap_or(cx.theme().colors.text_muted))
    }
}

#[cfg(feature = "mocks")]
pub(crate) fn mocks() -> Vec<mock::Mock> {
    use theme::dp;

    vec![
        mock::Mock::component("icon", "gallery", || {
            let named = |size: f32| {
                IconName::VARIANTS.iter().map(move |&name| {
                    (
                        <&str>::from(name),
                        Icon::new(name).exact_size(dp(size)).into_any_element(),
                    )
                })
            };
            crate::Gallery::new("Icon")
                .section("16 px", named(16.))
                .section("20 px", named(20.))
                .into_any_element()
        })
        .size(960., 520.),
    ]
}

#[cfg(test)]
mod tests {
    use gpui::AssetSource;

    use super::*;

    #[test]
    fn every_icon_is_bundled() {
        for &name in IconName::VARIANTS {
            let file = assets::Assets.load(&name.path()).unwrap();
            assert!(file.is_some(), "{} is missing from assets", name.path());
        }
    }
}
