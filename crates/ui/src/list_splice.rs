//! Tells a [`ListState`] which of its rows changed, so it measures only those again and keeps
//! its scroll position.

use std::{collections::HashSet, hash::Hash, ops::Range};

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

/// Splices each run of rows that changed between `old` and `new` on its own, for rows that each
/// say what they are and how tall they'll be, so no two rows are the same. A list moves its
/// scroll position to the start of any run spliced around it, so one splice from the first
/// change to the last would jump the list away from a row between them.
pub(crate) fn splice_keyed<T: Eq + Hash>(list: &ListState, old: &[T], new: &[T]) {
    // From the last, so the runs before stay where they were.
    for (range, count) in changed_runs(old, new).into_iter().rev() {
        list.splice(range, count);
    }
}

/// Each run of `old` that's gone or replaced, with how many rows of `new` take its place.
fn changed_runs<T: Eq + Hash>(old: &[T], new: &[T]) -> Vec<(Range<usize>, usize)> {
    let in_old: HashSet<&T> = old.iter().collect();
    let in_new: HashSet<&T> = new.iter().collect();
    let mut runs = Vec::new();
    let (mut at_old, mut at_new) = (0, 0);
    while at_old < old.len() || at_new < new.len() {
        if at_old < old.len() && at_new < new.len() && old[at_old] == new[at_new] {
            at_old += 1;
            at_new += 1;
            continue;
        }
        let (start_old, start_new) = (at_old, at_new);
        while at_old < old.len() && !in_new.contains(&old[at_old]) {
            at_old += 1;
        }
        while at_new < new.len() && !in_old.contains(&new[at_new]) {
            at_new += 1;
        }
        if (at_old, at_new) == (start_old, start_new) {
            // Both rows are still there but have moved, or one side has run out: replace them.
            if at_old == old.len() || at_new == new.len() {
                (at_old, at_new) = (old.len(), new.len());
            } else {
                at_old += 1;
                at_new += 1;
            }
        }
        runs.push((start_old..at_old, at_new - start_new));
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies `runs` to `old` with `new`'s rows, as the list would.
    fn apply(old: &[char], new: &[char], runs: &[(Range<usize>, usize)]) -> Vec<char> {
        let mut result = Vec::new();
        let (mut at_old, mut at_new) = (0, 0);
        for (range, count) in runs {
            let kept = range.start - at_old;
            result.extend(&old[at_old..range.start]);
            at_new += kept;
            result.extend(&new[at_new..at_new + count]);
            (at_old, at_new) = (range.end, at_new + count);
        }
        result.extend(&old[at_old..]);
        result
    }

    fn runs(old: &str, new: &str) -> Vec<(Range<usize>, usize)> {
        let (old, new): (Vec<char>, Vec<char>) = (old.chars().collect(), new.chars().collect());
        let runs = changed_runs(&old, &new);
        assert_eq!(apply(&old, &new, &runs), new, "the runs turn old into new");
        runs
    }

    #[test]
    fn opening_one_commit_while_another_is_open_splices_both_apart() {
        // A's files 1 and 2 go, and B's 3 and 4 arrive, with c between them untouched.
        assert_eq!(runs("A12cB", "acb34"), [(0..3, 1), (4..5, 3)]);
    }

    #[test]
    fn nothing_changed_splices_nothing() {
        assert!(runs("abc", "abc").is_empty());
    }

    #[test]
    fn moved_rows_and_uneven_ends_still_end_up_as_new() {
        runs("abcd", "dcba");
        runs("ab", "abcde");
        runs("abcde", "b");
        runs("", "xyz");
        runs("xyz", "");
    }
}
