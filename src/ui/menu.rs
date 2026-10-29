//! The right-click menu on a session's card or a group's header: what can
//! be done to that session or group, from one place. Drawn in chrome.rs.

use super::{App, Rect};
use crate::claude::Status;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    Open,
    Join,
    BreakOut,
    SplitSide,
    SplitStacked,
    Fork,
    Pick,
    NewGroup,
    MoveTo(usize),
    Ungroup,
    Rename(usize),
    Toggle(usize),
    Disband(usize),
    Detach,
}

pub struct Item {
    pub label: String,
    /// `None`: a divider.
    pub act: Option<Act>,
}

pub struct Menu {
    /// The session (pane id) or group the menu is for.
    pub pane: Option<u64>,
    pub title: String,
    pub items: Vec<Item>,
    pub at: (f32, f32),
    pub hover: Option<usize>,
}

impl Menu {
    pub fn row_h(scale: f32) -> f32 {
        (26.0 * scale).round()
    }

    pub fn width(scale: f32) -> f32 {
        (236.0 * scale).round()
    }

    /// The menu's rect and each row's, kept on screen.
    pub fn rects(&self, scale: f32, screen: (f32, f32)) -> (Rect, Vec<Rect>) {
        let (rh, dh, pad, head) = (Menu::row_h(scale), (9.0 * scale).round(), (6.0 * scale).round(), (28.0 * scale).round());
        let h = head + pad * 2.0 + self.items.iter().map(|i| if i.act.is_some() { rh } else { dh }).sum::<f32>();
        let w = Menu::width(scale);
        let x = self.at.0.min(screen.0 - w - 8.0).max(8.0);
        let y = self.at.1.min(screen.1 - h - 8.0).max(8.0);
        let mut rows = Vec::new();
        let mut ry = y + head + pad;
        for i in &self.items {
            let ih = if i.act.is_some() { rh } else { dh };
            rows.push(Rect { x: x + pad, y: ry, w: w - 2.0 * pad, h: ih });
            ry += ih;
        }
        (Rect { x, y, w, h }, rows)
    }
}

fn item(label: impl Into<String>, act: Act) -> Item {
    Item { label: label.into(), act: Some(act) }
}

fn divider() -> Item {
    Item { label: String::new(), act: None }
}

impl App {
    /// Right-click: a menu for the card or group header under (x, y).
    pub fn open_menu(&mut self, x: f32, y: f32) -> bool {
        let menu = if let Some(g) = self.head_at(x, y) {
            self.group_menu(g)
        } else if let Some(i) = self.card_at(x, y) {
            self.card_menu(i)
        } else {
            return false;
        };
        self.menu = Some(Menu { at: (x, y), ..menu });
        true
    }

    /// Snapshots: open the menu for session `N` (1-based) or group `gN`,
    /// with its second row lit.
    pub fn menu_target(&mut self, which: &str) {
        let l = self.layout();
        let r = match which.strip_prefix('g') {
            Some(g) => g.parse::<usize>().ok().and_then(|g| l.heads.iter().find(|h| h.0 + 1 == g)).map(|h| h.2),
            None => which.parse::<usize>().ok().and_then(|n| l.cards.get(n.wrapping_sub(1)).copied()),
        };
        if let Some(r) = r {
            let (x, y) = (r.x + r.w * 0.6, r.y + r.h / 2.0);
            self.open_menu(x, y);
            self.menu_step(1);
            self.menu_step(1);
        }
    }

    /// Snapshots: find the first divider across the middle of the view and
    /// drag it `dx` px.
    pub fn drag_divider_by(&mut self, dx: f32) -> bool {
        let t = self.layout().term;
        let y = t.y + t.h / 2.0;
        let mut x = t.x;
        while x < t.x + t.w {
            if self.divider_at(x, y).is_some_and(|(_, v)| v) {
                self.divider_press(x, y);
                self.divider_drag(x + dx, y);
                self.divider_release();
                return true;
            }
            x += 1.0;
        }
        false
    }

    /// Snapshots: hold session `n`'s card (1-based) over group `g`'s header.
    pub fn drag_card_over(&mut self, n: usize, g: usize) {
        let l = self.layout();
        let (Some(c), Some(h)) = (l.cards.get(n.wrapping_sub(1)).copied(), l.heads.iter().find(|h| h.0 + 1 == g).map(|h| h.2)) else { return };
        self.card_press(n - 1, c.x + 40.0, c.y + 10.0);
        self.card_drag_to(h.x + 60.0, h.y + h.h / 2.0);
    }

    fn group_menu(&self, g: usize) -> Menu {
        let gr = &self.groups[g];
        let items = vec![
            item(if gr.collapsed { "Expand" } else { "Roll up" }, Act::Toggle(g)),
            item("Rename group…", Act::Rename(g)),
            item("Ungroup all", Act::Disband(g)),
        ];
        Menu { pane: None, title: gr.name.to_uppercase(), items, at: (0.0, 0.0), hover: None }
    }

