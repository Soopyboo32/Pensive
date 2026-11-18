# Language registration and source context

The pure `language` crate owns stable canonical language IDs, one recognition catalog,
programmatic associations, installed-provider selection metadata and token roles. It depends
only on `text`, builds on native and Wasm, and does no I/O. `model::Document` remains the only
mutable document owner.

## Recognition and overrides

`Registry::default` builds the bundled catalog with `LanguageRegistration`, the same API used
to add a language or replace one later. The catalog recognizes common exact filenames and
extension aliases independently of installed providers.

Precedence, from highest to lowest:

1. Explicit whole-path association
2. Explicit filename association
3. Explicit final-extension association
4. Registered exact filename
5. Registered final extension
6. Plain text (`text`)

For example, an explicit `lock -> json` extension association beats the bundled exact
`Cargo.lock -> toml` mapping. A filename override can then beat that extension override, and
a path override can limit the choice to one file. Use `text` to disable recognition for an
association. The latest association for a normalized key wins; removing it exposes the next
matching rule. Unknown target languages and invalid keys fail without mutating the registry.

Path associations unify `/` and `\` but are case-sensitive, independent of the host OS.
They use the supplied path exactly: no canonicalization, dot-segment resolution or filesystem
access. Document paths are repository-relative. Exact filenames and final extensions use
ASCII case-insensitive comparison on every platform.
Non-ASCII characters are retained without Unicode case folding. Extensions may be supplied
with one leading dot; compound suffixes are not supported. `.rs` has no extension and falls
back to text, while `.config.toml` selects TOML. Known dotfiles such as `.bashrc`, `.zshrc`,
`.gitconfig` and `.gitattributes` are explicit filename entries. A trailing dot or separator
does not produce an extension. There is no shebang/content sniffing or glob matching yet.

Canonical IDs use lowercase ASCII letters, digits and hyphens. IDs and declared aliases are
looked up case-insensitively by `Registry::language` and association targets. Bundled extension
aliases also serve as language aliases (for example `rs -> rust`, `csharp -> c-sharp`).
Colliding names, filename mappings and extension mappings across registrations are rejected;
use an explicit association to override a mapping. Re-registering a canonical ID replaces its
whole entry, including old aliases and providers, atomically. Plain text always remains.

## Installed capabilities

Recognition does not promise a parser. All bundled registrations have **no providers** today,
so even recognized Rust/Java/TypeScript inputs return no syntax or semantic provider.
`Registry::empty` supplies only the text fallback, also without a provider.

Each language registration may carry `ProviderRegistration` metadata: provider ID, explicit
`Syntax`/`Semantic` capabilities and priority. The host owns the actual implementation and
must publish only capabilities it has installed for that language. This is a focused selection
contract, not a plugin loader, ABI, activation lifecycle or protocol for future features.
Syntax and semantic providers are selected independently: highest priority wins, then the
lexicographically smallest provider ID. Registration order does not affect selection. An
empty capability list offers neither feature; removing providers leaves recognition intact.
The later implementation milestones will add execution contracts where they are needed.

A `Selection` includes an opaque configuration identity as well as the chosen language and
providers. Registry clones retain this identity until changed; each successful mutation
allocates a new one. Independent registries and diverging clones cannot accidentally validate
each other's jobs through equal numeric generations. Conservative invalidation of all inputs
on catalog changes is adequate for this small catalog.

`TokenRole` retains the useful syntax/semantic distinctions, including declarations versus
calls, markup tags/attributes/values and captured variables, without colors or rendering.
`TokenSpan::new` validates a half-open UTF-8 byte range against the step 1 `TextSnapshot`.
There is no scalar-range compatibility shim. Producers must keep the input identity with
their results; spans alone do not identify a source.

## Both diff inputs

`FileVersions` now carries independent `FileSource` metadata for old/new inputs alongside
their existing `Version` values. Each source preserves its original repository-relative path
and `FileOrigin`: worktree or resolved commit ID. A missing side still keeps its requested
path/origin. A missing commit (unborn HEAD or root commit's parent) has no commit ID. The Git
backend resolves the commit before reading its bytes, avoiding a moving HEAD label on old
content. Fixtures use their synthetic commit IDs and run the same metadata contract checks.

`Projects::read_versions` still follows `FileChange::renamed_from`. `Contents` and `Document`
now retain the metadata instead of discarding it. `Document::source_snapshot(Base/New)` pairs
the corresponding existing text snapshot with its source, presence and current selection.
For `old.js -> new.ts`, the old side selects JavaScript and the new side TypeScript even when
both have identical bytes and revision zero. Added/deleted sides distinguish absence from an
empty text file. Commit documents retain their own and their parent's origins and stay read-only.

The step 1 `snapshot`/`base_snapshot` APIs and source stamps remain text contracts. A path,
presence, commit-context, association or provider change need not change the text revision.
Consumers use `DocumentSnapshot::same_input`, which compares both the text stamp and full
source/selection metadata, before reusing future language work. Original snapshots remain
immutable across edits, refreshes and registry replacement. No language jobs or caches are
implemented in this step.

Refresh accepts metadata and base changes while retaining dirty editable text. Clean documents
follow deletion/recreation too, retaining text source identity and distinguishing an absent side
from an empty file even when text stamps match. A read overtaken by a disk write, a newer request
or an accepted version refresh cannot restore stale metadata. Every accepted
version read advances the disk generation even if bytes are unchanged. `Projects::set_languages`
replaces the shared catalog for all open documents (including commits); an in-flight open uses
the latest catalog when published. Callers can build/validate a registry before installing it.
There is no settings UI or persisted association format yet.

## Alternatives and limits

Separate host and parser recognition tables would create competing recognition routes.
A single portable registration catalog with model-owned source context fits Pensive's
existing backend/fixture boundary and keeps document ownership in `model::Document`.

Syntax parsing, grammars, highlighting, semantic engines, LSP processes and historical project
VFS are later milestones. These source contexts identify the two existing diff inputs; they
do not provide a historical filesystem or project analysis session.
