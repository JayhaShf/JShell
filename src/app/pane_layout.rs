#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PaneLeaf {
    Terminal(String),
    Document(String),
    Empty,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PaneLayout {
    Leaf(PaneLeaf),
    Horizontal(Vec<PaneLayout>, f32),
    Vertical(Vec<PaneLayout>, f32),
}

impl PaneLayout {
    pub(crate) fn terminal(id: impl Into<String>) -> Self {
        Self::Leaf(PaneLeaf::Terminal(id.into()))
    }

    pub(crate) fn empty() -> Self {
        Self::Leaf(PaneLeaf::Empty)
    }

    pub(crate) fn terminal_ids(&self) -> Vec<&str> {
        let mut ids = Vec::new();
        self.collect_ids(&mut ids, |leaf| match leaf {
            PaneLeaf::Terminal(id) => Some(id.as_str()),
            _ => None,
        });
        ids
    }

    pub(crate) fn document_ids(&self) -> Vec<&str> {
        let mut ids = Vec::new();
        self.collect_ids(&mut ids, |leaf| match leaf {
            PaneLeaf::Document(id) => Some(id.as_str()),
            _ => None,
        });
        ids
    }

    pub(crate) fn leaves(&self) -> Vec<&PaneLeaf> {
        let mut leaves = Vec::new();
        self.collect_leaves(&mut leaves);
        leaves
    }

