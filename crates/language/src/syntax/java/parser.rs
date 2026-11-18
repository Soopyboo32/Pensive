//! Hand-written, error-recovering Java editor parser. Every loop consumes input
//! or stops at a delimiter; balanced block/argument subtrees can be resumed
//! through heap jobs while remaining recursive constructs share a depth limit.
use super::SyntaxCancellation;
use super::{
    lexer::{JavaToken, JavaTokenKind},
    tree::JavaTree,
};
use std::{ops::Range, sync::Arc};

/// Keywords that start a primitive class literal or primitive array reference.
fn is_primitive_type_keyword(text: &str) -> bool {
    matches!(
        text,
        "byte" | "short" | "int" | "long" | "char" | "float" | "double" | "boolean" | "void"
    )
}

pub(crate) type ParseTask =
    std::pin::Pin<Box<dyn std::future::Future<Output = Option<JavaTree>> + Send>>;

pub(crate) fn parse(
    source: Arc<str>,
    tokens: Arc<[JavaToken]>,
    cancellation: SyntaxCancellation,
    body: Option<Range<usize>>,
) -> ParseTask {
    Box::pin(async move {
        let mut parser = Parser {
            source: &source,
            tokens: vec![],
            at: 0,
            tree: JavaTree::default(),
            // A top-level type owns one level of depth in a full parse.
            depth: usize::from(body.is_some()),
            stop: usize::MAX,
            delimiter_ends: vec![],
            deferred: vec![],
            guard_arrow: None,
            cancellation: &cancellation,
            work: 0,
        };
        let tokens = if let Some(bytes) = &body {
            let start = tokens.partition_point(|token| token.bytes.start < bytes.start);
            let end = tokens.partition_point(|token| token.bytes.end <= bytes.end);
            let inside = tokens.get(start..end)?;
            if inside
                .first()
                .is_none_or(|token| token.bytes.start != bytes.start || token.text(&source) != "{")
                || inside
                    .last()
                    .is_none_or(|token| token.bytes.end != bytes.end || token.text(&source) != "}")
            {
                return None;
            }
            inside
        } else {
            &tokens
        };
        for token in tokens {
            parser.tick().await;
            if cancellation.is_cancelled() {
                return None;
            }
            if !token.is_trivia() {
                parser.tokens.push(token);
            }
        }
        parser.stop = parser.tokens.len();
        if let Some(bytes) = body {
            let root = Box::pin(parser.block()).await;
            if parser.at != parser.stop
                || parser.tree.nodes[root].bytes != bytes
                || cancellation.is_cancelled()
            {
                return None;
            }
            parser.tree.root = root;
            return parser.finish_deferred().await.then_some(parser.tree);
        }
        let mut children = vec![];
        while !parser.eof() {
            let at = parser.at;
            children.push(Box::pin(parser.top_level()).await);
            if parser.at == at {
                children.push(parser.error().await);
            }
        }
        if cancellation.is_cancelled() {
            return None;
        }
        // Root members identify a compact unit. Earlier type declarations then
        // become members too, while imports remain outside the implicit owner.
        if children.iter().any(|&id| {
            matches!(
                parser.tree.nodes[id].kind.as_ref(),
                "method_declaration" | "field_declaration" | "initializer"
            )
        }) {
            let (mut outside, mut members): (Vec<_>, Vec<_>) =
                children.into_iter().partition(|&id| {
                    matches!(
                        parser.tree.nodes[id].kind.as_ref(),
                        "package_declaration" | "import_declaration" | "module_declaration"
                    )
                });
            let start = members
                .first()
                .map_or(0, |&id| parser.tree.nodes[id].bytes.start);
            let name = parser
                .tree
                .add("implicit_identifier", start..start, vec![], vec![]);
            let body = parser.tree.add(
                "class_body",
                start..source.len(),
                std::mem::take(&mut members),
                vec![],
            );
            let owner = parser.tree.add(
                "implicit_class_declaration",
                start..source.len(),
                vec![name, body],
                vec![("name", name), ("body", body)],
            );
            outside.push(owner);
            children = outside;
        }
        parser.tree.root = parser
            .tree
            .add("program", 0..source.len(), children, vec![]);
        if !parser.finish_deferred().await {
            return None;
        }
        Some(parser.tree)
    })
}

/// A parser continuation yields without a runtime or a worker thread. Hosts poll
/// it according to their frame budget; no partial tree can escape publication.
struct Yield(bool);
impl std::future::Future for Yield {
    type Output = ();
    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.0 {
            std::task::Poll::Ready(())
        } else {
            self.0 = true;
            std::task::Poll::Pending
        }
    }
}

#[derive(Clone, Copy)]
enum DeferredKind {
    Block,
    Arguments,
}
struct Deferred {
    start: usize,
    end: usize,
    hole: usize,
    kind: DeferredKind,
}

