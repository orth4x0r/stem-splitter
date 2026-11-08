//! Batch queue: several links / files split one after another. Pure state; `app.rs` drives it
//! (starts the next job, feeds results back) and `views/queue.rs` draws it.

use std::path::PathBuf;

use crate::pipeline::InputSource;
use crate::settings::HistoryEntry;

#[derive(Debug, Clone)]
pub enum ItemStatus {
    /// Waiting for its turn.
    Pending,
    /// A playlist URL being listed by yt-dlp; replaced by its entries when that finishes.
    Expanding,
    /// Being split right now by job `job_id`.
    Running {
        job_id: u64,
    },
    Done(Box<HistoryEntry>),
    Failed(String),
    Cancelled,
}

impl ItemStatus {
    pub fn is_finished(&self) -> bool {
        matches!(
            self,
            ItemStatus::Done(_) | ItemStatus::Failed(_) | ItemStatus::Cancelled
        )
    }
}

#[derive(Debug, Clone)]
pub struct QueueItem {
    pub id: u64,
    pub source: InputSource,
    pub title: String,
    pub status: ItemStatus,
}

#[derive(Debug, Default)]
pub struct Queue {
    pub items: Vec<QueueItem>,
    next_id: u64,
}

impl Queue {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Add a source; playlist URLs start as [`ItemStatus::Expanding`]. Returns the new id.
    pub fn push(&mut self, source: InputSource, expanding: bool) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.items.push(QueueItem {
            id,
            title: source.provisional_title(),
            source,
            status: if expanding {
                ItemStatus::Expanding
            } else {
                ItemStatus::Pending
            },
        });
        id
    }

    pub fn get_mut(&mut self, id: u64) -> Option<&mut QueueItem> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    fn index(&self, id: u64) -> Option<usize> {
        self.items.iter().position(|i| i.id == id)
    }

    /// Replace a playlist placeholder with its entries (title, url), in place.
    pub fn expand(&mut self, id: u64, entries: Vec<(Option<String>, String)>) {
        let Some(at) = self.index(id) else { return };
        self.items.remove(at);
        for (k, (title, url)) in entries.into_iter().enumerate() {
            self.next_id += 1;
            let source = InputSource::Url(url);
            self.items.insert(
                at + k,
                QueueItem {
                    id: self.next_id,
                    title: title.unwrap_or_else(|| source.provisional_title()),
                    source,
                    status: ItemStatus::Pending,
                },
            );
        }
    }

    /// The first item waiting for its turn (playlists still being listed are skipped).
    pub fn next_pending(&self) -> Option<&QueueItem> {
        self.items
            .iter()
            .find(|i| matches!(i.status, ItemStatus::Pending))
    }

    pub fn running(&self) -> Option<&QueueItem> {
        self.items
            .iter()
            .find(|i| matches!(i.status, ItemStatus::Running { .. }))
    }

    /// The item that job `job_id` is working on.
    pub fn by_job(&mut self, job_id: u64) -> Option<&mut QueueItem> {
        self.items
            .iter_mut()
            .find(|i| matches!(i.status, ItemStatus::Running { job_id: j } if j == job_id))
    }

    pub fn remove(&mut self, id: u64) {
        self.items.retain(|i| i.id != id);
    }

    /// Move an item one place up (`-1`) or down (`+1`).
    pub fn shift(&mut self, id: u64, delta: isize) {
        let Some(i) = self.index(id) else { return };
        let j = i as isize + delta;
        if j >= 0 && (j as usize) < self.items.len() {
            self.items.swap(i, j as usize);
        }
    }

    pub fn clear_finished(&mut self) {
        self.items.retain(|i| !i.status.is_finished());
    }

    /// (finished, total)
    pub fn progress(&self) -> (usize, usize) {
        let done = self.items.iter().filter(|i| i.status.is_finished()).count();
        (done, self.items.len())
    }

    pub fn has_work(&self) -> bool {
        self.items.iter().any(|i| {
            matches!(
                i.status,
                ItemStatus::Pending | ItemStatus::Expanding | ItemStatus::Running { .. }
            )
        })
    }
}

