use std::{
    ops::Range,
    rc::{Rc, Weak},
};

use gpui::{Context, Task};
use language::{SyntaxCancellation, SyntaxState, TokenSpan, validate_syntax_ranges};
use text::{InvalidOffset, TextChange};

use super::{Document, DocumentSide, DocumentSnapshot};

const EDIT_HISTORY_LIMIT: usize = 128;

/// Identifies an editor's current attachment to a document. Dropping it
/// releases its claim on query coverage.
#[derive(Clone, Default)]
pub struct SyntaxViewport(Rc<()>);

#[derive(Default)]
pub(super) struct DocumentSyntax {
    base: Side,
    new: Side,
    changes: Vec<TextChange>,
}

#[derive(Default)]
struct Side {
    viewports: Vec<(Weak<()>, Vec<Range<usize>>)>,
    cache: Cache,
    worker: Option<Worker>,
    pending: Option<Request>,
    result: Option<(Request, Vec<TokenSpan>)>,
}

#[derive(Default)]
struct Cache {
    input: Option<DocumentSnapshot>,
    syntax: SyntaxState,
}

#[derive(Clone)]
struct Request {
    input: DocumentSnapshot,
    ranges: Vec<Range<usize>>,
}

impl Side {
    fn latest(&self) -> Option<&Request> {
        self.pending
            .as_ref()
            .or_else(|| {
                self.worker
                    .as_ref()
                    .filter(|worker| !worker.cancel.is_cancelled())
                    .map(|worker| &worker.request)
            })
            .or_else(|| self.result.as_ref().map(|(request, _)| request))
    }
}

struct Worker {
    request: Request,
    cancel: SyntaxCancellation,
    _task: Task<()>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

impl DocumentSyntax {
    fn side(&self, side: DocumentSide) -> &Side {
        match side {
            DocumentSide::Base => &self.base,
            DocumentSide::New => &self.new,
        }
    }

    fn side_mut(&mut self, side: DocumentSide) -> &mut Side {
        match side {
            DocumentSide::Base => &mut self.base,
            DocumentSide::New => &mut self.new,
        }
    }
}

impl DocumentSnapshot {
    pub(super) fn same_context(&self, other: &Self) -> bool {
        self.source == other.source
            && self.present == other.present
            && self.language == other.language
    }
}

impl Document {
    /// Request syntax for a half-open UTF-8 byte range in one diff side. The
    /// latest request on that side takes precedence. Completion notifies observers.
    pub fn request_syntax(
        &mut self,
        side: DocumentSide,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) -> Result<(), InvalidOffset> {
        self.request_syntax_ranges(side, vec![range], cx)
    }

    /// Request one batch of ordered, disjoint ranges on a side. Gaps need not
    /// be queried even though the parser retains full-file context.
    pub fn request_syntax_ranges(
        &mut self,
        side: DocumentSide,
        ranges: Vec<Range<usize>>,
        cx: &mut Context<Self>,
    ) -> Result<(), InvalidOffset> {
        let input = self.source_snapshot(side);
        validate_syntax_ranges(&input.text, &ranges)?;
        let state = self.syntax.side_mut(side);
        let matches =
            |request: &Request| request.ranges == ranges && request.input.same_input(&input);
        if state.latest().is_some_and(matches) {
            return Ok(());
        }
        if let Some(worker) = &state.worker {
            worker.cancel.cancel();
        }
        // A scroll back can reuse the last completed window while the
        // superseded query drains.
        if state
            .result
            .as_ref()
            .is_some_and(|(request, _)| matches(request))
        {
            state.pending = None;
            return Ok(());
        }
        let plain =
            ranges.iter().all(Range::is_empty) || !input.present || input.language.syntax.is_none();
        let request = Request { input, ranges };
        if plain {
            state.pending = None;
            state.result = Some((request, Vec::new()));
            cx.notify();
        } else {
            state.pending = Some(request);
            self.start_syntax(side, cx);
        }
        Ok(())
    }

    /// Tokens for the requested range of the current input. Returns `None`
    /// while work is pending or stale, and an empty slice for plain text.
    pub fn syntax_tokens(&self, side: DocumentSide, range: Range<usize>) -> Option<&[TokenSpan]> {
        let state = self.syntax.side(side);
        let (request, spans) = state.result.as_ref()?;
        (std::ptr::eq(state.latest()?, request)
            && request.ranges == [range]
            && request.input.same_input(&self.source_snapshot(side)))
        .then_some(spans.as_slice())
    }

    /// Whether this viewport needs to replace its padded coverage.
    pub fn syntax_viewport_needs(
        &self,
        viewport: &SyntaxViewport,
        side: DocumentSide,
        visible: &[Range<usize>],
    ) -> bool {
        let viewport = Rc::downgrade(&viewport.0);
        let previous = self
            .syntax
            .side(side)
            .viewports
            .iter()
            .find(|(owner, _)| owner.ptr_eq(&viewport));
        if visible.is_empty() {
            return previous.is_some();
        }
        let input = self.source_snapshot(side);
        if !input.present || input.language.syntax.is_none() {
            return false;
        }
        let Some((_, ranges)) = previous else {
            return true;
        };
        let mut ranges = ranges.iter().peekable();
        visible.iter().any(|visible| {
            while ranges
                .peek()
                .is_some_and(|range| range.end <= visible.start)
            {
                ranges.next();
            }
            ranges
                .peek()
                .is_none_or(|range| visible.start < range.start || visible.end > range.end)
        })
    }