    fn collect_leaves<'a>(&'a self, leaves: &mut Vec<&'a PaneLeaf>) {
        match self {
            Self::Leaf(PaneLeaf::Empty) => {}
            Self::Leaf(leaf) => leaves.push(leaf),
            Self::Horizontal(children, _) | Self::Vertical(children, _) => {
                for child in children {
                    child.collect_leaves(leaves);
                }
            }
        }
    }

    fn collect_ids<'a>(
        &'a self,
        ids: &mut Vec<&'a str>,
        select: impl Copy + Fn(&'a PaneLeaf) -> Option<&'a str>,
    ) {
        match self {
            Self::Leaf(leaf) => {
                if let Some(id) = select(leaf) {
                    ids.push(id);
                }
            }
            Self::Horizontal(children, _) | Self::Vertical(children, _) => {
                for child in children {
                    child.collect_ids(ids, select);
                }
            }
        }
    }

    pub(crate) fn leaf_count(&self) -> usize {
        match self {
            Self::Leaf(PaneLeaf::Empty) => 0,
            Self::Leaf(_) => 1,
            Self::Horizontal(children, _) | Self::Vertical(children, _) => {
                children.iter().map(Self::leaf_count).sum()
            }
        }
    }

    pub(crate) fn contains_terminal(&self, id: &str) -> bool {
        self.terminal_ids().contains(&id)
    }

    pub(crate) fn contains_document(&self, id: &str) -> bool {
        self.document_ids().contains(&id)
    }

    pub(crate) fn tab_ids(&self) -> Vec<&str> {
        self.terminal_ids()
    }

    pub(crate) fn contains(&self, id: &str) -> bool {
        self.contains_terminal(id)
    }

    pub(crate) fn focused_tab_id(&self, path: &[usize]) -> Option<&str> {
        match self.focused_leaf(path) {
            Some(PaneLeaf::Terminal(id)) => Some(id),
            _ => None,
        }
    }

    pub(crate) fn path_to_terminal(&self, id: &str) -> Option<Vec<usize>> {
        self.path_to_leaf(|leaf| matches!(leaf, PaneLeaf::Terminal(value) if value == id))
    }

    pub(crate) fn path_to_document(&self, id: &str) -> Option<Vec<usize>> {
        self.path_to_leaf(|leaf| matches!(leaf, PaneLeaf::Document(value) if value == id))
    }

    pub(crate) fn first_leaf_path(&self) -> Option<Vec<usize>> {
        self.path_to_leaf(|leaf| !matches!(leaf, PaneLeaf::Empty))
    }

    fn path_to_leaf(&self, predicate: impl Copy + Fn(&PaneLeaf) -> bool) -> Option<Vec<usize>> {
        fn visit(
            layout: &PaneLayout,
            path: &mut Vec<usize>,
            predicate: &impl Fn(&PaneLeaf) -> bool,
        ) -> bool {
            match layout {
                PaneLayout::Leaf(leaf) => predicate(leaf),
                PaneLayout::Horizontal(children, _) | PaneLayout::Vertical(children, _) => {
                    for (index, child) in children.iter().enumerate() {
                        path.push(index);
                        if visit(child, path, predicate) {
                            return true;
                        }
                        path.pop();
                    }
                    false
                }
            }
        }

        let mut path = Vec::new();
        visit(self, &mut path, &predicate).then_some(path)
    }

    pub(crate) fn focused_leaf(&self, path: &[usize]) -> Option<&PaneLeaf> {
        match self {
            Self::Leaf(leaf) if path.is_empty() && !matches!(leaf, PaneLeaf::Empty) => Some(leaf),
            Self::Horizontal(children, _) | Self::Vertical(children, _) => {
                let (&first, rest) = path.split_first()?;
                children.get(first)?.focused_leaf(rest)
            }
            _ => None,
        }
    }

    pub(crate) fn replace_at(&mut self, path: &[usize], replacement: PaneLayout) -> bool {
        match (self, path) {
            (this @ Self::Leaf(_), []) => {
                *this = replacement;
                true
            }
            (Self::Horizontal(children, _) | Self::Vertical(children, _), [first, rest @ ..]) => {
                children
                    .get_mut(*first)
                    .is_some_and(|child| child.replace_at(rest, replacement))
            }
            _ => false,
        }
    }

    pub(crate) fn insert_right(&mut self, path: &[usize], leaf: PaneLeaf) -> Option<Vec<usize>> {
        let target = self.layout_mut_at(path)?;
        if !matches!(target, Self::Leaf(_)) {
            return None;
        }
        let previous = std::mem::replace(target, Self::empty());
        *target = Self::Vertical(vec![previous, Self::Leaf(leaf)], 0.5);
        let mut inserted_path = path.to_vec();
        inserted_path.push(1);
        Some(inserted_path)
    }

    fn layout_mut_at(&mut self, path: &[usize]) -> Option<&mut PaneLayout> {
        match (self, path) {
            (this, []) => Some(this),
            (Self::Horizontal(children, _) | Self::Vertical(children, _), [first, rest @ ..]) => {
                children.get_mut(*first)?.layout_mut_at(rest)
            }
            _ => None,
        }
    }

    pub(crate) fn remove_terminal(&mut self, id: &str) -> bool {
        self.remove_matching(|leaf| matches!(leaf, PaneLeaf::Terminal(value) if value == id))
    }

    pub(crate) fn remove_terminal_and_focus(&mut self, id: &str) -> Option<Vec<usize>> {
        self.remove_leaf_and_focus(|leaf| matches!(leaf, PaneLeaf::Terminal(value) if value == id))
    }

    pub(crate) fn remove_tab(&mut self, id: &str) -> bool {
        self.remove_terminal(id)
    }

    pub(crate) fn remove_document_and_focus(&mut self, id: &str) -> Option<Vec<usize>> {
        self.remove_leaf_and_focus(|leaf| matches!(leaf, PaneLeaf::Document(value) if value == id))
    }

    fn remove_leaf_and_focus(
        &mut self,
        predicate: impl Copy + Fn(&PaneLeaf) -> bool,
    ) -> Option<Vec<usize>> {
        let leaves = self.leaves();
        let target_index = leaves.iter().position(|leaf| predicate(leaf))?;
        let neighbor = if target_index > 0 {
            Some((*leaves[target_index - 1]).clone())
        } else {
            leaves.get(1).map(|leaf| (*leaf).clone())
        };

        self.remove_matching(predicate);
        neighbor.and_then(|neighbor| self.path_to_leaf(|leaf| leaf == &neighbor))
    }

    fn remove_matching(&mut self, predicate: impl Copy + Fn(&PaneLeaf) -> bool) -> bool {
        match self {
            Self::Leaf(leaf) if predicate(leaf) => {
                *leaf = PaneLeaf::Empty;
                true
            }
            Self::Leaf(_) => false,
            Self::Horizontal(children, _) | Self::Vertical(children, _) => {
                let removed = children
                    .iter_mut()
                    .any(|child| child.remove_matching(predicate));
                if removed {
                    self.normalize();
                }
                removed
            }
        }
    }

    fn normalize(&mut self) {
        let (Self::Horizontal(children, _) | Self::Vertical(children, _)) = self else {
            return;
        };
        children.retain(|child| child.leaf_count() > 0);
        if children.is_empty() {
            *self = Self::empty();
        } else if children.len() == 1 {
            *self = children.pop().expect("one child remains");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PaneLayout, PaneLeaf};

    #[test]
    fn mixed_layout_keeps_terminal_and_document_ids_separate() {
        let layout = PaneLayout::Vertical(
            vec![
                PaneLayout::Leaf(PaneLeaf::Terminal("ssh".into())),
                PaneLayout::Leaf(PaneLeaf::Document("doc".into())),
            ],
            0.5,
        );

        assert_eq!(layout.terminal_ids(), vec!["ssh"]);
        assert_eq!(layout.document_ids(), vec!["doc"]);
        assert!(matches!(
            layout.focused_leaf(&[1]),
            Some(PaneLeaf::Document(id)) if id == "doc"
        ));
    }

    #[test]
    fn inserts_document_to_the_right_and_returns_its_focus_path() {
        let mut layout = PaneLayout::Leaf(PaneLeaf::Terminal("ssh".into()));

        let path = layout
            .insert_right(&[], PaneLeaf::Document("doc".into()))
            .expect("insert document");

        assert_eq!(path, vec![1]);
        assert!(matches!(
            layout.focused_leaf(&path),
            Some(PaneLeaf::Document(id)) if id == "doc"
        ));
    }

    #[test]
    fn removing_a_document_compresses_the_tree() {
        let mut layout = PaneLayout::Vertical(
            vec![
                PaneLayout::Leaf(PaneLeaf::Terminal("ssh".into())),
                PaneLayout::Leaf(PaneLeaf::Document("doc".into())),
            ],
            0.5,
        );

        assert_eq!(layout.remove_document_and_focus("doc"), Some(vec![]));
        assert_eq!(layout, PaneLayout::Leaf(PaneLeaf::Terminal("ssh".into())));
    }

    #[test]
    fn removing_a_middle_leaf_focuses_the_previous_neighbor() {
        let mut layout = PaneLayout::Vertical(
            vec![
                PaneLayout::Leaf(PaneLeaf::Terminal("left".into())),
                PaneLayout::Leaf(PaneLeaf::Document("doc".into())),
                PaneLayout::Leaf(PaneLeaf::Terminal("right".into())),
            ],
            0.5,
        );

        let focus_path = layout
            .remove_document_and_focus("doc")
            .expect("adjacent pane remains");

        assert!(matches!(
            layout.focused_leaf(&focus_path),
            Some(PaneLeaf::Terminal(id)) if id == "left"
        ));
    }

    #[test]
    fn removing_the_first_leaf_focuses_the_next_neighbor() {
        let mut layout = PaneLayout::Horizontal(
            vec![
                PaneLayout::Leaf(PaneLeaf::Terminal("first".into())),
                PaneLayout::Leaf(PaneLeaf::Document("doc".into())),
            ],
            0.5,
        );

        let focus_path = layout
            .remove_terminal_and_focus("first")
            .expect("adjacent pane remains");

        assert!(matches!(
            layout.focused_leaf(&focus_path),
            Some(PaneLeaf::Document(id)) if id == "doc"
        ));
    }

    #[test]
    fn removing_the_only_leaf_still_removes_it() {
        let mut layout = PaneLayout::Leaf(PaneLeaf::Document("doc".into()));

        assert_eq!(layout.remove_document_and_focus("doc"), None);
        assert_eq!(layout.leaf_count(), 0);
    }

    #[test]
    fn empty_leaf_is_not_a_terminal_or_document() {
        let layout = PaneLayout::Leaf(PaneLeaf::Empty);

        assert!(layout.terminal_ids().is_empty());
        assert!(layout.document_ids().is_empty());
        assert_eq!(layout.leaf_count(), 0);
    }
}
