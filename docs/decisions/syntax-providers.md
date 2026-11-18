# Syntax providers

The language registry selects syntax for each side by its own path and configuration.
Desktop builds install Tree-sitter for 110 bundled Arborium grammars. The pure Rust Java
provider is available on desktop and Wasm. The registry advertises only the providers
compiled for the target. All providers return theme-independent roles over half-open
UTF-8 byte ranges in the original text.

`Document::request_syntax` accepts one range per side. Each side keeps its own parse state,
one running job and at most one queued request. Requests that overtake running work cancel
it. The model publishes only results for the latest request whose input still passes
`DocumentSnapshot::same_input`. `Document::syntax_tokens` returns the result for that range.
A side with no source or available provider yields an empty list.

Parsing starts with the whole snapshot, so a comment, string or other construct that begins
outside the requested range still contributes tokens inside it. Tree-sitter reads Rope
chunks for parsing and query predicates. Highlight queries are compiled once per grammar.
Nested captures prefer the smallest original byte width, then the later query pattern.
The resolver clips the winners to the requested range and combines adjacent runs of the
same role.

Native Tree-sitter reuses the last complete tree only when the source, selection and edit
chain match. It applies each edit's byte offsets and LF points in transaction order. The
document keeps the latest 128 edit records at most. Missed edits, a changed source context,
or a changed virtual Markdown line ending require a fresh parse. An interrupted parse
preserves the previous completed tree.

Java flattens a snapshot once per changed input for Unicode escape translation and lexical
analysis. With an intact edit chain and identity byte mapping, it keeps the unchanged
lexical prefix with room for scanner lookahead. A change confined to an ordinary method
body of a top-level type can also reparse just that body and shift the cached roles after it.
Translated inputs and changes that cross a method boundary or require error recovery use
a full parse. The cached lexical ranges and identifier roles support binary-search viewport
lookups in raw byte coordinates, including for escaped identifiers.

The workers bound source size, parser and query progress, and output size. The desktop limits
are 8 MiB for Tree-sitter and 1 MiB for Java. The single-threaded Wasm build uses a 16 KiB
Java source limit and a smaller parser polling budget. Exceeding a
limit yields plain text for the requested range. Rendering, embedded-language queries,
locals and the Java project engine are later steps.
