//! How a subject's mocks lay out as a grid of cards, shared by the atlas and the mock browser.

use gpui::{Pixels, SharedString, Size};

/// About as many columns as rows, for a grid of `count` cards.
pub fn columns(count: usize) -> usize {
    (count as f64).sqrt().ceil().max(1.) as usize
}

/// The size every card leaves room for.
pub fn largest(sizes: impl IntoIterator<Item = Size<Pixels>>) -> Size<Pixels> {
    sizes
        .into_iter()
        .fold(Size::default(), |largest, size| largest.max(&size))
}

/// The line under a subject's name: how many mocks, the largest one's size and the command that
/// renders them.
pub fn summary(subject: &str, count: usize, largest: Size<Pixels>) -> SharedString {
    format!(
        "{count} mocks · {} × {} · cargo mocks {subject}/",
        largest.width.as_f32(),
        largest.height.as_f32(),
    )
    .into()
}
