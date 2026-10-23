//! Pure tab/split layout and focus model; IDs never change when panes move.
pub type PaneId = u64;
pub const MAX_TABS: usize = 8;
pub const MAX_PANES: usize = 32;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}
impl Rect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}
#[derive(Clone, Copy, Debug)]
pub enum Axis {
    Columns,
    Rows,
}
#[derive(Clone, Debug)]
enum Node {
    Pane(PaneId),
    Split {
        axis: Axis,
        first: Box<Node>,
        second: Box<Node>,
    },
}
impl Node {
    fn ids(&self, out: &mut Vec<PaneId>) {
        match self {
            Self::Pane(id) => out.push(*id),
            Self::Split { first, second, .. } => {
                first.ids(out);
                second.ids(out);
            }
        }
    }
    fn split(&mut self, target: PaneId, new: PaneId, axis: Axis) -> bool {
        match self {
            Self::Pane(id) if *id == target => {
                *self = Self::Split {
                    axis,
                    first: Box::new(Self::Pane(target)),
                    second: Box::new(Self::Pane(new)),
                };
                true
            }
            Self::Split { first, second, .. } => {
                first.split(target, new, axis) || second.split(target, new, axis)
            }
            _ => false,
        }
    }
    fn remove(self, target: PaneId) -> Option<Self> {
        match self {
            Self::Pane(id) => (id != target).then_some(Self::Pane(id)),
            Self::Split {
                axis,
                first,
                second,
            } => match (first.remove(target), second.remove(target)) {
                (Some(first), Some(second)) => Some(Self::Split {
                    axis,
                    first: Box::new(first),
                    second: Box::new(second),
                }),
                (first, second) => first.or(second),
            },
        }
    }
    fn layout(&self, rect: Rect, gap: f32, out: &mut Vec<(PaneId, Rect)>) {
        match self {
            Self::Pane(id) => out.push((*id, rect)),
            Self::Split {
                axis,
                first,
                second,
            } => {
                let (a, b) = match axis {
                    Axis::Columns => {
                        let gap = gap.min(rect.width);
                        let width = ((rect.width - gap) / 2.0).floor();
                        (
                            Rect { width, ..rect },
                            Rect {
                                x: rect.x + width + gap,
                                width: rect.width - width - gap,
                                ..rect
                            },
                        )
                    }
                    Axis::Rows => {
                        let gap = gap.min(rect.height);
                        let height = ((rect.height - gap) / 2.0).floor();
                        (
                            Rect { height, ..rect },
                            Rect {
                                y: rect.y + height + gap,
                                height: rect.height - height - gap,
                                ..rect
                            },
                        )
                    }
                };
                first.layout(a, gap, out);
                second.layout(b, gap, out);
            }
        }
    }
}
#[derive(Clone, Debug)]
pub struct Tab {
    root: Node,
    pub active: PaneId,
}
impl Tab {
    pub fn ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        self.root.ids(&mut ids);
        ids
    }
}
#[derive(Clone, Debug)]
pub struct Workspace {
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
}
impl Workspace {
    pub fn new(id: PaneId) -> Self {
        Self {
            tabs: vec![Tab {
                root: Node::Pane(id),
                active: id,
            }],
            active_tab: 0,
        }
    }
    pub fn active(&self) -> PaneId {
        self.tabs[self.active_tab].active
    }
    pub fn count(&self) -> usize {
        self.tabs.iter().map(|tab| tab.ids().len()).sum()
    }
    pub fn add_tab(&mut self, id: PaneId) {
        self.tabs.push(Tab {
            root: Node::Pane(id),
            active: id,
        });
        self.active_tab = self.tabs.len() - 1;
    }
    pub fn split(&mut self, id: PaneId, axis: Axis) {
        let tab = &mut self.tabs[self.active_tab];
        tab.root.split(tab.active, id, axis);
        tab.active = id;
    }
    pub fn focus(&mut self, id: PaneId) -> bool {
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if tab.ids().contains(&id) {
                tab.active = id;
                self.active_tab = index;
                return true;
            }
        }
        false
    }
    pub fn cycle_tab(&mut self, forward: bool) {
        let n = self.tabs.len();
        self.active_tab = (self.active_tab + if forward { 1 } else { n - 1 }) % n;
    }
    pub fn cycle_pane(&mut self, forward: bool) {
        let tab = &mut self.tabs[self.active_tab];
        let ids = tab.ids();
        let index = ids.iter().position(|id| *id == tab.active).unwrap_or(0);
        tab.active = ids[(index + if forward { 1 } else { ids.len() - 1 }) % ids.len()];
    }
    pub fn close(&mut self, id: PaneId) {
        let Some(index) = self.tabs.iter().position(|tab| tab.ids().contains(&id)) else {
            return;
        };
        let old = self.tabs.remove(index);
        if let Some(root) = old.root.remove(id) {
            let mut ids = Vec::new();
            root.ids(&mut ids);
            self.tabs.insert(
                index,
                Tab {
                    root,
                    active: if old.active == id { ids[0] } else { old.active },
                },
            );
        } else if index < self.active_tab {
            self.active_tab -= 1;
        }
        self.active_tab = self.active_tab.min(self.tabs.len().saturating_sub(1));
    }
    pub fn placements(&self, rect: Rect, gap: f32) -> Vec<(PaneId, Rect)> {
        let mut out = Vec::new();
        if let Some(tab) = self.tabs.get(self.active_tab) {
            tab.root.layout(rect, gap, &mut out);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn nested_close_preserves_other_panes_and_focus() {
        let mut w = Workspace::new(1);
        w.split(2, Axis::Columns);
        w.split(3, Axis::Rows);
        assert_eq!(w.tabs[0].ids(), [1, 2, 3]);
        w.close(2);
        assert_eq!(w.active(), 3);
        w.close(3);
        assert_eq!(w.active(), 1);
        w.add_tab(4);
        w.focus(1);
        w.close(4);
        assert_eq!(w.active(), 1);
        w.close(1);
        assert!(w.tabs.is_empty());
    }
    #[test]
    fn panes_partition_scaled_area_without_overlap() {
        let mut w = Workspace::new(1);
        w.split(2, Axis::Columns);
        w.split(3, Axis::Rows);
        for size in [1.0, 327.0, 1060.0] {
            let rect = Rect {
                x: 11.0,
                y: 32.0,
                width: size,
                height: size,
            };
            let p = w.placements(rect, 6.0);
            for (i, (_, a)) in p.iter().enumerate() {
                assert!(a.width >= 0.0 && a.height >= 0.0);
                assert!(
                    a.x + a.width <= rect.x + rect.width && a.y + a.height <= rect.y + rect.height
                );
                for (_, b) in &p[i + 1..] {
                    assert!(
                        a.x + a.width <= b.x
                            || b.x + b.width <= a.x
                            || a.y + a.height <= b.y
                            || b.y + b.height <= a.y
                    );
                }
            }
        }
    }
    #[test]
    fn cycling_tabs_restores_each_tabs_last_focused_pane() {
        let mut w = Workspace::new(1);
        w.split(2, Axis::Columns);
        w.add_tab(3);
        w.cycle_tab(false);
        assert_eq!(w.active(), 2);
        w.cycle_pane(false);
        assert_eq!(w.active(), 1);
        w.cycle_tab(true);
        assert_eq!(w.active(), 3);
    }
}