    /// Query the union of live editor viewports so two windows on the same
    /// document share the parser and cannot cancel each other's coverage.
    pub fn request_viewport_syntax(
        &mut self,
        viewport: &SyntaxViewport,
        side: DocumentSide,
        ranges: Vec<Range<usize>>,
        cx: &mut Context<Self>,
    ) -> Result<(), InvalidOffset> {
        validate_syntax_ranges(&self.source_snapshot(side).text, &ranges)?;
        let viewport = Rc::downgrade(&viewport.0);
        let viewports = &mut self.syntax.side_mut(side).viewports;
        viewports.retain(|(owner, _)| owner.strong_count() > 0 && !owner.ptr_eq(&viewport));
        if !ranges.is_empty() {
            viewports.push((viewport, ranges));
        }
        let mut ranges: Vec<_> = viewports
            .iter()
            .flat_map(|(_, ranges)| ranges.iter().cloned())
            .collect();
        ranges.sort_unstable_by_key(|range| range.start);
        let mut union: Vec<Range<usize>> = Vec::new();
        for range in ranges {
            if let Some(last) = union.last_mut().filter(|last| range.start <= last.end) {
                last.end = last.end.max(range.end);
            } else {
                union.push(range);
            }
        }
        self.request_syntax_ranges(side, union, cx)
    }

    /// Published spans for the current input, including coverage from the
    /// previous viewport while a new query runs. Input mutations invalidate
    /// this result before notifying views.
    pub fn syntax_spans(&self, side: DocumentSide) -> &[TokenSpan] {
        self.syntax
            .side(side)
            .result
            .as_ref()
            .map_or(&[], |(_, spans)| spans)
    }

    pub(super) fn record_syntax_change(&mut self, change: &TextChange) {
        let changes = &mut self.syntax.changes;
        if change.edits.len() > EDIT_HISTORY_LIMIT {
            changes.clear();
            return;
        }
        let mut remaining = EDIT_HISTORY_LIMIT - change.edits.len();
        let keep = changes
            .iter()
            .rev()
            .take_while(|old| {
                if let Some(next) = remaining.checked_sub(old.edits.len()) {
                    remaining = next;
                    true
                } else {
                    false
                }
            })
            .count();
        changes.drain(..changes.len() - keep);
        changes.push(change.clone());
    }

    pub(super) fn invalidate_syntax(&mut self, side: DocumentSide) {
        let input = self.source_snapshot(side);
        let state = self.syntax.side_mut(side);
        if state
            .latest()
            .is_none_or(|request| !request.input.same_input(&input))
        {
            state.viewports.clear();
        }
        if state
            .pending
            .as_ref()
            .is_some_and(|request| !request.input.same_input(&input))
        {
            state.pending = None;
        }
        if state
            .result
            .as_ref()
            .is_some_and(|(request, _)| !request.input.same_input(&input))
        {
            state.result = None;
        }
        if let Some(worker) = &state.worker
            && !worker.request.input.same_input(&input)
        {
            worker.cancel.cancel();
        }
    }

    fn start_syntax(&mut self, side: DocumentSide, cx: &mut Context<Self>) {
        let state = self.syntax.side_mut(side);
        if state.worker.is_some() {
            return;
        }
        let Some(request) = state.pending.take() else {
            return;
        };
        let mut cache = std::mem::take(&mut state.cache);
        let changes = match side {
            DocumentSide::Base => Vec::new(),
            DocumentSide::New => self.syntax.changes.clone(),
        };
        let cancel = SyntaxCancellation::default();
        let work = request.clone();
        let work_cancel = cancel.clone();
        let task = cx.spawn(async move |this, cx| {
            let (cache, spans) = cx
                .background_executor()
                .spawn(async move {
                    let Request { input, ranges } = work;
                    if cache
                        .input
                        .as_ref()
                        .is_none_or(|old| !old.same_context(&input))
                    {
                        cache.syntax = SyntaxState::default();
                    }
                    let spans = cache
                        .syntax
                        .tokens_for_ranges(
                            &input.text,
                            &input.language,
                            &changes,
                            &ranges,
                            &work_cancel,
                        )
                        .expect("request range validated against its snapshot");
                    cache.input = Some(input);
                    (cache, spans)
                })
                .await;
            this.update(cx, |document, cx| {
                let current = document.source_snapshot(side);
                let state = document.syntax.side_mut(side);
                state.cache = cache;
                if let Some(worker) = state.worker.take()
                    && !worker.cancel.is_cancelled()
                    && worker.request.input.same_input(&current)
                {
                    state.result = Some((worker.request.clone(), spans));
                    cx.notify();
                }
                document.start_syntax(side, cx);
            })
            .ok();
        });
        self.syntax.side_mut(side).worker = Some(Worker {
            request,
            cancel,
            _task: task,
        });
    }
}
