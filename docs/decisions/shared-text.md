# Shared text snapshots and edits

Language support starts with one text contract in the pure `text` crate. `model::Document`
keeps ownership of the existing mutable Rope, edit grouping, undo/redo, saved revision,
conflicts and diff scheduling. There is no second editable document or editor adapter.
The crate has only a Ropey dependency and works on native and Wasm targets without GPUI.

## Identity and snapshots

A `SourceId` is an opaque, process-local allocation identity. Clones retain it; independently
opened documents get distinct identities even for the same path, contents and revision zero.
Each document has separate identities for its base/old and editable/new sources. A
`SourceStamp` pairs an identity with its monotonically increasing revision. Revision alone,
path alone and shared Rope storage are not valid result/cache keys. Stamps are not persistent
file IDs or a project VFS.

`Document::snapshot` cheaply clones the existing Rope. `base_snapshot` clones a separately
stamped immutable base Rope. Both retain their original contents across subsequent edits or
base refreshes. The existing flat base string remains for borrowed removed-line display;
base Rope construction happens on load/refresh, not on snapshot access. Missing text is empty,
as before; binary versions do not open text documents.

## Coordinates

All shared source ranges are half-open UTF-8 **byte** ranges. Both ends must be in bounds
and at scalar boundaries, with start no greater than end. `text::validate_range`,
`byte_to_char`, `byte_to_utf16`, `utf16_to_byte` and `byte_to_point` return errors for invalid
coordinates. UTF-16 surrogate interiors are rejected, not silently rounded. Scalars are
used only at the Rope boundary; they are not the public range unit. `Document::edit` treats
invalid byte ranges as programming errors and validates them in release builds too.

`BytePoint` is a zero-based LF line and byte column. A CR is an ordinary byte, so the position
between CR and LF is valid. No text API normalizes line endings, Unicode or file bytes.
The existing platform IME adapter retains its clamping/rounding policy for platform input;
strict shared APIs do not inherit that policy. Editor movement and GPUI shaping keep their
existing byte offsets. Line-relative UTF-16 protocol positions can be added at their actual
protocol boundary in a later milestone.

## Transactions and revisions

`DocumentEvent::Changed(TextChange)` publishes a nonempty list of `TextEdit` records and
`before`/`after` stamps. Each edit records old/new byte ranges and old/new start/end points.
Coordinates are sequential: an edit's old coordinates refer to the text immediately before
that edit, and its new coordinates to the text immediately after it. A parser can apply the
records in order and then parse the final snapshot.

Typing or replacement publishes one edit and advances the revision once. An undo group
applies inverse edits in reverse history order, publishes all their records together and
advances once. Redo applies the original edits in forward order, also advancing once. No
intermediate revision escapes. Undo never restores an old revision number. Existing cursor,
grouping and revision-based dirty/save behavior stays intact, including marking an undone
document dirty until saved even if its bytes match an earlier saved version.

A disk reload preserves a shared scalar-aligned prefix and suffix and replaces the intervening
bytes as its own undo step. This gives a precise changed range and first changed line without
resetting the whole file. Reload marks the resulting revision saved, clears conflict and can
be undone. An unchanged reload publishes no edit. Refresh of a clean document uses the same
path; dirty editable text is retained. Reload results overtaken by an edit or disk write/read
are discarded.

`first_changed_line()` is the earliest old start line across the ordered edits. Consumers
must match `before` before incremental use; a missed revision or different source requires a
fresh snapshot and rebuild. Events do not retain an unbounded change log or intermediate
text. Consumers that need text read the immutable snapshot for the resulting revision.

## Background publication and the base side

Base content changes advance only the base source revision, even when editable text is
unchanged or dirty. They schedule a new diff without generating an editable-text event or
changing dirty state. Identical base contents keep their stamp. Old base snapshots stay valid.

Each diff request carries both input stamps and a monotonically increasing request generation.
Publication requires all three to equal the current request. Generation distinguishes two
requests at the same text revisions. Replacing a GPUI task still cancels superseded work, and
its weak entity handle protects document lifetime; the explicit stamp check makes correctness
independent of cancellation timing. There are no placeholder language jobs or provider APIs.

## Alternatives and scope

Keeping snapshots in `model` would couple future pure parsers to GPUI. Importing SoopyCode's
mutable document or scalar range API would introduce another editing owner and competing
coordinate contracts. Instead its snapshot identity, conversion and versioned-job ideas inform
native byte-based values; attribution and its MIT notice are in `crates/text/NOTICE`.

Syntax, grammars, language registration, LSP, semantic sessions, historical project sources
and the Java project engine remain later milestones.
