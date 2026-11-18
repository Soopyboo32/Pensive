//! Smaller sets of sample changes for the projects besides pensive. Harbor is partway through
//! merging master, so it has the statuses pensive's samples don't.

use super::{Build, SampleCommit, WorktreeSamples, base, file, origin};
use crate::{FileStatus, Staging, SymbolKind};

pub(super) fn harbor() -> WorktreeSamples {
    WorktreeSamples {
        base: base("master"),
        upstream: origin(2),
        uncommitted: vec![
            file(
                "src/gui/scale.rs",
                FileStatus::Conflicted,
                (4, 0),
                Staging::Unstaged,
            )
            .symbol(SymbolKind::Function, "ui_scale", (4, 0))
            .lines(&[
                (
                    ' ',
                    "pub fn ui_scale(window: &Window, settings: &Settings) -> f32 {",
                ),
                ('+', "<<<<<<< HEAD"),
                (' ', "    window.scale_factor() * settings.ui_scale"),
                ('+', "======="),
                ('+', "    window.scale_factor().max(settings.min_scale)"),
                ('+', ">>>>>>> master"),
                (' ', "}"),
            ]),
            file(
                "src/gui/theme/metrics.rs",
                FileStatus::Renamed,
                (1, 1),
                Staging::Staged,
            )
            .renamed_from("src/gui/metrics.rs")
            .symbol(SymbolKind::Type, "Metrics", (1, 1))
            .lines(&[
                (' ', "pub struct Metrics {"),
                ('-', "    pub row_height: u32,"),
                ('+', "    pub row_height: f32,"),
                (' ', "}"),
            ]),
            file(
                "src/gui/tooltip.rs",
                FileStatus::Modified,
                (2, 1),
                Staging::Unstaged,
            )
            .symbol(SymbolKind::Function, "Tooltip::place", (2, 1))
            .lines(&[
                (' ', "impl Tooltip {"),
                (
                    ' ',
                    "    fn place(&self, anchor: Rect, window: Rect) -> Point {",
                ),
                ('-', "        anchor.bottom_left()"),
                ('+', "        let scale = self.scale;"),
                ('+', "        window.clamp(anchor.bottom_left() * scale)"),
                (' ', "    }"),
                (' ', "}"),
            ]),
            file(
                "src/gui/widgets/scale_slider.rs",
                FileStatus::Added,
                (3, 0),
                Staging::Unstaged,
            )
            .symbol(SymbolKind::Type, "ScaleSlider", (3, 0))
            .added(&["pub struct ScaleSlider {", "    pub value: f32,", "}"]),
            file(
                "src/gui/zoom.rs",
                FileStatus::Deleted,
                (0, 3),
                Staging::Staged,
            )
            .removed(&[
                "//! Zoom steps, replaced by the UI scale.",
                "",
                "pub const STEPS: [f32; 3] = [1.0, 1.25, 1.5];",
            ]),
        ],
        commits: vec![
            SampleCommit::new(
                "a3f19c2",
                "feat: scale the GUI with the display",
                false,
                vec![
                    file(
                        "src/gui/scale.rs",
                        FileStatus::Renamed,
                        (1, 1),
                        Staging::Staged,
                    )
                    .renamed_from("src/gui/dpi.rs")
                    .symbol(SymbolKind::Function, "ui_scale", (1, 1))
                    .lines(&[
                        ('-', "pub fn dpi(window: &Window) -> f32 {"),
                        (
                            '+',
                            "pub fn ui_scale(window: &Window, settings: &Settings) -> f32 {",
                        ),
                        (' ', "    window.scale_factor() * settings.ui_scale"),
                        (' ', "}"),
                    ]),
                    file(
                        "src/gui/fixed_layout.rs",
                        FileStatus::Deleted,
                        (0, 2),
                        Staging::Staged,
                    )
                    .removed(&["//! Layout at a fixed 96 DPI.", "pub const DPI: u32 = 96;"]),
                ],
            )
            .described("Every size now goes through ui_scale, so the fixed layout is gone."),
            SampleCommit::new(
                "7be04d1",
                "fix: keep tooltips inside the window",
                true,
                vec![
                    file(
                        "src/gui/tooltip.rs",
                        FileStatus::Modified,
                        (1, 1),
                        Staging::Staged,
                    )
                    .symbol(SymbolKind::Function, "Tooltip::show", (1, 1))
                    .lines(&[
                        (' ', "fn show(&mut self, anchor: Rect) {"),
                        ('-', "    self.origin = anchor.top_left();"),
                        ('+', "    self.origin = anchor.bottom_left();"),
                        (' ', "}"),
                    ]),
                ],
            ),
        ],
    }
}

