//! One line of the editor's text: shaped and painted with its selection, cursor and highlights,
//! and recorded so the editor can find text under the pointer.

use std::{cell::RefCell, iter::Peekable, ops::Range, rc::Rc};

use gpui::{
    App, Bounds, ElementId, GlobalElementId, Hsla, LayoutId, Pixels, ShapedLine, SharedString,
    Style, TextAlign, TextRun, UnderlineStyle, WeakEntity, Window, fill, point, prelude::*,
    relative, size,
};
use theme::{ActiveTheme, CARET, Dp, HAIRLINE, dp};

use super::Editor;

/// How wide a selected line ending shows.
const NEWLINE_WIDTH: Dp = dp(6.);
/// How close to the edge the cursor may get before the text scrolls sideways.
const SCROLL_MARGIN: Dp = dp(24.);

/// Where a line was painted last frame.
pub(super) struct LineLayout {
    pub line: usize,
    /// The byte offset of the line's start in the text.
    pub start: usize,
    pub bounds: Bounds<Pixels>,
    pub shaped: ShapedLine,
}

impl LineLayout {
    /// The text offset closest to `x`, a window position.
    pub fn offset_at(&self, x: Pixels, scroll_x: Pixels) -> usize {
        self.start
            + self
                .shaped
                .closest_index_for_x(x - self.bounds.left() + scroll_x)
    }

    /// The window position of the text offset `offset`, which must be on this line.
    pub fn x_of(&self, offset: usize, scroll_x: Pixels) -> Pixels {
        self.bounds.left() + self.shaped.x_for_index(offset - self.start) - scroll_x
    }
}

pub(super) type Layouts = Rc<RefCell<Vec<LineLayout>>>;

pub(super) struct LineElement {
    /// Without its line ending.
    pub text: SharedString,
    pub foregrounds: Vec<(Range<usize>, Hsla)>,
    /// Bytes with a background, such as words that changed.
    pub highlights: Vec<(Range<usize>, Hsla)>,
    pub scroll_x: Pixels,
    pub interaction: Option<LineInteraction>,
}

pub(super) struct LineInteraction {
    pub line: usize,
    pub start: usize,
    /// The selected bytes of this line, and whether the selection runs on past its end.
    pub selection: Option<(Range<usize>, bool)>,
    /// Where the cursor is on this line, while the editor has focus.
    pub cursor: Option<usize>,
    /// Text the input method is composing, underlined.
    pub marked: Option<Range<usize>>,
    /// Whether to scroll sideways so the cursor shows.
    pub autoscroll: Option<WeakEntity<Editor>>,
    pub layouts: Layouts,
}

impl IntoElement for LineElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for LineElement {
    type RequestLayoutState = ();
    type PrepaintState = Option<ShapedLine>;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        let style = window.text_style();
        let font_size = style.font_size.to_pixels(window.rem_size());
        let base = style.to_run(self.text.len());
        let runs = runs(
            &self.text,
            base.clone(),
            &self.foregrounds,
            &self.highlights,
            self.interaction
                .as_ref()
                .and_then(|line| line.marked.as_ref()),
        );
        let text_system = window.text_system();
        let mut shaped = text_system.shape_line(self.text.clone(), font_size, &runs, None);
        if runs
            .windows(2)
            .any(|pair| pair[0].color != pair[1].color || pair[0].underline != pair[1].underline)
        {
            // GPUI splits shaping at foreground and underline boundaries.
            // Paint the decorations on the whole-line layout so changing a
            // color cannot break a ligature or move the hit-test positions.
            *shaped = text_system.layout_line(&self.text, font_size, &[base], None);
        }
        Some(shaped)
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        shaped: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(shaped) = shaped.take() else { return };
        let colors = &cx.theme().colors;
        let (selected_color, cursor_color) = (colors.accent_solid.opacity(0.4), colors.text_strong);
        let x = |index: usize| bounds.left() + shaped.x_for_index(index) - self.scroll_x;
        if let Some((selected, past_end)) = self
            .interaction
            .as_ref()
            .and_then(|line| line.selection.as_ref())
        {
            let newline = NEWLINE_WIDTH.to_pixels(window.rem_size());
            let end = x(selected.end) + if *past_end { newline } else { Pixels::ZERO };
            window.paint_quad(fill(
                Bounds::from_corners(
                    point(x(selected.start), bounds.top()),
                    point(end, bounds.bottom()),
                ),
                selected_color,
            ));
        }
        let origin = point(bounds.left() - self.scroll_x, bounds.top());
        let line_height = window.line_height();
        shaped
            .paint_background(origin, line_height, TextAlign::Left, None, window, cx)
            .ok();
        shaped
            .paint(origin, line_height, TextAlign::Left, None, window, cx)
            .ok();
        let Some(line) = &self.interaction else {
            return;
        };
        if let Some(cursor) = line.cursor {
            let cursor_x = x(cursor);
            window.paint_quad(fill(
                Bounds::new(
                    point(cursor_x, bounds.top()),
                    size(CARET, bounds.size.height),
                ),
                cursor_color,
            ));
            if let Some(editor) = &line.autoscroll {
                self.scroll_to_show(editor, cursor_x, bounds, window, cx);
            }
        }
        line.layouts.borrow_mut().push(LineLayout {
            line: line.line,
            start: line.start,
            bounds,
            shaped,
        });
    }
}

