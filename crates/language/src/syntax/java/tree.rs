//! Immutable arena-backed syntax elements with source ranges and named fields.
use std::{borrow::Cow, ops::Range};
#[derive(Clone, Debug)]
pub(crate) struct Element {
    pub kind: Cow<'static, str>,
    pub bytes: Range<usize>,
    pub children: Vec<usize>,
    pub fields: Vec<(Cow<'static, str>, usize)>,
    pub parent: Option<usize>,
}
#[derive(Clone, Debug, Default)]
pub struct JavaTree {
    pub(crate) nodes: Vec<Element>,
    pub(crate) root: usize,
}
#[derive(Clone, Copy, Debug)]
pub struct Node<'a> {
    tree: &'a JavaTree,
    id: usize,
}
impl PartialEq for Node<'_> {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self.tree, other.tree) && self.id == other.id
    }
}
impl Eq for Node<'_> {}
impl JavaTree {
    pub fn root_node(&self) -> Node<'_> {
        Node {
            tree: self,
            id: self.root,
        }
    }
    pub(crate) fn add(
        &mut self,
        kind: &'static str,
        bytes: Range<usize>,
        children: Vec<usize>,
        fields: Vec<(&'static str, usize)>,
    ) -> usize {
        self.add_owned(
            kind.into(),
            bytes,
            children,
            fields.into_iter().map(|(f, id)| (f.into(), id)).collect(),
        )
    }
    pub(crate) fn add_owned(
        &mut self,
        kind: Cow<'static, str>,
        bytes: Range<usize>,
        children: Vec<usize>,
        fields: Vec<(Cow<'static, str>, usize)>,
    ) -> usize {
        let id = self.nodes.len();
        for &child in &children {
            self.nodes[child].parent = Some(id);
        }
        self.nodes.push(Element {
            kind,
            bytes,
            children,
            fields,
            parent: None,
        });
        id
    }
}
impl<'a> Node<'a> {
    pub fn kind(self) -> &'a str {
        &self.tree.nodes[self.id].kind
    }
    pub fn start_byte(self) -> usize {
        self.tree.nodes[self.id].bytes.start
    }
    pub fn bytes(self) -> Range<usize> {
        self.tree.nodes[self.id].bytes.clone()
    }
    pub fn parent(self) -> Option<Self> {
        self.tree.nodes[self.id].parent.map(|id| Self {
            tree: self.tree,
            id,
        })
    }
    pub fn child_by_field_name(self, name: &str) -> Option<Self> {
        self.tree.nodes[self.id]
            .fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, id)| Self {
                tree: self.tree,
                id: *id,
            })
    }
    pub fn children(self) -> impl DoubleEndedIterator<Item = Self> + 'a {
        self.tree.nodes[self.id]
            .children
            .iter()
            .map(move |&id| Self {
                tree: self.tree,
                id,
            })
    }
}