pub(super) fn lantern() -> WorktreeSamples {
    WorktreeSamples {
        base: base("main"),
        upstream: origin(0),
        uncommitted: vec![
            file(
                "src/batch.rs",
                FileStatus::Modified,
                (2, 1),
                Staging::Partly,
            )
            .symbol(SymbolKind::Function, "Batch::run", (2, 1))
            .lines(&[
                (' ', "impl Batch {"),
                ('-', "    pub fn run(self) {"),
                ('+', "    pub fn run(self, workers: usize) {"),
                ('+', "        let queue = Queue::new(workers);"),
                (' ', "    }"),
                (' ', "}"),
            ]),
            file("src/queue.rs", FileStatus::Added, (3, 0), Staging::Staged)
                .symbol(SymbolKind::Type, "Queue", (3, 0))
                .added(&["pub struct Queue {", "    workers: usize,", "}"]),
        ],
        commits: vec![
            SampleCommit::new(
                "c41d8e0",
                "feat: generate thumbnails in batches",
                false,
                vec![
                    file(
                        "src/thumbnail.rs",
                        FileStatus::Modified,
                        (1, 1),
                        Staging::Staged,
                    )
                    .symbol(SymbolKind::Function, "generate", (1, 1))
                    .lines(&[
                        ('-', "pub fn generate(image: &Image) -> Thumbnail {"),
                        ('+', "pub fn generate(images: &[Image]) -> Vec<Thumbnail> {"),
                        (' ', "    todo!()"),
                        (' ', "}"),
                    ]),
                ],
            ),
            SampleCommit::new(
                "09ab7f3",
                "refactor: read image sizes up front",
                true,
                vec![
                    file(
                        "src/image.rs",
                        FileStatus::Modified,
                        (1, 0),
                        Staging::Staged,
                    )
                    .symbol(SymbolKind::Type, "Image", (1, 0))
                    .lines(&[
                        (' ', "pub struct Image {"),
                        ('+', "    pub size: Size,"),
                        (' ', "}"),
                    ]),
                ],
            ),
        ],
    }
}

pub(super) fn glyph_ui() -> WorktreeSamples {
    WorktreeSamples {
        base: None,
        upstream: origin(1),
        uncommitted: vec![
            file(
                "src/components/Button.tsx",
                FileStatus::Modified,
                (1, 1),
                Staging::Unstaged,
            )
            .symbol(SymbolKind::Function, "Button", (1, 1))
            .lines(&[
                (' ', "export function Button({ tone }: ButtonProps) {"),
                ('-', "  return <button className={tone} />;"),
                ('+', "  return <button className={styles[tone]} />;"),
                (' ', "}"),
            ]),
            file(
                "src/tokens.css",
                FileStatus::Modified,
                (1, 1),
                Staging::Staged,
            )
            .lines(&[
                (' ', ":root {"),
                ('-', "  --radius: 4px;"),
                ('+', "  --radius: 6px;"),
                (' ', "}"),
            ]),
        ],
        commits: vec![
            SampleCommit::new(
                "d27e6b1",
                "fix: keep focus rings inside rounded corners",
                true,
                vec![
                    file(
                        "src/focus.css",
                        FileStatus::Modified,
                        (1, 1),
                        Staging::Staged,
                    )
                    .lines(&[
                        (' ', ".focusable:focus-visible {"),
                        ('-', "  outline-offset: 2px;"),
                        ('+', "  outline-offset: -1px;"),
                        (' ', "}"),
                    ]),
                ],
            ),
            SampleCommit::new(
                "4f80a3c",
                "chore: lint the component stories",
                true,
                vec![
                    file(
                        "src/components/Button.stories.tsx",
                        FileStatus::Modified,
                        (1, 1),
                        Staging::Staged,
                    )
                    .lines(&[
                        ('-', "export default { title: 'Button' }"),
                        ('+', "export default { title: 'Button' };"),
                    ]),
                ],
            ),
        ],
    }
}