/// Split pasted text into sources: one existing file per line (paths may contain spaces), or
/// any number of links separated by whitespace / newlines (a single-line text box turns pasted
/// newlines into spaces). Bare domains get `https://`. Returns sources and the rejected bits.
pub fn parse_input(text: &str) -> (Vec<InputSource>, Vec<String>) {
    let mut out = Vec::new();
    let mut bad = Vec::new();
    for line in text.lines() {
        let line = line.trim().trim_matches('"').trim();
        if line.is_empty() {
            continue;
        }
        let path = PathBuf::from(line);
        if path.is_file() {
            out.push(InputSource::File(path));
            continue;
        }
        for tok in line.split_whitespace() {
            let tok = tok.trim_matches(['"', '\'', ',', ';', '<', '>']);
            if tok.is_empty() {
                continue;
            }
            let p = PathBuf::from(tok);
            if p.is_file() {
                out.push(InputSource::File(p));
            } else if tok.starts_with("http://") || tok.starts_with("https://") {
                out.push(InputSource::Url(tok.to_owned()));
            } else if tok.contains('.') && !tok.contains("://") && !tok.starts_with('.') {
                out.push(InputSource::Url(format!("https://{tok}")));
            } else {
                bad.push(tok.to_owned());
            }
        }
    }
    (out, bad)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn urls(srcs: &[InputSource]) -> Vec<String> {
        srcs.iter().map(InputSource::display).collect()
    }

    #[test]
    fn parses_many_links() {
        let (s, bad) = parse_input(
            "https://youtu.be/a\n  https://soundcloud.com/x/y  https://x.com/z/status/1\n\nyoutube.com/watch?v=b",
        );
        assert_eq!(
            urls(&s),
            [
                "https://youtu.be/a",
                "https://soundcloud.com/x/y",
                "https://x.com/z/status/1",
                "https://youtube.com/watch?v=b"
            ]
        );
        assert!(bad.is_empty());
        let (s, bad) = parse_input("hello https://a.com/x");
        assert_eq!(urls(&s), ["https://a.com/x"]);
        assert_eq!(bad, ["hello"]);
    }

    #[test]
    fn file_lines_may_contain_spaces() {
        let dir = std::env::temp_dir().join(format!("stem-q-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("my song.wav");
        std::fs::write(&f, b"x").unwrap();
        let (s, bad) = parse_input(&format!("\"{}\"\nhttps://a.com/b", f.display()));
        assert!(bad.is_empty());
        assert!(matches!(&s[0], InputSource::File(p) if p == &f));
        assert_eq!(s.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn queue_order_and_states() {
        let mut q = Queue::default();
        let a = q.push(InputSource::Url("https://a.com/1".into()), false);
        let p = q.push(InputSource::Url("https://y.com/playlist".into()), true);
        let c = q.push(InputSource::Url("https://c.com/3".into()), false);
        assert_eq!(q.next_pending().map(|i| i.id), Some(a));
        q.get_mut(a).unwrap().status = ItemStatus::Running { job_id: 7 };
        assert_eq!(q.next_pending().map(|i| i.id), Some(c));
        assert_eq!(q.by_job(7).map(|i| i.id), Some(a));
        q.expand(
            p,
            vec![
                (Some("One".into()), "https://y.com/1".into()),
                (None, "https://y.com/2".into()),
            ],
        );
        let titles: Vec<_> = q.items.iter().map(|i| i.title.clone()).collect();
        assert_eq!(titles[1], "One");
        assert_eq!(q.items.len(), 4);
        assert_eq!(q.items[3].id, c);
        // Playlist entries now come before c.
        assert_eq!(q.next_pending().map(|i| i.title.as_str()), Some("One"));
        q.shift(c, -1);
        assert_eq!(q.items[2].id, c);
        q.shift(q.items[0].id, -1); // no-op at the top
        assert_eq!(q.items[0].id, a);
        q.get_mut(a).unwrap().status = ItemStatus::Failed("x".into());
        assert_eq!(q.progress(), (1, 4));
        q.clear_finished();
        assert_eq!(q.items.len(), 3);
        assert!(q.has_work());
        q.remove(c);
        assert_eq!(q.items.len(), 2);
    }
}
