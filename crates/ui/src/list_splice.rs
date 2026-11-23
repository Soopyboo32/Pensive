//! Tells a [`ListState`] which of its rows changed, so it measures only those again and keeps
//! its scroll position.

use gpui::ListState;

/// Splices the rows of `list` between the longest run that `old` and `new` share at the start
/// and the longest they share at the end, where `same` says two rows keep the same height.
pub(crate) fn splice_changed<T>(
    list: &ListState,
    old: &[T],
    new: &[T],
    same: impl Fn(&T, &T) -> bool,
) {
    let prefix = old
        .iter()
        .zip(new)
        .take_while(|(old, new)| same(old, new))
        .count();
    let longest = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(longest)
        .take_while(|(old, new)| same(old, new))
        .count();
    if prefix + suffix < old.len().max(new.len()) {
        list.splice(prefix..old.len() - suffix, new.len() - prefix - suffix);
    }
}
