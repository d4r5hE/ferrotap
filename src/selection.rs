//! G1 multi-selection in the session grid (click, Ctrl+click toggle, Shift+click range, select all) and
//! "select related sessions" (duplicates, same host, parent / child requests by Referer).
use std::collections::BTreeSet;

use crate::model::SessionRecord;

/// The grid's selected sessions. `GridApp::model.selected` stays the focused session (the inspector shows it);
/// this holds every selected id, including the focused one.
#[derive(Default, Debug, Clone)]
pub struct Selection {
    pub ids: BTreeSet<usize>,
    /// Where a Shift+click range starts (the last plain or Ctrl click).
    pub anchor: Option<usize>,
}

impl Selection {
    /// A click on session `id`. `visible` is the grid's row order (ids).
    pub fn click(&mut self, id: usize, visible: &[usize], ctrl: bool, shift: bool) {
        if shift {
            let anchor = self.anchor.unwrap_or(id);
            match (visible.iter().position(|&x| x == anchor), visible.iter().position(|&x| x == id)) {
                (Some(a), Some(b)) => {
                    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                    if !ctrl {
                        self.ids.clear();
                    }
                    self.ids.extend(visible[lo..=hi].iter().copied());
                }
                _ => self.only(id),
            }
            return; // the anchor stays put, as in file managers
        }
        if ctrl {
            if !self.ids.remove(&id) {
                self.ids.insert(id);
            }
            self.anchor = Some(id);
            return;
        }
        self.only(id);
    }

    /// Select just this session.
    pub fn only(&mut self, id: usize) {
        self.ids.clear();
        self.ids.insert(id);
        self.anchor = Some(id);
    }

    pub fn set(&mut self, ids: impl IntoIterator<Item = usize>) {
        self.ids = ids.into_iter().collect();
        self.anchor = self.ids.iter().next().copied();
    }

    pub fn contains(&self, id: usize) -> bool {
        self.ids.contains(&id)
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn clear(&mut self) {
        self.ids.clear();
        self.anchor = None;
    }

    /// Drop ids whose sessions were removed.
    pub fn retain_existing(&mut self, rows: &[SessionRecord]) {
        let live: BTreeSet<usize> = rows.iter().map(|r| r.id).collect();
        self.ids.retain(|id| live.contains(id));
        if self.anchor.is_some_and(|a| !live.contains(&a)) {
            self.anchor = None;
        }
    }

    pub fn to_vec(&self) -> Vec<usize> {
        self.ids.iter().copied().collect()
    }
}

fn header<'a>(s: &'a SessionRecord, name: &str) -> Option<&'a str> {
    s.request_headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// The URL without a #fragment, for comparing a Referer with a request URL.
fn norm(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

/// Sessions repeating this request: same method, URL and request body (including the session itself).
pub fn duplicates_of(rows: &[SessionRecord], id: usize) -> Vec<usize> {
    let Some(s) = rows.iter().find(|r| r.id == id) else { return vec![] };
    rows.iter()
        .filter(|r| r.method == s.method && r.url == s.url && r.request_body == s.request_body)
        .map(|r| r.id)
        .collect()
}

/// Sessions to the same host (including the session itself).
pub fn same_host(rows: &[SessionRecord], id: usize) -> Vec<usize> {
    let Some(s) = rows.iter().find(|r| r.id == id) else { return vec![] };
    rows.iter().filter(|r| r.host() == s.host()).map(|r| r.id).collect()
}

/// Child requests: sessions whose Referer is this session's URL (e.g. the scripts and images a page loaded).
pub fn children_of(rows: &[SessionRecord], id: usize) -> Vec<usize> {
    let Some(s) = rows.iter().find(|r| r.id == id) else { return vec![] };
    let url = norm(&s.url);
    rows.iter()
        .filter(|r| r.id != id && header(r, "referer").is_some_and(|re| norm(re) == url))
        .map(|r| r.id)
        .collect()
}

/// The parent request: the latest earlier session whose URL is this session's Referer.
pub fn parent_of(rows: &[SessionRecord], id: usize) -> Option<usize> {
    let s = rows.iter().find(|r| r.id == id)?;
    let referer = norm(header(s, "referer")?);
    rows.iter().filter(|r| r.id < id && norm(&r.url) == referer).map(|r| r.id).max()
}