impl LineElement {
    /// Moves the text sideways when the cursor at `cursor_x` is outside `bounds`, from the next
    /// frame, since a change made while painting doesn't draw another.
    fn scroll_to_show(
        &self,
        editor: &WeakEntity<Editor>,
        cursor_x: Pixels,
        bounds: Bounds<Pixels>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let margin = SCROLL_MARGIN
            .to_pixels(window.rem_size())
            .min(bounds.size.width / 2.);
        let scroll_x = if cursor_x > bounds.right() - margin {
            self.scroll_x + (cursor_x - (bounds.right() - margin))
        } else if cursor_x < bounds.left() + margin {
            (self.scroll_x - (bounds.left() + margin - cursor_x)).max(Pixels::ZERO)
        } else {
            return;
        };
        editor
            .update(cx, |editor, cx| editor.set_scroll_x(scroll_x, cx))
            .ok();
        window.request_animation_frame();
    }
}

/// Clip absolute bytes to the displayed line, preserving scalar boundaries.
pub(super) fn local_range(text: &str, start: usize, bytes: &Range<usize>) -> Option<Range<usize>> {
    let from = bytes.start.max(start) - start;
    let to = bytes.end.min(start + text.len()).saturating_sub(start);
    (from < to && text.is_char_boundary(from) && text.is_char_boundary(to)).then_some(from..to)
}

/// Merge ordered, disjoint ranges within each layer. Every run uses the same
/// font, even where foreground, background and the IME underline overlap.
fn runs(
    text: &str,
    base: TextRun,
    foregrounds: &[(Range<usize>, Hsla)],
    highlights: &[(Range<usize>, Hsla)],
    marked: Option<&Range<usize>>,
) -> Vec<TextRun> {
    let mut foregrounds = foregrounds.iter().cloned().peekable();
    let mut highlights = highlights.iter().cloned().peekable();
    let mut marked = marked
        .cloned()
        .map(|range| (range, ()))
        .into_iter()
        .peekable();
    let mut offset = 0;
    let mut result = Vec::new();
    while offset < text.len() {
        let mut end = text.len();
        let color = layer(&mut foregrounds, text, offset, &mut end).unwrap_or(base.color);
        let background = layer(&mut highlights, text, offset, &mut end);
        let underline = layer(&mut marked, text, offset, &mut end).map(|()| UnderlineStyle {
            color: Some(base.color),
            thickness: HAIRLINE,
            wavy: false,
        });
        result.push(TextRun {
            len: end - offset,
            color,
            background_color: background,
            underline,
            ..base.clone()
        });
        offset = end;
    }
    result
}

fn layer<T: Copy>(
    ranges: &mut Peekable<impl Iterator<Item = (Range<usize>, T)>>,
    text: &str,
    offset: usize,
    end: &mut usize,
) -> Option<T> {
    while let Some((range, value)) = ranges.peek() {
        if range.end <= offset
            || range.is_empty()
            || !text.is_char_boundary(range.start)
            || !text.is_char_boundary(range.end)
        {
            ranges.next();
        } else if range.start > offset {
            *end = (*end).min(range.start);
            return None;
        } else {
            *end = (*end).min(range.end);
            return Some(*value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use gpui::{Hsla, font};

    use super::*;

    fn base() -> TextRun {
        TextRun {
            len: 0,
            font: font("Mono"),
            color: Hsla::default(),
            background_color: None,
            underline: None,
            strikethrough: None,
        }
    }

    #[test]
    fn highlights_split_the_line_into_runs() {
        let red = Hsla::red();
        let runs = runs("0123456789", base(), &[], &[(2..5, red)], Some(&(4..6)));
        let lens: Vec<_> = runs.iter().map(|run| run.len).collect();
        assert_eq!(lens, [2, 2, 1, 1, 4]);
        assert_eq!(runs[1].background_color, Some(red));
        assert!(runs[2].underline.is_some() && runs[2].background_color.is_some());
        assert!(runs[3].underline.is_some() && runs[3].background_color.is_none());
    }

    #[test]
    fn an_empty_line_has_no_runs() {
        assert!(runs("", base(), &[], &[], None).is_empty());
    }

    #[test]
    fn unicode_foreground_word_background_and_multiline_ime_compose() {
        let source = "x\r\né😀z\r\na";
        let start = source.find('é').unwrap();
        let text = &source[start..source.find('z').unwrap() + 1];
        let syntax = local_range(text, start, &(0..start + 6)).unwrap();
        let marked = local_range(text, start, &(start + 2..source.len())).unwrap();
        let blue = Hsla::blue();
        let background = Hsla::red();
        let runs = runs(
            text,
            base(),
            &[(syntax, blue)],
            &[(2..6, background)],
            Some(&marked),
        );
        assert_eq!(
            runs.iter().map(|run| run.len).collect::<Vec<_>>(),
            [2, 4, 1]
        );
        assert_eq!(runs[1].color, blue);
        assert_eq!(runs[1].background_color, Some(background));
        assert!(runs[1].underline.is_some());
        assert_eq!(runs[2].color, base().color);
        assert!(runs[2].underline.is_some());
        assert_eq!(runs[2].background_color, None);
        assert_eq!(local_range(text, start, &(start + 1..start + 2)), None);
    }
}