    fn card_menu(&self, i: usize) -> Menu {
        let p = &self.panes[i];
        let a = self.panes.get(self.active);
        let tmux = p.tmux_pane.is_some();
        let mut items = Vec::new();
        if i != self.active {
            items.push(item("Open", Act::Open));
        }
        let joinable = a.is_some_and(|a| a.tmux_pane.is_some() && a.host == p.host && !(a.conn == p.conn && a.window == p.window));
        if tmux && i != self.active && joinable {
            items.push(item("Join beside current", Act::Join));
        }
        if tmux {
            items.push(item("Split side by side", Act::SplitSide));
            items.push(item("Split stacked", Act::SplitStacked));
        }
        if tmux && self.group(i).len() > 1 {
            items.push(item("Break out of split", Act::BreakOut));
        }
        if self.ui[i].status != Status::Shell {
            items.push(item("Fork Claude session…", Act::Fork));
        }
        items.push(divider());
        let picked = self.picked.contains(&p.id);
        items.push(item(if picked { "Unpick" } else { "Pick (for a group)" }, Act::Pick));
        let n = self.picked.len();
        items.push(item(if n > 1 { format!("New group from {n} picked…") } else { "New group…".to_string() }, Act::NewGroup));
        let mine = self.group_of(i);
        for (g, gr) in self.groups.iter().enumerate() {
            if Some(g) != mine {
                items.push(item(format!("Move to {}", gr.name), Act::MoveTo(g)));
            }
        }
        if let Some(g) = mine {
            items.push(item("Remove from group", Act::Ungroup));
            items.push(item("Rename group…", Act::Rename(g)));
        }
        items.push(divider());
        items.push(item("Detach (keeps running)", Act::Detach));
        let title = if p.title.is_empty() { p.subtitle.clone() } else { super::clean_title(&p.title) };
        Menu { pane: Some(p.id), title, items, at: (0.0, 0.0), hover: None }
    }

    /// The pointer moved: highlight the row under it. True when that changed.
    pub fn menu_hover(&mut self, x: f32, y: f32) -> bool {
        let (scale, screen) = (self.scale, (self.size.0 as f32, self.size.1 as f32));
        let Some(m) = self.menu.as_mut() else { return false };
        let (_, rows) = m.rects(scale, screen);
        let h = rows.iter().zip(&m.items).position(|(r, it)| it.act.is_some() && x >= r.x && x <= r.x + r.w && y >= r.y && y < r.y + r.h);
        let changed = h != m.hover;
        m.hover = h;
        changed
    }

    /// Up/down through the menu's actions.
    pub fn menu_step(&mut self, d: isize) {
        let Some(m) = self.menu.as_mut() else { return };
        let acts: Vec<usize> = (0..m.items.len()).filter(|&k| m.items[k].act.is_some()).collect();
        if acts.is_empty() {
            return;
        }
        let at = m.hover.and_then(|h| acts.iter().position(|&k| k == h));
        let next = match at {
            Some(a) => (a as isize + d).rem_euclid(acts.len() as isize) as usize,
            None if d > 0 => 0,
            None => acts.len() - 1,
        };
        m.hover = Some(acts[next]);
    }

    /// A click while the menu is open: run the row under it, or close.
    pub fn menu_click(&mut self, x: f32, y: f32) {
        self.menu_hover(x, y);
        self.menu_enter();
    }

    /// Run the highlighted row (closing the menu either way).
    pub fn menu_enter(&mut self) {
        let Some(m) = self.menu.take() else { return };
        let Some(act) = m.hover.and_then(|h| m.items.get(h)).and_then(|i| i.act) else { return };
        let i = m.pane.and_then(|id| self.panes.iter().position(|p| p.id == id));
        match (act, i) {
            (Act::Toggle(g), _) => self.toggle_group(g),
            (Act::Rename(g), _) => self.rename_group(Some(g)),
            (Act::Disband(g), _) => {
                if g < self.groups.len() {
                    self.groups.remove(g);
                    self.settle_groups();
                }
            }
            (Act::Open, Some(i)) => self.select(i),
            (Act::Join, Some(i)) => self.join(i),
            (Act::BreakOut, Some(i)) => {
                self.select(i);
                self.break_out();
            }
            (Act::SplitSide | Act::SplitStacked, Some(i)) => {
                self.select(i);
                self.split(act == Act::SplitSide);
            }
            // Claude Code's /fork carries the conversation into a new
            // background session; it wants a prompt, so leave that to you.
            (Act::Fork, Some(i)) => {
                self.select(i);
                self.panes[i].write(b"/fork ".to_vec());
                self.toast("FORK // TYPE ITS FIRST PROMPT, THEN ↵");
            }
            (Act::Pick, Some(i)) => self.pick(i),
            (Act::NewGroup, Some(i)) => {
                self.picked.insert(self.panes[i].id);
                self.group_picked();
            }
            (Act::MoveTo(g), Some(i)) => self.move_to_group(i, g),
            (Act::Ungroup, Some(i)) => {
                self.picked.clear();
                self.picked.insert(self.panes[i].id);
                self.ungroup_picked();
            }
            (Act::Detach, Some(i)) => {
                self.select(i);
                self.close_active();
            }
            _ => {}
        }
    }
}
