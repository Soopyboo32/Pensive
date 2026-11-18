use std::ops::Range;

use gpui::{Context, Task};
use language::{SyntaxCancellation, SyntaxState, TokenSpan};
use text::{InvalidOffset, TextChange};

use super::{Document, DocumentSide, DocumentSnapshot};

const EDIT_HISTORY_LIMIT: usize = 128;

#[derive(Default)]
pub(super) struct DocumentSyntax {
    base: Side,
    new: Side,
    changes: Vec<TextChange>,
}

#[derive(Default)]
struct Side {
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
    range: Range<usize>,
}

impl Request {
    fn matches(&self, input: &DocumentSnapshot, range: &Range<usize>) -> bool {
        self.range == *range && self.input.same_input(input)
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
        let input = self.source_snapshot(side);
        text::validate_range(input.text.rope(), &range)?;
        let state = self.syntax.side_mut(side);
        if state
            .pending
            .as_ref()
            .is_some_and(|request| request.matches(&input, &range))
            || state.worker.as_ref().is_some_and(|worker| {
                !worker.cancel.is_cancelled() && worker.request.matches(&input, &range)
            })
            || state
                .result
                .as_ref()
                .is_some_and(|(request, _)| request.matches(&input, &range))
        {
            return Ok(());
        }
        state.result = None;
        if let Some(worker) = &state.worker {
            worker.cancel.cancel();
        }
        let plain = range.is_empty() || !input.present || input.language.syntax.is_none();
        let request = Request { input, range };
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
        let (request, spans) = self.syntax.side(side).result.as_ref()?;
        request
            .matches(&self.source_snapshot(side), &range)
            .then_some(spans.as_slice())
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
                    let Request { input, range } = work;
                    if cache
                        .input
                        .as_ref()
                        .is_none_or(|old| !old.same_context(&input))
                    {
                        cache.syntax = SyntaxState::default();
                    }
                    let spans = cache
                        .syntax
                        .tokens(&input.text, &input.language, &changes, range, &work_cancel)
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
