// Undo history of the editor. Only the newest state is kept whole; every older one is the
// difference to the state after it (the changed middle of the text), so a long session on a
// large note costs about as much as what was typed, like CodeMirror's change sets.

#[derive(Clone, Debug)]
enum Entry {
    Full { text: String, cursor: usize },
    /// The state = next state with chars [prefix, prefix + inserted) replaced by `removed`.
    Diff { prefix: usize, removed: String, inserted: usize, cursor: usize },
}

#[derive(Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
}

const MAX: usize = 500;

/// How `older` differs from `newer`: common prefix and suffix, in characters.
fn diff(older: &str, newer: &str, cursor: usize) -> Entry {
    let a: Vec<char> = older.chars().collect();
    let b: Vec<char> = newer.chars().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let max_suffix = a.len().min(b.len()) - prefix;
    let suffix = a.iter().rev().zip(b.iter().rev()).take(max_suffix).take_while(|(x, y)| x == y).count();
    Entry::Diff { prefix, removed: a[prefix..a.len() - suffix].iter().collect(), inserted: b.len() - suffix - prefix, cursor }
}

fn apply(entry: &Entry, newer: &str) -> (String, usize) {
    match entry {
        Entry::Full { text, cursor } => (text.clone(), *cursor),
        Entry::Diff { prefix, removed, inserted, cursor } => {
            let b: Vec<char> = newer.chars().collect();
            let p = (*prefix).min(b.len());
            let end = (p + inserted).min(b.len());
            let mut s: String = b[..p].iter().collect();
            s.push_str(removed);
            s.extend(&b[end..]);
            (s, *cursor)
        }
    }
}

impl History {
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    /// The state before a change: one undo step. `text` is the text right now.
    pub fn checkpoint(&mut self, text: &str, cursor: usize) {
        if let Some(Entry::Full { text: top, .. }) = self.undo.last() {
            if top == text {
                return; // nothing changed since the last step
            }
        }
        if let Some(Entry::Full { text: top, cursor: c }) = self.undo.pop() {
            self.undo.push(diff(&top, text, c));
        }
        self.undo.push(Entry::Full { text: text.to_string(), cursor });
        if self.undo.len() > MAX {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Goes back one step from `current`; None when there is nothing to undo.
    pub fn undo(&mut self, current: &str, cursor: usize) -> Option<(String, usize)> {
        loop {
            let top = self.undo.pop()?;
            let (text, c) = apply(&top, current);
            // The step below becomes the new whole state.
            if let Some(below) = self.undo.pop() {
                let (t2, c2) = apply(&below, &text);
                self.undo.push(Entry::Full { text: t2, cursor: c2 });
            }
            if text == current {
                // A checkpoint with no change after it: skip it.
                if self.undo.is_empty() {
                    self.undo.push(Entry::Full { text, cursor: c });
                    return None;
                }
                continue;
            }
            self.push_redo(current, cursor);
            return Some((text, c));
        }
    }

    fn push_redo(&mut self, current: &str, cursor: usize) {
        if let Some(Entry::Full { text: top, cursor: c }) = self.redo.pop() {
            self.redo.push(diff(&top, current, c));
        }
        self.redo.push(Entry::Full { text: current.to_string(), cursor });
    }

    pub fn redo(&mut self, current: &str, cursor: usize) -> Option<(String, usize)> {
        let top = self.redo.pop()?;
        let (text, c) = apply(&top, current);
        if let Some(below) = self.redo.pop() {
            let (t2, c2) = apply(&below, &text);
            self.redo.push(Entry::Full { text: t2, cursor: c2 });
        }
        // Back on the undo stack, without clearing the rest of the redo stack.
        if let Some(Entry::Full { text: t, cursor: c0 }) = self.undo.pop() {
            self.undo.push(diff(&t, current, c0));
        }
        self.undo.push(Entry::Full { text: current.to_string(), cursor });
        Some((text, c))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_and_redo_walk_the_history() {
        let mut h = History::default();
        h.checkpoint("a", 0);
        h.checkpoint("ab", 1);
        h.checkpoint("abc", 2);
        let cur = "abcd";
        let (t, _) = h.undo(cur, 3).unwrap_or_default();
        assert_eq!(t, "abc");
        let (t, _) = h.undo(&t, 2).unwrap_or_default();
        assert_eq!(t, "ab");
        let (t, _) = h.undo(&t, 1).unwrap_or_default();
        assert_eq!(t, "a");
        assert!(h.undo(&t, 0).is_none());
        let (t, _) = h.redo(&t, 0).unwrap_or_default();
        assert_eq!(t, "ab");
        let (t, _) = h.redo(&t, 0).unwrap_or_default();
        assert_eq!(t, "abc");
        let (t, _) = h.redo(&t, 0).unwrap_or_default();
        assert_eq!(t, "abcd");
        assert!(h.redo(&t, 0).is_none());
        let (t, _) = h.undo(&t, 0).unwrap_or_default();
        assert_eq!(t, "abc");
    }

    #[test]
    fn a_new_change_drops_the_redo_steps() {
        let mut h = History::default();
        h.checkpoint("x", 0);
        let (t, _) = h.undo("xy", 1).unwrap_or_default();
        assert_eq!(t, "x");
        h.checkpoint(&t, 0);
        assert!(h.redo("xz", 0).is_none());
        let (t, _) = h.undo("xz", 1).unwrap_or_default();
        assert_eq!(t, "x");
    }
}