/// A local repository, so it has no upstream and none of its commits are pushed.
pub(super) fn dotfiles() -> WorktreeSamples {
    WorktreeSamples {
        base: None,
        upstream: None,
        uncommitted: vec![
            file(
                ".config/nvim/init.lua",
                FileStatus::Modified,
                (1, 0),
                Staging::Unstaged,
            )
            .lines(&[
                (' ', "vim.g.mapleader = ' '"),
                ('+', "vim.opt.relativenumber = true"),
            ]),
            file(".zshrc", FileStatus::Modified, (1, 1), Staging::Unstaged).lines(&[
                (' ', "export EDITOR=nvim"),
                ('-', "alias gs='git status'"),
                ('+', "alias gs='git status --short'"),
            ]),
        ],
        commits: vec![
            SampleCommit::new(
                "e5c90aa",
                "nvim: use space as the leader",
                false,
                vec![
                    file(
                        ".config/nvim/init.lua",
                        FileStatus::Modified,
                        (1, 1),
                        Staging::Staged,
                    )
                    .lines(&[
                        ('-', "vim.g.mapleader = ','"),
                        ('+', "vim.g.mapleader = ' '"),
                    ]),
                ],
            ),
            SampleCommit::new(
                "1d7c3f8",
                "zsh: add git aliases",
                false,
                vec![
                    file(".zshrc", FileStatus::Modified, (1, 0), Staging::Staged)
                        .lines(&[(' ', "export EDITOR=nvim"), ('+', "alias gs='git status'")]),
                ],
            ),
        ],
    }
}

pub(super) fn zed() -> WorktreeSamples {
    WorktreeSamples {
        base: None,
        upstream: origin(3),
        uncommitted: vec![
            file(
                "crates/editor/src/cursor.rs",
                FileStatus::Modified,
                (1, 1),
                Staging::Unstaged,
            )
            .symbol(SymbolKind::Function, "Cursor::blink", (1, 1))
            .lines(&[
                (' ', "impl Cursor {"),
                ('-', "    const BLINK_MS: u64 = 500;"),
                ('+', "    const BLINK_MS: u64 = 530;"),
                (' ', "}"),
            ]),
            file(
                "docs/keymaps.md",
                FileStatus::Modified,
                (2, 0),
                Staging::Staged,
            )
            .symbol(SymbolKind::Heading, "Cursor", (2, 0))
            .lines(&[
                (' ', "## Cursor"),
                ('+', ""),
                ('+', "The cursor stops blinking while you type."),
            ]),
        ],
        commits: vec![
            SampleCommit::new(
                "8ac2e14",
                "editor: stop blinking while typing",
                true,
                vec![
                    file(
                        "crates/editor/src/cursor.rs",
                        FileStatus::Modified,
                        (1, 0),
                        Staging::Staged,
                    )
                    .symbol(SymbolKind::Function, "Cursor::on_input", (1, 0))
                    .lines(&[
                        (' ', "fn on_input(&mut self) {"),
                        ('+', "    self.blink_paused = true;"),
                        (' ', "}"),
                    ]),
                ],
            ),
            SampleCommit::new(
                "b60f7d9",
                "docs: list the cursor settings",
                true,
                vec![
                    file(
                        "docs/settings.md",
                        FileStatus::Modified,
                        (1, 0),
                        Staging::Staged,
                    )
                    .symbol(SymbolKind::Heading, "Editor", (1, 0))
                    .lines(&[(' ', "## Editor"), ('+', "- `cursor_blink`")]),
                ],
            ),
        ],
    }
}
