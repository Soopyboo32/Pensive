use regex::bytes::{Regex, RegexBuilder};
use text::Rope;
use tree_sitter::{Node, QueryMatch, QueryPredicate, QueryPredicateArg};

use super::node_text;

pub(super) enum Predicate {
    Text {
        capture: u32,
        regex: Regex,
        positive: bool,
    },
    HasAncestor {
        capture: u32,
        kinds: Vec<Box<str>>,
    },
    NotHasParent {
        capture: u32,
        kinds: Vec<Box<str>>,
    },
}

impl Predicate {
    pub(super) fn new(predicate: &QueryPredicate) -> Option<Self> {
        let [QueryPredicateArg::Capture(capture), args @ ..] = predicate.args.as_ref() else {
            return None;
        };
        let capture = *capture;
        let args = args
            .iter()
            .map(|arg| match arg {
                QueryPredicateArg::String(value) => Some(value.clone()),
                QueryPredicateArg::Capture(_) => None,
            })
            .collect::<Option<Vec<_>>>()?;
        match (predicate.operator.as_ref(), args.as_slice()) {
            (op @ ("lua-match?" | "not-lua-match?"), [pattern]) => Some(Self::Text {
                capture,
                regex: lua_pattern(pattern)?,
                positive: op == "lua-match?",
            }),
            ("has-ancestor?", [_, ..]) => Some(Self::HasAncestor {
                capture,
                kinds: args,
            }),
            ("not-has-parent?", [_, ..]) => Some(Self::NotHasParent {
                capture,
                kinds: args,
            }),
            _ => None,
        }
    }

    pub(super) fn matches(&self, matched: &QueryMatch<'_, '_>, rope: &Rope) -> bool {
        match self {
            Self::Text {
                capture,
                regex,
                positive,
            } => matched
                .nodes_for_capture_index(*capture)
                .all(|node| matches_text(regex, rope, node) == *positive),
            Self::HasAncestor { capture, kinds } => {
                matched.nodes_for_capture_index(*capture).all(|node| {
                    std::iter::successors(node.parent(), Node::parent)
                        .any(|node| kinds.iter().any(|kind| node.kind() == kind.as_ref()))
                })
            }
            Self::NotHasParent { capture, kinds } => {
                matched.nodes_for_capture_index(*capture).all(|node| {
                    node.parent().is_none_or(|parent| {
                        kinds.iter().all(|kind| parent.kind() != kind.as_ref())
                    })
                })
            }
        }
    }
}

fn matches_text(regex: &Regex, rope: &Rope, node: Node<'_>) -> bool {
    let mut chunks = node_text(rope, node);
    let first = chunks.next().unwrap_or_default();
    let Some(second) = chunks.next() else {
        return regex.is_match(first);
    };
    let mut bytes = Vec::with_capacity(node.byte_range().len());
    bytes.extend_from_slice(first);
    bytes.extend_from_slice(second);
    bytes.extend(chunks.flatten().copied());
    regex.is_match(&bytes)
}

// Translate the Lua patterns in the bundled queries. Reject new patterns until
// their semantics are checked, since Lua and regular expressions differ.
fn lua_pattern(pattern: &str) -> Option<Regex> {
    let regex = match pattern {
        "^[^,]*,%s*(%w+)" => r"^[^,]*,[\x09-\x0d ]*[A-Za-z0-9]+",
        "^[%u@][%u%d_]+$" => "^[A-Z@][A-Z0-9_]+$",
        "^;+%s*format%-ignore%s*$" => r"^;+[\x09-\x0d ]*format-ignore[\x09-\x0d ]*$",
        "^#!/"
        | "^gl_"
        | "[A-Z0-9]+"
        | "^[A-Z][a-zA-Z0-9]*$"
        | "^_*[A-Z][A-Z0-9_]*$"
        | "^;+ *inherits *:"
        | "^;+ *extends *$"
        | "^[_]*[A-Z]"
        | "^[_A-Z][A-Z0-9_]*$"
        | "^/[*][*][^*].*[*]/$"
        | "^///[^/]"
        | "^///$"
        | "#!.*" => pattern,
        _ => return None,
    };
    RegexBuilder::new(regex)
        .unicode(false)
        .dot_matches_new_line(true)
        .build()
        .ok()
}