struct Parser<'a> {
    source: &'a str,
    tokens: Vec<&'a JavaToken>,
    at: usize,
    tree: JavaTree,
    depth: usize,
    stop: usize,
    delimiter_ends: Vec<Option<usize>>,
    deferred: Vec<Deferred>,
    guard_arrow: Option<usize>,
    cancellation: &'a SyntaxCancellation,
    work: usize,
}
impl Parser<'_> {
    /// Delimiter ownership is computed once, without recursive descent. A
    /// mismatched closer invalidates outer pairs, so recovery is never hidden
    /// behind a deferred region whose boundary has not been proved.
    async fn prepare_delimiters(&mut self) {
        self.delimiter_ends = vec![None; self.tokens.len()];
        let mut stack = vec![];
        for at in 0..self.tokens.len() {
            self.tick().await;
            if self.cancellation.is_cancelled() {
                return;
            }
            match self.tokens[at].text(self.source) {
                "(" => stack.push((")", at)),
                "[" => stack.push(("]", at)),
                "{" => stack.push(("}", at)),
                ")" | "]" | "}" => {
                    if let Some((expected, start)) = stack.pop() {
                        if expected == self.tokens[at].text(self.source) {
                            self.delimiter_ends[start] = Some(at + 1);
                        } else {
                            stack.clear();
                        }
                    }
                }
                _ => {}
            }
        }
    }
    /// A job owns one complete delimiter pair. Parsing resumes at depth zero;
    /// the same opener cannot defer itself again. Nested jobs therefore shrink
    /// strictly and their count is bounded by the original token inventory.
    async fn defer_balanced(&mut self, kind: DeferredKind) -> Option<usize> {
        if self.depth < 32 || self.cancellation.is_cancelled() {
            return None;
        }
        // Ordinary shallow files retain the existing parser path without a
        // delimiter inventory allocation or a second token walk.
        if self.delimiter_ends.is_empty() {
            self.prepare_delimiters().await;
        }
        if self.cancellation.is_cancelled() {
            return None;
        }
        let end = self.delimiter_ends.get(self.at).copied().flatten()?;
        if end > self.stop {
            return None;
        }
        let hole = self.tree.add(
            "deferred_subtree",
            self.pos()..self.tokens[end - 1].bytes.end,
            vec![],
            vec![],
        );
        self.deferred.push(Deferred {
            start: self.at,
            end,
            hole,
            kind,
        });
        self.at = end;
        Some(hole)
    }
    /// Temporary holes never leave this parser. Each job uses the original
    /// token vector and its exact bounded region. The final iterative postorder
    /// copy restores the arena invariant that every child precedes its parent.
    async fn finish_deferred(&mut self) -> bool {
        if self.deferred.is_empty() {
            return !self.cancellation.is_cancelled();
        }
        let saved = (self.at, self.stop, self.depth, self.guard_arrow);
        let mut replacements = std::collections::BTreeMap::new();
        let mut next = 0;
        while next < self.deferred.len() {
            self.tick().await;
            if self.cancellation.is_cancelled() {
                return false;
            }
            let job = &self.deferred[next];
            let (start, end, hole, kind) = (job.start, job.end, job.hole, job.kind);
            self.at = start;
            self.stop = end;
            self.depth = 0;
            self.guard_arrow = None;
            let mut node = match kind {
                DeferredKind::Block => Box::pin(self.block()).await,
                DeferredKind::Arguments => Box::pin(self.arguments()).await,
            };
            // Invalid contents may stop argument recovery before its closing
            // delimiter. Keep all remaining owned tokens as errors, rather
            // than publishing an incomplete placeholder as a complete parse.
            if !self.eof() {
                let mut children = vec![node];
                while !self.eof() {
                    children.push(self.error().await);
                }
                node = self.node("ERROR", self.tokens[start].bytes.start, children, vec![]);
            }
            replacements.insert(hole, node);
            next += 1;
        }
        (self.at, self.stop, self.depth, self.guard_arrow) = saved;
        let mut tree = JavaTree::default();
        let mut remapped = vec![None; self.tree.nodes.len()];
        let mut pending = vec![(self.tree.root, false)];
        while let Some((id, visited)) = pending.pop() {
            self.tick().await;
            if self.cancellation.is_cancelled() {
                return false;
            }
            let id = replacements.get(&id).copied().unwrap_or(id);
            if remapped[id].is_some() {
                continue;
            }
            let element = &self.tree.nodes[id];
            if !visited {
                pending.push((id, true));
                pending.extend(element.children.iter().rev().map(|&child| (child, false)));
                continue;
            }
            let resolve = |child: usize| {
                remapped[replacements.get(&child).copied().unwrap_or(child)]
                    .expect("completed deferred child")
            };
            remapped[id] = Some(
                tree.add_owned(
                    element.kind.clone(),
                    element.bytes.clone(),
                    element
                        .children
                        .iter()
                        .map(|&child| resolve(child))
                        .collect(),
                    element
                        .fields
                        .iter()
                        .map(|(name, child)| (name.clone(), resolve(*child)))
                        .collect(),
                ),
            );
        }
        tree.root = remapped[self.tree.root].expect("completed deferred root");
        self.tree = tree;
        true
    }
    fn token(&self, at: usize) -> Option<&&JavaToken> {
        (at < self.stop).then(|| self.tokens.get(at)).flatten()
    }

    async fn tick(&mut self) {
        self.work += 1;
        if self.work == 4096 {
            self.work = 0;
            Yield(false).await;
        }
    }
    fn eof(&self) -> bool {
        self.at >= self.stop || self.cancellation.is_cancelled()
    }
    fn text(&self) -> &str {
        self.nth(0)
    }
    fn nth(&self, n: usize) -> &str {
        self.text_at(self.at + n)
    }
    fn text_at(&self, at: usize) -> &str {
        self.token(at).map_or("", |t| t.text(self.source))
    }
    /// `non-sealed` is one modifier only when its three tokens touch (JLS 3.9).
    fn non_sealed_at(&self, at: usize) -> bool {
        let touching = |first: usize| {
            self.token(first)
                .zip(self.token(first + 1))
                .is_some_and(|(left, right)| left.bytes.end == right.bytes.start)
        };
        self.text_at(at) == "non"
            && self.text_at(at + 1) == "-"
            && self.text_at(at + 2) == "sealed"
            && touching(at)
            && touching(at + 1)
    }
    /// JavacParser.allowedAfterSealedOrNonSealed: otherwise `sealed` is a type name.
    fn declaration_follows_sealed(&self, next: usize, after_non_sealed: bool) -> bool {
        match self.text_at(next) {
            "public" | "protected" | "private" | "abstract" | "static" | "final" | "strictfp"
            | "class" | "interface" | "enum" | "sealed" => true,
            "@" => after_non_sealed || self.text_at(next + 1) != "interface",
            // javac reads `sealed record` as a type name; the record rules report the modifier.
            "record" => self
                .token(next + 1)
                .is_some_and(|token| token.kind == JavaTokenKind::Identifier),
            _ => self.non_sealed_at(next),
        }
    }
    fn pos(&self) -> usize {
        self.token(self.at).map_or_else(
            || {
                if self.stop == self.tokens.len() {
                    self.source.len()
                } else {
                    self.stop
                        .checked_sub(1)
                        .and_then(|at| self.tokens.get(at))
                        .map_or(self.source.len(), |token| token.bytes.end)
                }
            },
            |token| token.bytes.start,
        )
    }
    fn end(&self) -> usize {
        self.at
            .checked_sub(1)
            .and_then(|i| self.tokens.get(i))
            .map_or(0, |t| t.bytes.end)
    }
    /// `_` reads as an identifier like javac's `ident()`; the analyzer reports it outside a binding.
    fn ident_at(&self, at: usize) -> bool {
        self.token(at)
            .is_some_and(|t| t.kind == JavaTokenKind::Identifier || t.text(self.source) == "_")
    }
    fn ident(&self) -> bool {
        self.ident_at(self.at)
    }
    async fn leaf(&mut self, kind: &'static str) -> usize {
        self.tick().await;
        let range = self
            .token(self.at)
            .map_or(self.pos()..self.pos(), |t| t.bytes.clone());
        if !self.eof() {
            self.at += 1;
        }
        self.tree.add(kind, range, vec![], vec![])
    }
    async fn bump(&mut self) -> usize {
        let kind = match self.token(self.at).map(|t| t.kind) {
            Some(JavaTokenKind::Identifier) => "identifier",
            Some(JavaTokenKind::Keyword) if self.text() == "_" => "identifier",
            Some(JavaTokenKind::Number) => "number_literal",
            Some(JavaTokenKind::String) => "string_literal",
            Some(JavaTokenKind::Character) => "character_literal",
            Some(JavaTokenKind::Error) => "ERROR",
            _ => match self.text() {
                "this" => "this",
                "super" => "super",
                "static" => "static",
                "public" => "public",
                "private" => "private",
                "protected" => "protected",
                "*" => "asterisk",
                _ => "token",
            },
        };
        self.leaf(kind).await
    }
    async fn eat(&mut self, text: &str) -> bool {
        self.tick().await;
        if self.text() == text && !self.eof() {
            self.at += 1;
            true
        } else {
            false
        }
    }
    async fn error(&mut self) -> usize {
        self.leaf(if self.eof() { "MISSING" } else { "ERROR" })
            .await
    }
    fn missing_at_token(&mut self) -> usize {
        self.tree
            .add("MISSING_AT_TOKEN", self.pos()..self.pos(), vec![], vec![])
    }
    /// Like `expect`, for the places javac reports the absence at the offending token instead of
    /// after the previous one: type arguments `>` and the `(` or `[` after `new Type`.
    async fn expect_at_token(&mut self, text: &str, children: &mut Vec<usize>) {
        if !self.eat(text).await {
            children.push(self.missing_at_token());
        }
    }
    async fn expect(&mut self, text: &str, children: &mut Vec<usize>) {
        if !self.eat(text).await {
            children.push(
                self.tree
                    .add("MISSING", self.pos()..self.pos(), vec![], vec![]),
            );
        }
    }
    fn node(
        &mut self,
        kind: &'static str,
        start: usize,
        children: Vec<usize>,
        fields: Vec<(&'static str, usize)>,
    ) -> usize {
        self.tree
            .add(kind, start..self.end().max(start), children, fields)
    }
    async fn name(&mut self) -> usize {
        if self.ident() {
            self.bump().await
        } else {
            self.tree
                .add("MISSING", self.pos()..self.pos(), vec![], vec![])
        }
    }
    async fn binding_name(&mut self) -> usize {
        if self.text() == "_" {
            self.leaf("unnamed_identifier").await
        } else {
            self.name().await
        }
    }
    async fn top_level(&mut self) -> usize {
        let checkpoint = (self.at, self.tree.nodes.len(), self.deferred.len());
        let start = self.pos();
        let mods = Box::pin(self.modifiers()).await;
        if matches!(self.text(), "package" | "import") {
            let kind = if self.text() == "package" {
                "package_declaration"
            } else {
                "import_declaration"
            };
            let mut children = mods;
            if kind == "package_declaration" {
                // Package modifiers are annotations only. Inspect the consumed
                // prefix outside complete annotation spans, including keyword
                // sequences such as non-sealed that have no modifier node.
                let annotations = children
                    .iter()
                    .filter(|&&id| self.tree.nodes[id].kind.as_ref() == "annotation")
                    .map(|&id| self.tree.nodes[id].bytes.clone())
                    .collect::<Vec<_>>();
                let mut annotation = 0;
                for at in checkpoint.0..self.at {
                    self.tick().await;
                    let bytes = self.tokens[at].bytes.clone();
                    while annotations
                        .get(annotation)
                        .is_some_and(|range| range.end <= bytes.start)
                    {
                        annotation += 1;
                    }
                    if !annotations
                        .get(annotation)
                        .is_some_and(|range| range.start <= bytes.start && bytes.end <= range.end)
                    {
                        children.push(self.tree.add("ERROR", bytes, vec![], vec![]));
                    }
                }
            }
            self.at += 1;
            if self.text() == "static" {
                children.push(self.bump().await);
            }
            let s = self.pos();
            let mut parts = vec![];
            while self.ident() {
                parts.push(self.bump().await);
                if !self.eat(".").await {
                    break;
                }
            }
            children.push(self.node("scoped_identifier", s, parts, vec![]));
            if self.text() == "*" {
                children.push(self.bump().await);
            }
            self.expect(";", &mut children).await;
            return self.node(kind, start, children, vec![]);
        }
        if self.text() == "module" || self.text() == "open" && self.nth(1) == "module" {
            let mut children = mods;
            while !self.eof() && !matches!(self.text(), "{" | ";") {
                children.push(self.bump().await);
            }
            if self.eat("{").await {
                while !self.eof() && self.text() != "}" {
                    children.push(self.bump().await);
                }
                self.expect("}", &mut children).await;
            } else {
                self.expect(";", &mut children).await;
            }
            return self.node("module_declaration", start, children, vec![]);
        }
        if self.is_type_decl() {
            return Box::pin(self.type_decl(start, mods)).await;
        }
        if self.eat(";").await {
            return self.node("empty_declaration", start, mods, vec![]);
        }
        self.at = checkpoint.0;
        self.tree.nodes.truncate(checkpoint.1);
        self.deferred.truncate(checkpoint.2);
        Box::pin(self.member("", false)).await
    }
    async fn modifiers(&mut self) -> Vec<usize> {
        let mut nodes = vec![];
        loop {
            if self.text() == "@" && self.nth(1) != "interface" {
                nodes.push(Box::pin(self.annotation()).await);
            } else if matches!(
                self.text(),
                "public"
                    | "private"
                    | "protected"
                    | "static"
                    | "final"
                    | "abstract"
                    | "native"
                    | "synchronized"
                    | "transient"
                    | "volatile"
                    | "strictfp"
                    | "default"
            ) {
                nodes.push(self.bump().await);
            } else if self.text() == "sealed" && self.declaration_follows_sealed(self.at + 1, false)
            {
                // Its own kind keeps a modifier apart from a name or type spelled sealed.
                nodes.push(self.leaf("sealed").await);
            } else if self.non_sealed_at(self.at)
                && self.declaration_follows_sealed(self.at + 3, true)
            {
                self.at += 3;
            } else {
                break;
            }
        }
        nodes
    }
    async fn annotation(&mut self) -> usize {
        let start = self.pos();
        self.eat("@").await;
        let mut children = vec![self.name().await];
        while self.eat(".").await {
            children.push(self.name().await);
        }
        if self.eat("(").await {
            // An annotation has either named element-value pairs or one
            // unnamed value. The optional trailing comma belongs only to
            // an array initializer inside a value, never this outer list.
            let named = self.ident() && self.nth(1) == "=";
            while !self.eof() && self.text() != ")" {
                let pair = self.ident() && self.nth(1) == "=";
                let value = if pair {
                    let start = self.pos();
                    let name = self.leaf("element_name").await;
                    self.eat("=").await;
                    let value = Box::pin(self.expr(0)).await;
                    self.node(
                        "element_value_pair",
                        start,
                        vec![name, value],
                        vec![("value", value)],
                    )
                } else {
                    Box::pin(self.expr(0)).await
                };
                if pair != named {
                    children.push(self.tree.add(
                        "ERROR",
                        self.tree.nodes[value].bytes.clone(),
                        vec![],
                        vec![],
                    ));
                }
                children.push(value);
                let comma = self.pos();
                if !self.eat(",").await {
                    break;
                }
                if !named || self.text() == ")" {
                    children.push(self.tree.add("ERROR", comma..self.end(), vec![], vec![]));
                }
            }
            self.expect(")", &mut children).await;
        }
        self.node("annotation", start, children, vec![])
    }
    fn is_type_decl(&self) -> bool {
        matches!(self.text(), "class" | "interface" | "enum")
            || self.text() == "record"
                && self
                    .token(self.at + 1)
                    .is_some_and(|token| token.kind == JavaTokenKind::Identifier)
            || self.text() == "@" && self.nth(1) == "interface"
    }
    async fn type_decl(&mut self, start: usize, mut children: Vec<usize>) -> usize {
        if self.depth >= 48 {
            return self.error().await;
        }
        self.depth += 1;
        let kind = match self.text() {
            "class" => "class_declaration",
            "interface" => "interface_declaration",
            "enum" => "enum_declaration",
            "record" => "record_declaration",
            _ => {
                self.at += 1;
                "annotation_type_declaration"
            }
        };
        self.at += 1;
        let name = self.name().await;
        let type_name = self.source[self.tree.nodes[name].bytes.clone()].to_string();
        children.push(name);
        if self.text() == "<" {
            children.push(Box::pin(self.type_parameters()).await);
        }
        if kind == "record_declaration" {
            if self.text() == "(" {
                children.push(Box::pin(self.parameters(false)).await);
            } else {
                self.expect("(", &mut children).await;
            }
        }
        let mut seen_permits = false;
        while !self.eof() && !matches!(self.text(), "{" | ";" | "}") {
            let type_expected =
                self.at > 0 && matches!(self.text_at(self.at - 1), "extends" | "implements" | ",");
            if matches!(self.text(), "extends" | "implements" | ",")
                || self.text() == "permits" && !type_expected
            {
                // Only clause keywords owned by this header participate:
                // nested type names and annotations are consumed by ty().
                let repeated_permits = self.text() == "permits" && seen_permits;
                seen_permits |= self.text() == "permits";
                // Consume illegal keywords for recovery without losing the
                // remaining header types or inventing body members.
                let keyword = if repeated_permits
                    || kind == "annotation_type_declaration" && self.text() != ","
                {
                    self.leaf("ERROR").await
                } else {
                    self.bump().await
                };
                children.push(keyword);
            } else if let Some(ty) = Box::pin(self.ty()).await {
                children.push(ty);
            } else {
                children.push(self.error().await);
            }
        }
        let body_start = self.pos();
        let mut members = vec![];
        self.expect("{", &mut members).await;
        if kind == "enum_declaration" {
            // The optional trailing comma is legal even when the list is empty.
            if self.text() == "," && matches!(self.nth(1), ";" | "}") {
                self.at += 1;
            }
            while self.ident() || self.text() == "@" {
                let s = self.pos();
                let mut c = Box::pin(self.modifiers()).await;
                let n = self.name().await;
                c.push(n);
                if self.text() == "(" {
                    c.push(Box::pin(self.arguments()).await);
                }
                if self.text() == "{" {
                    c.push(Box::pin(self.anonymous_body()).await);
                }
                members.push(self.node("enum_constant", s, c, vec![("name", n)]));
                if !self.eat(",").await {
                    break;
                }
            }
            self.eat(";").await;
        }
        while !self.eof() && self.text() != "}" {
            let before = self.at;
            members.push(
                Box::pin(self.member(&type_name, kind == "annotation_type_declaration")).await,
            );
            if self.at == before {
                members.push(self.error().await);
            }
        }
        self.expect("}", &mut members).await;
        let body = self.node("class_body", body_start, members, vec![]);
        children.push(body);
        self.depth -= 1;
        self.node(kind, start, children, vec![("name", name), ("body", body)])
    }
    async fn member(&mut self, type_name: &str, annotation_owner: bool) -> usize {
        let start = self.pos();
        let mut children = Box::pin(self.modifiers()).await;
        if self.is_type_decl() {
            return Box::pin(self.type_decl(start, children)).await;
        }
        if self.text() == "{" {
            let body = Box::pin(self.block()).await;
            children.push(body);
            return self.node("initializer", start, children, vec![]);
        }
        if self.eat(";").await {
            return self.node("empty_declaration", start, children, vec![]);
        }
        let generic = self.text() == "<";
        if generic {
            children.push(Box::pin(self.type_parameters()).await);
        }
        let constructor = self.text() == type_name && matches!(self.nth(1), "(" | "{");
        let mut void_result = false;
        if !constructor {
            if let Some(ty) = Box::pin(self.ty()).await {
                void_result = &self.source[self.tree.nodes[ty].bytes.clone()] == "void";
                children.push(ty);
            } else {
                return self.error().await;
            }
        }
        let name = self.name().await;
        children.push(name);
        if self.text() == "(" || constructor && self.text() == "{" {
            let compact = self.text() == "{";
            if !compact {
                children.push(Box::pin(self.parameters(true)).await);
            }
            let result_dimensions = children.len();
            Box::pin(self.empty_dimensions(&mut children)).await;
            if constructor && children.len() != result_dimensions {
                children.push(self.tree.add(
                    "ERROR",
                    self.tree.nodes[children[result_dimensions]].bytes.clone(),
                    vec![],
                    vec![],
                ));
            }
            if self.eat("throws").await {
                while let Some(ty) = Box::pin(self.ty()).await {
                    children.push(ty);
                    if self.tree.nodes[ty].kind.as_ref() == "array_type" {
                        children.push(self.tree.add(
                            "ERROR",
                            self.tree.nodes[ty].bytes.clone(),
                            vec![],
                            vec![],
                        ));
                    }
                    if !self.eat(",").await {
                        break;
                    }
                }
            }
            if self.text() == "default" {
                // An element default is permitted only in this annotation
                // declaration's own body, not nested ordinary or anonymous
                // types. Interface default methods use a leading modifier.
                if annotation_owner && !constructor {
                    self.eat("default").await;
                } else {
                    children.push(self.leaf("ERROR").await);
                }
                children.push(Box::pin(self.expr(0)).await);
            }
            let mut fields = vec![("name", name)];
            if self.text() == "{" {
                let body = Box::pin(self.block()).await;
                children.push(body);
                fields.push(("body", body));
            } else {
                self.expect(";", &mut children).await;
            }
            return self.node(
                if compact {
                    "compact_constructor_declaration"
                } else if constructor {
                    "constructor_declaration"
                } else {
                    "method_declaration"
                },
                start,
                children,
                fields,
            );
        }
        // JLS 8.4.4: only a method declares type parameters, so javac wants `(`
        // at the token after the name. A void field is left to the semantic
        // check, which reports the type instead of the missing parenthesis.
        let void_or_generic = generic || void_result;
        if generic {
            children.push(self.missing_at_token());
        }
        let variable = Box::pin(self.variable(name)).await;
        children.push(variable);
        while self.eat(",").await {
            let name = self.name().await;
            let variable = Box::pin(self.variable(name)).await;
            children.push(variable);
        }
        if void_or_generic {
            if !self.eat(";").await && !generic {
                // javac wants `(` here and reports it at the offending token (JavacParser.classOrInterfaceBodyDeclaration).
                children.push(self.missing_at_token());
            }
        } else {
            self.expect(";", &mut children).await;
        }
        self.node("field_declaration", start, children, vec![])
    }
    async fn anonymous_body(&mut self) -> usize {
        if self.depth >= 48 {
            return self.error().await;
        }
        self.depth += 1;
        let start = self.pos();
        let name = self
            .tree
            .add("anonymous_identifier", start..start, vec![], vec![]);
        let mut members = vec![];
        self.expect("{", &mut members).await;
        while !self.eof() && self.text() != "}" {
            let before = self.at;
            members.push(Box::pin(self.member("", false)).await);
            if self.at == before {
                members.push(self.error().await);
            }
        }
        self.expect("}", &mut members).await;
        let body = self.node("class_body", start, members, vec![]);
        self.depth -= 1;
        self.node(
            "anonymous_class_declaration",
            start,
            vec![name, body],
            vec![("name", name), ("body", body)],
        )
    }
    async fn type_parameters(&mut self) -> usize {
        let start = self.pos();
        self.at += 1;
        let mut children = vec![];
        while !self.eof() && !matches!(self.text(), ">" | "{" | "(") {
            let s = self.pos();
            let mut c = vec![];
            while self.text() == "@" {
                c.push(Box::pin(self.annotation()).await);
            }
            let n = self.leaf("type_identifier").await;
            c.push(n);
            if self.eat("extends").await {
                loop {
                    if let Some(ty) = Box::pin(self.ty()).await {
                        c.push(ty);
                    } else {
                        c.push(self.error().await);
                    }
                    if !self.eat("&").await {
                        break;
                    }
                }
            }
            children.push(self.node("type_parameter", s, c, vec![("name", n)]));
            if !self.eat(",").await {
                break;
            }
        }
        self.expect(">", &mut children).await;
        self.node("type_parameters", start, children, vec![])
    }
    async fn ty(&mut self) -> Option<usize> {
        self.tick().await;
        if self.depth >= 48 {
            return None;
        }
        let start = self.pos();
        let mut children = vec![];
        while self.text() == "@" {
            children.push(Box::pin(self.annotation()).await);
        }
        if self.ident()
            || matches!(
                self.text(),
                "byte"
                    | "short"
                    | "int"
                    | "long"
                    | "char"
                    | "float"
                    | "double"
                    | "boolean"
                    | "void"
            )
        {
            children.push(self.leaf("type_identifier").await);
        } else if self.eat("?").await {
            self.depth += 1;
            if matches!(self.text(), "extends" | "super") {
                self.at += 1;
                if let Some(ty) = Box::pin(self.ty()).await {
                    children.push(ty);
                }
            }
            self.depth -= 1;
            return Some(self.node("wildcard", start, children, vec![]));
        } else {
            return None;
        }
        self.depth += 1;
        loop {
            if self.eat("<").await {
                let s = self.end();
                let mut args = vec![];
                while !self.eof() && !matches!(self.text(), ">" | "{" | ";" | "(") {
                    if let Some(ty) = Box::pin(self.ty()).await {
                        args.push(ty);
                    } else {
                        args.push(self.error().await);
                    }
                    if !self.eat(",").await {
                        break;
                    }
                }
                self.expect_at_token(">", &mut args).await;
                children.push(self.node("type_arguments", s, args, vec![]));
            }
            if self.text() == "." && (self.ident_at(self.at + 1) || self.nth(1) == "@") {
                self.at += 1;
                while self.text() == "@" {
                    children.push(Box::pin(self.annotation()).await);
                }
                if !self.ident() {
                    break;
                }
                children.push(self.leaf("type_identifier").await);
            } else {
                break;
            }
        }
        let mut array = false;
        loop {
            let suffix_annotations = children.len();
            while self.text() == "@" {
                children.push(Box::pin(self.annotation()).await);
            }
            if self.text() != "[" || self.nth(1) != "]" {
                // A suffix annotation requires following array brackets,
                // possibly sized by an array creation, or a varargs ellipsis.
                // Do not silently turn an incomplete annotated dimension into
                // a legal type followed by the declarator name.
                if children.len() > suffix_annotations && !matches!(self.text(), "[" | "...") {
                    let start = self.tree.nodes[children[suffix_annotations]].bytes.start;
                    children.push(self.tree.add("ERROR", start..self.end(), vec![], vec![]));
                }
                break;
            }
            self.tick().await;
            array = true;
            self.at += 2;
        }
        self.depth -= 1;
        Some(if array {
            self.node("array_type", start, children, vec![])
        } else if children.len() == 1 {
            children[0]
        } else {
            self.node("type", start, children, vec![])
        })
    }
    /// `this` or `Name(.Name)*.this`: a receiver parameter's name.
    fn names_this_receiver(&self) -> bool {
        let mut at = self.at;
        while self.ident_at(at) && self.text_at(at + 1) == "." {
            at += 2;
        }
        self.text_at(at) == "this"
    }
    async fn parameters(&mut self, receiver_allowed: bool) -> usize {
        let start = self.pos();
        self.at += 1;
        let mut children = vec![];
        while !self.eof() && !matches!(self.text(), ")" | "{" | "}") {
            let s = self.pos();
            let mut c = Box::pin(self.modifiers()).await;
            if let Some(ty) = Box::pin(self.ty()).await {
                c.push(ty);
            } else {
                c.push(self.error().await);
            }
            let spread = self.eat("...").await;
            if self.names_this_receiver() {
                let mut fields = vec![];
                if self.text() != "this" {
                    let mut parts = vec![];
                    while self.text() != "this" {
                        parts.push(self.leaf("type_identifier").await);
                        self.at += 1; // '.'
                    }
                    // javac reads any dotted name before `.this` and checks it afterwards.
                    let qualifier = match parts[..] {
                        [only] => only,
                        _ => {
                            let first = self.tree.nodes[parts[0]].bytes.start;
                            let last = self.tree.nodes[parts[parts.len() - 1]].bytes.end;
                            self.tree
                                .add("scoped_identifier", first..last, parts, vec![])
                        }
                    };
                    c.push(qualifier);
                    fields.push(("qualifier", qualifier));
                }
                let receiver = self.bump().await;
                c.push(receiver);
                fields.push(("name", receiver));
                if !receiver_allowed
                    || !children.is_empty()
                    || spread
                    || c.iter().any(|id| {
                        !matches!(
                            self.tree.nodes[*id].kind.as_ref(),
                            "annotation"
                                | "type"
                                | "array_type"
                                | "type_identifier"
                                | "scoped_identifier"
                                | "this"
                        )
                    })
                {
                    c.push(self.tree.add(
                        "ERROR",
                        self.tree.nodes[receiver].bytes.clone(),
                        vec![],
                        vec![],
                    ));
                }
                children.push(self.node("receiver_parameter", s, c, fields));
                if !self.eat(",").await {
                    break;
                }
                continue;
            }
            let n = self.name().await;
            c.push(n);
            Box::pin(self.empty_dimensions(&mut c)).await;
            children.push(self.node(
                if spread {
                    "spread_parameter"
                } else {
                    "formal_parameter"
                },
                s,
                c,
                vec![("name", n)],
            ));
            if !self.eat(",").await {
                break;
            }
        }
        self.expect(")", &mut children).await;
        self.node("formal_parameters", start, children, vec![])
    }
    async fn type_arguments(&mut self) -> usize {
        let start = self.pos();
        let mut args = vec![];
        self.expect("<", &mut args).await;
        while !self.eof() && !matches!(self.text(), ">" | ";" | "{" | "(") {
            if let Some(ty) = Box::pin(self.ty()).await {
                args.push(ty);
            } else {
                args.push(self.error().await);
            }
            if !self.eat(",").await {
                break;
            }
        }
        self.expect_at_token(">", &mut args).await;
        self.node("type_arguments", start, args, vec![])
    }
    async fn variable(&mut self, name: usize) -> usize {
        let start = self.tree.nodes[name].bytes.start;
        let mut children = vec![name];
        let mut fields = vec![("name", name)];
        Box::pin(self.empty_dimensions(&mut children)).await;
        if self.eat("=").await {
            let value = Box::pin(self.expr(0)).await;
            children.push(value);
            fields.push(("value", value));
        }
        self.node("variable_declarator", start, children, fields)
    }
    // Dims can follow a declarator name or a method parameter list. Keep each
    // annotated bracket pair separate from the declared element type.
    async fn empty_dimensions(&mut self, children: &mut Vec<usize>) {
        let unnamed = children
            .last()
            .is_some_and(|id| self.tree.nodes[*id].kind.as_ref() == "unnamed_identifier");
        while matches!(self.text(), "[" | "@") {
            let checkpoint = (self.at, self.tree.nodes.len(), self.deferred.len());
            let start = self.pos();
            let mut dimension = vec![];
            while self.text() == "@" {
                dimension.push(Box::pin(self.annotation()).await);
            }
            if !self.eat("[").await {
                self.at = checkpoint.0;
                self.tree.nodes.truncate(checkpoint.1);
                self.deferred.truncate(checkpoint.2);
                break;
            }
            self.expect("]", &mut dimension).await;
            if unnamed {
                dimension.push(self.tree.add("ERROR", start..self.end(), vec![], vec![]));
            }
            children.push(self.node("dimensions", start, dimension, vec![]));
        }
    }
    async fn local(&mut self) -> Option<usize> {
        let checkpoint = (self.at, self.tree.nodes.len(), self.deferred.len());
        let start = self.pos();
        let mut children = Box::pin(self.modifiers()).await;
        if let Some(ty) = Box::pin(self.ty()).await {
            children.push(ty);
        } else {
            self.at = checkpoint.0;
            self.tree.nodes.truncate(checkpoint.1);
            self.deferred.truncate(checkpoint.2);
            return None;
        }
        if !(self.ident() || self.text() == "_")
            || !matches!(self.nth(1), "=" | "," | ";" | ":" | "[" | "@" | ")")
        {
            self.at = checkpoint.0;
            self.tree.nodes.truncate(checkpoint.1);
            self.deferred.truncate(checkpoint.2);
            return None;
        }
        loop {
            let name = self.binding_name().await;
            children.push(Box::pin(self.variable(name)).await);
            if !self.eat(",").await {
                break;
            }
        }
        Some(self.node("local_variable_declaration", start, children, vec![]))
    }
    async fn block(&mut self) -> usize {
        if let Some(hole) = self.defer_balanced(DeferredKind::Block).await {
            return hole;
        }
        let start = self.pos();
        if self.depth >= 48 {
            return self.error().await;
        }
        self.depth += 1;
        self.at += 1;
        let mut children = vec![];
        while !self.eof() && self.text() != "}" {
            let before = self.at;
            children.push(Box::pin(self.statement()).await);
            if self.at == before {
                children.push(self.error().await);
            }
        }
        self.expect("}", &mut children).await;
        self.depth -= 1;
        self.node("block", start, children, vec![])
    }
    async fn statement(&mut self) -> usize {
        self.tick().await;
        if self.depth >= 48 {
            return self.error().await;
        }
        self.depth += 1;
        let node = Box::pin(self.statement_inner()).await;
        self.depth -= 1;
        node
    }
    async fn statement_inner(&mut self) -> usize {
        let start = self.pos();
        // A switch immediately after an arrow is the rule's expression result,
        // whose trailing semicolon belongs to the rule rather than its block.
        if self.text() == "switch"
            && self
                .at
                .checked_sub(1)
                .is_some_and(|previous| self.tokens[previous].text(self.source) == "->")
        {
            let value = Box::pin(self.expr(0)).await;
            let mut children = vec![value];
            self.expect(";", &mut children).await;
            return self.node("expression_statement", start, children, vec![]);
        }
        if self.text() == "{" {
            return Box::pin(self.block()).await;
        }
        if self.is_type_decl() {
            return Box::pin(self.type_decl(start, vec![])).await;
        }
        if matches!(
            self.text(),
            "@" | "final"
                | "abstract"
                | "strictfp"
                | "sealed"
                | "non"
                | "static"
                | "public"
                | "private"
                | "protected"
        ) {
            let checkpoint = (self.at, self.tree.nodes.len(), self.deferred.len());
            let modifiers = Box::pin(self.modifiers()).await;
            if self.is_type_decl() {
                return Box::pin(self.type_decl(start, modifiers)).await;
            }
            self.at = checkpoint.0;
            self.tree.nodes.truncate(checkpoint.1);
            self.deferred.truncate(checkpoint.2);
        }
        if self.eat(";").await {
            return self.node("empty_statement", start, vec![], vec![]);
        }
        if self.eat("for").await {
            return Box::pin(self.for_statement(start)).await;
        }
        if self.eat("try").await {
            return Box::pin(self.try_statement(start)).await;
        }
        if matches!(self.text(), "if" | "while" | "switch" | "synchronized") {
            let kind = match self.text() {
                "if" => "if_statement",
                "while" => "while_statement",
                "switch" => "switch_statement",
                _ => "synchronized_statement",
            };
            self.at += 1;
            let mut children = vec![];
            self.expect("(", &mut children).await;
            let condition = Box::pin(self.expr(0)).await;
            children.push(condition);
            self.expect(")", &mut children).await;
            let consequence = Box::pin(self.statement()).await;
            children.push(consequence);
            let mut fields = vec![("condition", condition), ("consequence", consequence)];
            if self.eat("else").await {
                let alternative = Box::pin(self.statement()).await;
                children.push(alternative);
                fields.push(("alternative", alternative));
            }
            return self.node(kind, start, children, fields);
        }
        if self.eat("do").await {
            let body = Box::pin(self.statement()).await;
            let mut children = vec![body];
            self.expect("while", &mut children).await;
            self.expect("(", &mut children).await;
            let condition = Box::pin(self.expr(0)).await;
            children.push(condition);
            self.expect(")", &mut children).await;
            self.expect(";", &mut children).await;
            return self.node(
                "do_statement",
                start,
                children,
                vec![("body", body), ("condition", condition)],
            );
        }
        if matches!(self.text(), "case" | "default") {
            let checkpoint = (self.at, self.tree.nodes.len(), self.deferred.len());
            if self.eat("case").await
                && let Some(pattern) = Box::pin(self.pattern()).await
            {
                let mut children = vec![pattern];
                while self.eat(",").await {
                    if let Some(label) = Box::pin(self.case_label()).await {
                        children.push(label);
                    } else {
                        children.push(self.error().await);
                        break;
                    }
                }
                return Box::pin(self.switch_rule(start, children)).await;
            }
            self.at = checkpoint.0;
            self.tree.nodes.truncate(checkpoint.1);
            self.deferred.truncate(checkpoint.2);
            let mut children = vec![];
            if self.text() == "default" {
                children.push(self.bump().await);
            } else {
                children.push(self.bump().await); // case
                let previous = self.guard_arrow;
                self.guard_arrow = Box::pin(self.switch_guard_arrow()).await;
                while !self.eof() && !matches!(self.text(), "->" | ":" | "}") {
                    let before = self.at;
                    if let Some(label) = Box::pin(self.case_label()).await {
                        children.push(label);
                    }
                    if self.at == before {
                        children.push(self.error().await);
                    }
                    if self.text() != "," {
                        break;
                    }
                    children.push(self.bump().await);
                }
                self.guard_arrow = previous;
                // javac parses a pattern after a constant and rejects the mix in Check, not the parser.
                let mixed = children.iter().any(|&id| {
                    matches!(
                        self.tree.nodes[id].kind.as_ref(),
                        "type_pattern" | "record_pattern" | "match_all_pattern"
                    )
                });
                if mixed {
                    children.retain(|&id| {
                        !matches!(
                            &self.source[self.tree.nodes[id].bytes.clone()],
                            "case" | ","
                        )
                    });
                    return Box::pin(self.switch_rule(start, children)).await;
                }
            }
            if self.text() != "}" {
                self.at += 1;
            }
            return self.node("switch_label", start, children, vec![]);
        }
        if self.ident() && self.nth(1) == ":" {
            self.at += 2;
            let child = Box::pin(self.statement()).await;
            return self.node("labeled_statement", start, vec![child], vec![]);
        }
        // `yield` is a restricted identifier, so a yield expression must be
        // recognized before the ambiguous `type name;` local-declaration shape.
        // It can still name a variable in assignment/member/array statements.
        // A switch rule body is an expression, block or throw (JLS 14.11.1), so
        // a `yield` directly after the arrow is an identifier operand. A
        // following operand is still recovered as the statement it was meant as.
        let after_arrow = self
            .at
            .checked_sub(1)
            .is_some_and(|previous| self.tokens[previous].text(self.source) == "->")
            && matches!(
                self.nth(1),
                "+" | "-"
                    | "*"
                    | "/"
                    | "%"
                    | "<<"
                    | ">>"
                    | ">>>"
                    | "<"
                    | ">"
                    | "<="
                    | ">="
                    | "=="
                    | "!="
                    | "&"
                    | "|"
                    | "^"
                    | "&&"
                    | "||"
                    | "?"
                    | ";"
                    | ")"
                    | ","
            );
        let yield_statement = self.text() == "yield"
            && !after_arrow
            && !matches!(
                self.nth(1),
                "=" | "+="
                    | "-="
                    | "*="
                    | "/="
                    | "%="
                    | "&="
                    | "|="
                    | "^="
                    | "<<="
                    | ">>="
                    | ">>>="
                    | "."
                    | "["
                    | "::"
            )
            && !(matches!(self.nth(1), "++" | "--") && self.nth(2) == ";");
        if !yield_statement && let Some(local) = Box::pin(self.local()).await {
            let mut c = vec![local];
            self.expect(";", &mut c).await;
            return self.node("declaration_statement", start, c, vec![]);
        }
        let mut children = vec![];
        let statement_kind = match self.text() {
            "return" => "return_statement",
            "throw" => "throw_statement",
            "yield" if yield_statement => "yield_statement",
            "break" => "break_statement",
            "continue" => "continue_statement",
            "assert" => "assert_statement",
            _ => "expression_statement",
        };
        if statement_kind != "expression_statement" {
            self.at += 1;
        }
        if !matches!(self.text(), ";" | "}") && !self.eof() {
            children.push(Box::pin(self.expr(0)).await);
        }
        if self.eat(":").await {
            children.push(Box::pin(self.expr(0)).await);
        }
        self.expect(";", &mut children).await;
        self.node(statement_kind, start, children, vec![])
    }
    /// One label of a `case`: `default`, a pattern, or a constant expression.
    async fn case_label(&mut self) -> Option<usize> {
        if self.text() == "default" {
            return Some(self.bump().await);
        }
        if let Some(pattern) = Box::pin(self.pattern()).await {
            return Some(pattern);
        }
        if matches!(self.text(), "->" | ":" | "}") || self.eof() {
            return None;
        }
        // Case constants are conditional expressions. Retain their structure
        // for native constant/type analysis.
        Some(Box::pin(self.expr(2)).await)
    }
    /// The guard, arrow or colon, and statements that follow the labels of a
    /// case whose labels include a pattern.
    async fn switch_rule(&mut self, start: usize, mut children: Vec<usize>) -> usize {
        let mut fields = vec![];
        if self.eat("when").await {
            let previous = self.guard_arrow;
            self.guard_arrow = Box::pin(self.switch_guard_arrow()).await;
            let guard = Box::pin(self.expr(0)).await;
            children.push(guard);
            fields.push(("guard", guard));
            self.guard_arrow = previous;
        }
        let arrow = self.eat("->").await;
        if !arrow {
            self.expect(":", &mut children).await;
        }
        while !self.eof() && !matches!(self.text(), "case" | "default" | "}") {
            let before = self.at;
            children.push(Box::pin(self.statement()).await);
            if self.at == before {
                children.push(self.error().await);
            }
            if arrow {
                break;
            }
        }
        self.node("switch_rule", start, children, fields)
    }
    async fn for_statement(&mut self, start: usize) -> usize {
        let mut children = vec![];
        self.expect("(", &mut children).await;
        // Enhanced-for variables are declared on the loop, matching its scope.
        let checkpoint = (self.at, self.tree.nodes.len(), self.deferred.len());
        let mods = Box::pin(self.modifiers()).await;
        let ty = Box::pin(self.ty()).await;
        if let Some(ty) = ty.filter(|_| (self.ident() || self.text() == "_") && self.nth(1) == ":")
        {
            children.extend(mods);
            children.push(ty);
            let name = self.binding_name().await;
            children.push(name);
            self.at += 1;
            let value = Box::pin(self.expr(0)).await;
            children.push(value);
            self.expect(")", &mut children).await;
            let body = Box::pin(self.statement()).await;
            children.push(body);
            return self.node(
                "enhanced_for_statement",
                start,
                children,
                vec![("name", name), ("body", body), ("value", value)],
            );
        }
        self.at = checkpoint.0;
        self.tree.nodes.truncate(checkpoint.1);
        self.deferred.truncate(checkpoint.2);
        if self.text() != ";" {
            if let Some(local) = Box::pin(self.local()).await {
                children.push(local);
            } else {
                children.push(Box::pin(self.expr(0)).await);
                while self.eat(",").await {
                    children.push(Box::pin(self.expr(0)).await);
                }
            }
        }
        self.expect(";", &mut children).await;
        let mut fields = vec![];
        if self.text() != ";" {
            let condition = Box::pin(self.expr(0)).await;
            children.push(condition);
            fields.push(("condition", condition));
        }
        self.expect(";", &mut children).await;
        if self.text() != ")" {
            children.push(Box::pin(self.expr(0)).await);
            while self.eat(",").await {
                children.push(Box::pin(self.expr(0)).await);
            }
        }
        self.expect(")", &mut children).await;
        let body = Box::pin(self.statement()).await;
        children.push(body);
        fields.push(("body", body));
        self.node("for_statement", start, children, fields)
    }
    async fn try_statement(&mut self, start: usize) -> usize {
        let mut children = vec![];
        let resources = self.eat("(").await;
        if resources {
            while !self.eof() && self.text() != ")" {
                if let Some(local) = Box::pin(self.local()).await {
                    children.push(local);
                } else {
                    children.push(Box::pin(self.expr(0)).await);
                }
                if !self.eat(";").await {
                    break;
                }
            }
            self.expect(")", &mut children).await;
        }
        if self.text() == "{" {
            children.push(Box::pin(self.block()).await);
        } else {
            children.push(self.error().await);
        }
        if resources {
            let scope = self.node("try_with_resources_statement", start, children, vec![]);
            children = vec![scope];
        }
        while self.eat("catch").await {
            let s = self.end();
            let mut c = vec![];
            self.expect("(", &mut c).await;
            let parameter_start = self.pos();
            let mut parameter_children = Box::pin(self.modifiers()).await;
            loop {
                if let Some(ty) = Box::pin(self.ty()).await {
                    parameter_children.push(ty);
                } else {
                    parameter_children.push(self.error().await);
                }
                if !self.eat("|").await {
                    break;
                }
            }
            let n = self.binding_name().await;
            parameter_children.push(n);
            let param = self.node(
                "catch_formal_parameter",
                parameter_start,
                parameter_children,
                vec![("name", n)],
            );
            c.push(param);
            self.expect(")", &mut c).await;
            if self.text() == "{" {
                c.push(Box::pin(self.block()).await);
            } else {
                c.push(self.error().await);
            }
            children.push(self.node("catch_clause", s, c, vec![]));
        }
        if self.eat("finally").await {
            if self.text() == "{" {
                children.push(Box::pin(self.block()).await);
            } else {
                children.push(self.error().await);
            }
        }
        self.node("try_statement", start, children, vec![])
    }
    async fn arguments(&mut self) -> usize {
        if let Some(hole) = self.defer_balanced(DeferredKind::Arguments).await {
            return hole;
        }
        let start = self.pos();
        self.at += 1;
        let mut children = vec![];
        while !self.eof() && !matches!(self.text(), ")" | ";" | "}") {
            let before = self.at;
            children.push(Box::pin(self.expr(0)).await);
            if self.at == before {
                children.push(self.error().await);
            }
            if !self.eat(",").await {
                break;
            }
        }
        self.expect(")", &mut children).await;
        self.node("argument_list", start, children, vec![])
    }
    async fn lambda_end(&mut self) -> Option<usize> {
        if self.text() != "(" {
            return None;
        }
        let mut depth = 0;
        for i in self.at..self.stop {
            self.tick().await;
            match self.tokens[i].text(self.source) {
                "(" => depth += 1,
                ")" => {
                    depth -= 1;
                    if depth == 0 {
                        return (self
                            .token(i + 1)
                            .is_some_and(|t| t.text(self.source) == "->"))
                        .then_some(i);
                    }
                }
                "{" | ";" => return None,
                _ => {}
            }
        }
        None
    }
    async fn switch_guard_arrow(&mut self) -> Option<usize> {
        let mut nesting = 0usize;
        let mut conditional = 0usize;
        for i in self.at..self.stop {
            self.tick().await;
            match self.tokens[i].text(self.source) {
                "(" | "[" | "{" => nesting += 1,
                ")" | "]" | "}" => nesting = nesting.checked_sub(1)?,
                "->" if nesting == 0 => return Some(i),
                "?" if nesting == 0 => conditional += 1,
                ":" if nesting == 0 => conditional = conditional.checked_sub(1)?,
                ";" if nesting == 0 => return None,
                _ => {}
            }
        }
        None
    }
    async fn expr(&mut self, min: u8) -> usize {
        self.tick().await;
        if self.depth >= 48 {
            return self.error().await;
        }
        self.depth += 1;
        let node = Box::pin(self.expr_inner(min)).await;
        self.depth -= 1;
        node
    }
    async fn pattern(&mut self) -> Option<usize> {
        if self.depth >= 48 {
            return None;
        }
        self.depth += 1;
        let checkpoint = (self.at, self.tree.nodes.len(), self.deferred.len());
        let start = self.pos();
        let modifiers = Box::pin(self.modifiers()).await;
        let result = if self.eat("_").await {
            Some(self.node("match_all_pattern", start, modifiers, vec![]))
        } else if let Some(ty) = Box::pin(self.ty()).await {
            if self.eat("(").await {
                let mut children = modifiers;
                children.push(ty);
                while !self.eof() && !matches!(self.text(), ")" | "->" | ":" | "}") {
                    if let Some(pattern) = Box::pin(self.pattern()).await {
                        children.push(pattern);
                    } else {
                        children.push(self.error().await);
                    }
                    if !self.eat(",").await {
                        break;
                    }
                }
                self.expect(")", &mut children).await;
                Some(self.node("record_pattern", start, children, vec![]))
            } else if self.ident() || self.text() == "_" {
                let name = self.binding_name().await;
                let mut children = modifiers;
                children.extend([ty, name]);
                Some(self.node("type_pattern", start, children, vec![("name", name)]))
            } else {
                None
            }
        } else {
            None
        };
        self.depth -= 1;
        if result.is_none() {
            self.at = checkpoint.0;
            self.tree.nodes.truncate(checkpoint.1);
            self.deferred.truncate(checkpoint.2);
        }
        result
    }
    async fn primary(&mut self) -> usize {
        let start = self.pos();
        if self.text() == "@" {
            let checkpoint = (self.at, self.tree.nodes.len(), self.deferred.len());
            let ty = Box::pin(self.ty()).await;
            if let Some(ty) = ty.filter(|_| self.text() == "::") {
                self.node("type_expression", start, vec![ty], vec![])
            } else if let Some(ty) = ty.filter(|_| self.text() == "." && self.nth(1) == "class") {
                let mut children = vec![ty];
                self.expect(".", &mut children).await;
                children.push(self.bump().await);
                self.node("class_literal", start, children, vec![("object", ty)])
            } else {
                self.at = checkpoint.0;
                self.tree.nodes.truncate(checkpoint.1);
                self.deferred.truncate(checkpoint.2);
                Box::pin(self.annotation()).await
            }
        } else if (self.ident() || self.text() == "_")
            && self.nth(1) == "->"
            && self.guard_arrow != Some(self.at + 1)
            || Box::pin(self.lambda_end())
                .await
                .is_some_and(|end| self.guard_arrow != Some(end + 1))
        {
            let mut children = vec![];
            let param = if self.eat("(").await {
                let s = self.pos();
                let mut params = vec![];
                while !self.eof() && self.text() != ")" {
                    if (self.ident() || self.text() == "_") && matches!(self.nth(1), "," | ")") {
                        params.push(self.binding_name().await);
                    } else {
                        let p = self.pos();
                        let mut c = Box::pin(self.modifiers()).await;
                        if let Some(ty) = Box::pin(self.ty()).await {
                            c.push(ty);
                        }
                        let spread = self.eat("...").await;
                        let n = self.binding_name().await;
                        c.push(n);
                        Box::pin(self.empty_dimensions(&mut c)).await;
                        params.push(self.node(
                            if spread {
                                "spread_parameter"
                            } else {
                                "formal_parameter"
                            },
                            p,
                            c,
                            vec![("name", n)],
                        ));
                    }
                    if !self.eat(",").await {
                        break;
                    }
                }
                self.expect(")", &mut params).await;
                self.node("inferred_parameters", s, params, vec![])
            } else {
                self.binding_name().await
            };
            children.push(param);
            self.expect("->", &mut children).await;
            if self.text() == "{" {
                children.push(Box::pin(self.block()).await);
            } else {
                children.push(Box::pin(self.expr(0)).await);
            }
            self.node(
                "lambda_expression",
                start,
                children,
                vec![("parameters", param)],
            )
        } else if matches!(self.text(), "!" | "~" | "+" | "-" | "++" | "--") {
            self.at += 1;
            let value = Box::pin(self.expr(14)).await;
            self.node("unary_expression", start, vec![value], vec![])
        } else if self.eat("new").await {
            let mut children = vec![];
            let mut fields = vec![];
            if self.text() == "<" {
                let types = Box::pin(self.type_arguments()).await;
                children.push(types);
                fields.push(("type_arguments", types));
            }
            if let Some(ty) = Box::pin(self.ty()).await {
                children.push(ty);
                fields.push(("type", ty));
            } else {
                children.push(self.error().await);
            }
            let constructor = self.text() == "(";
            if constructor {
                children.push(Box::pin(self.arguments()).await);
            } else if !matches!(self.text(), "[" | "@" | "{")
                && fields.iter().any(|&(name, ty)| {
                    name == "type" && self.tree.nodes[ty].kind.as_ref() != "array_type"
                })
            {
                children.push(self.missing_at_token());
            }
            while matches!(self.text(), "[" | "@") {
                let s = self.pos();
                let mut dimension = vec![];
                while self.text() == "@" {
                    dimension.push(Box::pin(self.annotation()).await);
                }
                self.expect("[", &mut dimension).await;
                if self.text() != "]" {
                    dimension.push(Box::pin(self.expr(0)).await);
                }
                self.expect("]", &mut dimension).await;
                children.push(self.node("dimension", s, dimension, vec![]));
            }
            if self.text() == "{" {
                children.push(if constructor {
                    Box::pin(self.anonymous_body()).await
                } else {
                    Box::pin(self.array_initializer()).await
                });
            }
            self.node(
                if constructor {
                    "object_creation_expression"
                } else {
                    "array_creation_expression"
                },
                start,
                children,
                fields,
            )
        } else if self.text() == "<" {
            let types = Box::pin(self.type_arguments()).await;
            let name = if matches!(self.text(), "this" | "super") {
                self.bump().await
            } else {
                self.name().await
            };
            self.node(
                "generic_method_name",
                start,
                vec![types, name],
                vec![("name", name), ("type_arguments", types)],
            )
        } else if self.eat("switch").await {
            let mut c = vec![];
            self.expect("(", &mut c).await;
            c.push(Box::pin(self.expr(0)).await);
            self.expect(")", &mut c).await;
            if self.text() == "{" {
                c.push(Box::pin(self.block()).await);
            } else {
                c.push(self.error().await);
            }
            self.node("switch_expression", start, c, vec![])
        } else if self.eat("(").await {
            let checkpoint = (self.at, self.tree.nodes.len(), self.deferred.len());
            let mut ty = Box::pin(self.ty()).await;
            if let Some(first) = ty
                && self.text() == "&"
            {
                let type_start = self.tree.nodes[first].bytes.start;
                let mut bounds = vec![first];
                while self.eat("&").await {
                    if let Some(bound) = Box::pin(self.ty()).await {
                        bounds.push(bound);
                    } else {
                        bounds.push(self.error().await);
                    }
                }
                ty = Some(self.node("type", type_start, bounds, vec![]));
            }
            let primitive = ty.is_some_and(|n| {
                matches!(
                    &self.source[self.tree.nodes[n].bytes.clone()],
                    "int" | "long" | "byte" | "short" | "char" | "float" | "double" | "boolean"
                )
            });
            let cast = ty.is_some()
                && self.eat(")").await
                && (self.ident()
                    || matches!(
                        self.text(),
                        "new"
                            | "this"
                            | "super"
                            | "null"
                            | "true"
                            | "false"
                            | "switch"
                            | "("
                            | "!"
                            | "~"
                    )
                    || is_primitive_type_keyword(self.text())
                    || primitive && matches!(self.text(), "+" | "-")
                    || self.token(self.at).is_some_and(|t| {
                        matches!(
                            t.kind,
                            JavaTokenKind::Number
                                | JavaTokenKind::String
                                | JavaTokenKind::Character
                        )
                    }));
            if cast {
                let value = Box::pin(self.expr(14)).await;
                self.node("cast_expression", start, vec![ty.unwrap(), value], vec![])
            } else {
                self.at = checkpoint.0;
                self.tree.nodes.truncate(checkpoint.1);
                self.deferred.truncate(checkpoint.2);
                let mut c = vec![Box::pin(self.expr(0)).await];
                self.expect(")", &mut c).await;
                self.node("parenthesized_expression", start, c, vec![])
            }
        } else if is_primitive_type_keyword(self.text()) {
            let array_reference = self.text() != "void";
            let Some(ty) = Box::pin(self.ty()).await else {
                return self.error().await;
            };
            if array_reference
                && self.tree.nodes[ty].kind.as_ref() == "array_type"
                && self.text() == "::"
                && self.tree.nodes[ty]
                    .children
                    .iter()
                    .filter(|&&child| self.tree.nodes[child].kind.as_ref() == "type_identifier")
                    .count()
                    == 1
                && self.tree.nodes[ty].children.iter().all(|&child| {
                    matches!(
                        self.tree.nodes[child].kind.as_ref(),
                        "type_identifier" | "annotation"
                    )
                })
                && self.source[self.tree.nodes[ty].bytes.clone()].ends_with(']')
            {
                return self.node("array_type_expression", start, vec![ty], vec![]);
            }
            let mut children = vec![ty];
            self.expect(".", &mut children).await;
            if self.text() == "class" {
                children.push(self.bump().await);
            } else {
                self.expect("class", &mut children).await;
            }
            self.node("class_literal", start, children, vec![("object", ty)])
        } else if self.text() == "{" {
            Box::pin(self.array_initializer()).await
        } else if self.ident()
            || matches!(self.text(), "this" | "super" | "true" | "false" | "null")
            || self.token(self.at).is_some_and(|t| {
                matches!(
                    t.kind,
                    JavaTokenKind::Number
                        | JavaTokenKind::String
                        | JavaTokenKind::Character
                        | JavaTokenKind::Error
                )
            })
        {
            self.bump().await
        } else {
            self.missing_at_token()
        }
    }
    async fn expr_inner(&mut self, min: u8) -> usize {
        let start = self.pos();
        let mut left = Box::pin(self.primary()).await;
        loop {
            if self.eof() {
                break;
            }
            // JLS 15.28: a switch expression is not a Primary, so no selector,
            // invocation, index, postfix or method reference can follow it.
            if self.tree.nodes[left].kind.as_ref() == "switch_expression"
                && matches!(self.text(), "." | "(" | "[" | "++" | "--" | "::")
            {
                break;
            }
            if self.eat(".").await {
                if self.eat("new").await {
                    let mut children = vec![left];
                    let mut fields = vec![("object", left)];
                    if self.text() == "<" {
                        let types = Box::pin(self.type_arguments()).await;
                        children.push(types);
                        fields.push(("type_arguments", types));
                    }
                    if let Some(ty) = Box::pin(self.ty()).await {
                        children.push(ty);
                        fields.push(("type", ty));
                    } else {
                        children.push(self.error().await);
                    }
                    if self.text() == "(" {
                        children.push(Box::pin(self.arguments()).await);
                    } else {
                        self.expect_at_token("(", &mut children).await;
                    }
                    if self.text() == "{" {
                        children.push(Box::pin(self.anonymous_body()).await);
                    }
                    left = self.node("object_creation_expression", start, children, fields);
                    continue;
                }
                let types = if self.text() == "<" {
                    Some(Box::pin(self.type_arguments()).await)
                } else {
                    None
                };
                let field = if matches!(self.text(), "class" | "this" | "super") {
                    self.bump().await
                } else {
                    self.name().await
                };
                let mut children = vec![left, field];
                let mut fields = vec![("object", left), ("field", field)];
                let selected = &self.source[self.tree.nodes[field].bytes.clone()];
                let expression_only = matches!(
                    self.tree.nodes[left].kind.as_ref(),
                    "this"
                        | "super"
                        | "string_literal"
                        | "character_literal"
                        | "number_literal"
                        | "method_invocation"
                        | "object_creation_expression"
                        | "array_creation_expression"
                        | "array_access"
                        | "parenthesized_expression"
                        | "cast_expression"
                        | "class_literal"
                ) || matches!(
                    &self.source[self.tree.nodes[left].bytes.clone()],
                    "null" | "true" | "false"
                );
                // Names and ordinary field selections may denote types. Their
                // legality requires binding evidence, unlike these value forms.
                if selected == "class" && expression_only
                    || selected == "super" && !matches!(self.text(), "." | "::" | "(")
                {
                    children.push(self.tree.add(
                        "ERROR",
                        self.tree.nodes[field].bytes.clone(),
                        vec![],
                        vec![],
                    ));
                }
                if let Some(types) = types {
                    children.push(types);
                    fields.push(("type_arguments", types));
                    // Type arguments after '.' belong to a method invocation,
                    // never to a field selected before another '.' or '['.
                    if self.text() != "(" {
                        children.push(self.tree.add(
                            "ERROR",
                            self.tree.nodes[types].bytes.clone(),
                            vec![],
                            vec![],
                        ));
                    }
                }
                let kind = if &self.source[self.tree.nodes[field].bytes.clone()] == "class" {
                    "class_literal"
                } else {
                    "field_access"
                };
                left = self.node(kind, start, children, fields);
                continue;
            }
            if self.text() == "(" {
                let args = Box::pin(self.arguments()).await;
                let mut c = vec![];
                let mut fields = vec![("arguments", args)];
                let element = &self.tree.nodes[left];
                if element.kind == "field_access" {
                    let obj = element.fields[0].1;
                    let name = element.fields[1].1;
                    c.extend([obj, name, args]);
                    fields.extend([("object", obj), ("name", name)]);
                } else if element.kind == "generic_method_name" {
                    let name = element.fields[0].1;
                    c.extend([name, args]);
                    fields.push(("name", name));
                } else {
                    c.extend([left, args]);
                    fields.push(("name", left));
                }
                if let Some((_, types)) = element
                    .fields
                    .iter()
                    .find(|(name, _)| *name == "type_arguments")
                {
                    c.push(*types);
                    fields.push(("type_arguments", *types));
                }
                left = self.node("method_invocation", start, c, fields);
                continue;
            }
            if self.text() == "@"
                && matches!(
                    self.tree.nodes[left].kind.as_ref(),
                    "identifier" | "field_access" | "array_type_expression"
                )
            {
                // Annotated dimensions of a type used as an expression: `String @A [].class`.
                let checkpoint = (self.at, self.tree.nodes.len(), self.deferred.len());
                let mut children = vec![left];
                while self.text() == "@" && self.nth(1) != "interface" {
                    children.push(Box::pin(self.annotation()).await);
                }
                if self.text() == "[" && self.nth(1) == "]" {
                    self.at += 2;
                    left = self.node("array_type_expression", start, children, vec![]);
                    continue;
                }
                self.at = checkpoint.0;
                self.tree.nodes.truncate(checkpoint.1);
                self.deferred.truncate(checkpoint.2);
            }
            if self.eat("[").await {
                if self.eat("]").await {
                    left = self.node("array_type_expression", start, vec![left], vec![]);
                    continue;
                }
                let idx = Box::pin(self.expr(0)).await;
                let mut c = vec![left, idx];
                self.expect("]", &mut c).await;
                left = self.node("array_access", start, c, vec![]);
                continue;
            }
            if matches!(self.text(), "++" | "--") {
                self.at += 1;
                left = self.node("update_expression", start, vec![left], vec![]);
                continue;
            }
            if self.text() == "<"
                && matches!(
                    self.tree.nodes[left].kind.as_ref(),
                    "identifier" | "field_access" | "type_expression"
                )
            {
                let checkpoint = (self.at, self.tree.nodes.len(), self.deferred.len());
                let mut children = vec![left, Box::pin(self.type_arguments()).await];
                while self.eat(".").await {
                    children.push(self.name().await);
                    if self.text() == "<" {
                        children.push(Box::pin(self.type_arguments()).await);
                    }
                }
                let mut array = false;
                while self.eat("[").await {
                    array = true;
                    self.expect("]", &mut children).await;
                }
                if self.text() == "::" {
                    left = self.node(
                        if array {
                            "array_type_expression"
                        } else {
                            "type_expression"
                        },
                        start,
                        children,
                        vec![],
                    );
                    continue;
                }
                // A relational/shift expression can start with the same token
                // sequence. Speculation must not change its nodes or cursor.
                self.at = checkpoint.0;
                self.tree.nodes.truncate(checkpoint.1);
                self.deferred.truncate(checkpoint.2);
            }
            if self.eat("::").await {
                let types = if self.text() == "<" {
                    Some(Box::pin(self.type_arguments()).await)
                } else {
                    None
                };
                let name = if self.text() == "new" {
                    self.bump().await
                } else {
                    self.name().await
                };
                let mut children = vec![left, name];
                let mut fields = vec![("object", left), ("name", name)];
                if let Some(types) = types {
                    children.push(types);
                    fields.push(("type_arguments", types));
                }
                left = self.node("method_reference", start, children, fields);
                continue;
            }
            if self.text() == "?" && min <= 2 {
                self.at += 1;
                let yes = Box::pin(self.expr(0)).await;
                let mut c = vec![left, yes];
                self.expect(":", &mut c).await;
                c.push(Box::pin(self.expr(2)).await);
                left = self.node("ternary_expression", start, c, vec![]);
                continue;
            }
            let Some((precedence, right_assoc, length)) = self.binary_operator() else {
                break;
            };
            if precedence < min {
                break;
            }
            let instance = self.text() == "instanceof";
            let op_start = self.pos();
            self.at += length;
            let operator = self
                .tree
                .add("operator", op_start..self.end(), vec![], vec![]);
            let right = if instance {
                if let Some(pattern) = Box::pin(self.pattern()).await {
                    pattern
                } else if let Some(ty) = Box::pin(self.ty()).await {
                    ty
                } else {
                    self.error().await
                }
            } else {
                Box::pin(self.expr(if right_assoc {
                    precedence
                } else {
                    precedence + 1
                }))
                .await
            };
            let c = vec![left, operator, right];
            left = self.node(
                if right_assoc {
                    "assignment_expression"
                } else {
                    "binary_expression"
                },
                start,
                c,
                vec![("left", left), ("operator", operator), ("right", right)],
            );
        }
        left
    }
    fn binary_operator(&self) -> Option<(u8, bool, usize)> {
        if matches!(self.text(), "<" | ">") {
            let mut count = 1;
            for i in 1..4 {
                let Some(next) = self.token(self.at + i) else {
                    break;
                };
                if self.tokens[self.at + i - 1].bytes.end != next.bytes.start {
                    break;
                }
                let op = &self.source[self.pos()..next.bytes.end];
                if matches!(
                    op,
                    "<=" | ">=" | "<<" | ">>" | ">>>" | "<<=" | ">>=" | ">>>="
                ) {
                    count = i + 1;
                } else {
                    break;
                }
            }
            let op = &self.source[self.pos()..self.tokens[self.at + count - 1].bytes.end];
            return Some(if op.ends_with('=') && op.len() > 2 {
                (1, true, count)
            } else if op.len() > 1 && !op.ends_with('=') {
                (10, false, count)
            } else {
                (9, false, count)
            });
        }
        Some(match self.text() {
            "=" | "+=" | "-=" | "*=" | "/=" | "%=" | "&=" | "|=" | "^=" => (1, true, 1),
            "||" => (3, false, 1),
            "&&" => (4, false, 1),
            "|" => (5, false, 1),
            "^" => (6, false, 1),
            "&" => (7, false, 1),
            "==" | "!=" => (8, false, 1),
            "instanceof" => (9, false, 1),
            "+" | "-" => (11, false, 1),
            "*" | "/" | "%" => (12, false, 1),
            _ => return None,
        })
    }
    async fn array_initializer(&mut self) -> usize {
        if self.depth >= 48 {
            return self.error().await;
        }
        self.depth += 1;
        let start = self.pos();
        self.at += 1;
        let mut c = vec![];
        if self.text() == "," && self.nth(1) == "}" {
            self.at += 1;
        }
        while !self.eof() && self.text() != "}" {
            let before = self.at;
            c.push(Box::pin(self.expr(0)).await);
            if self.at == before {
                c.push(self.error().await);
            }
            if !self.eat(",").await {
                break;
            }
        }
        self.expect("}", &mut c).await;
        self.depth -= 1;
        self.node("array_initializer", start, c, vec![])
    }
}
