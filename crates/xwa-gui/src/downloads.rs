//! Downloads from the Kiwix library: one at a time, the rest waiting their
//! turn. Each can be paused (a later start carries on from its unfinished
//! file), resumed, retried or taken off the list.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use xwa_fetch::{Book, size_text};

use crate::app::Job;

#[derive(Debug)]
pub enum Fetch {
    Waiting,
    /// Downloading. `run` tells this start's messages from those of an
    /// earlier one, which may still be stopping.
    Running {
        job: Job,
        run: u64,
    },
    Paused {
        fraction: f32,
    },
    Done(PathBuf),
    Failed(String),
}

#[derive(Debug)]
pub struct Download {
    pub book: Book,
    pub state: Fetch,
    /// The folder it is saved in, from its first start: a paused download
    /// carries on there.
    pub dir: Option<PathBuf>,
}

/// A fraction as a whole percentage, never rounded up to 100%.
fn percent(fraction: f32) -> String {
    format!("{:.0}%", (fraction * 100.0).floor())
}

impl Download {
    pub fn id(&self) -> &str {
        &self.book.id
    }

    pub fn fraction(&self) -> Option<f32> {
        match &self.state {
            Fetch::Running { job, .. } => job.fraction(),
            Fetch::Paused { fraction } => Some(*fraction),
            Fetch::Done(_) => Some(1.0),
            Fetch::Waiting | Fetch::Failed(_) => None,
        }
    }

    pub fn job(&self) -> Option<&Job> {
        match &self.state {
            Fetch::Running { job, .. } => Some(job),
            _ => None,
        }
    }

    /// The unfinished file it leaves while it runs, and when it stops.
    pub fn part(&self) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        Some(dir.join(format!("{}.part", self.book.file_name())))
    }

    /// Where it is, in a line under its title (its badge gives the state),
    /// what matters most first, as the end may be cut off.
    pub fn line(&self) -> String {
        let what = self.book.flavour_description();
        let size = size_text(self.book.size);
        match &self.state {
            Fetch::Waiting => format!("{size} · {what}"),
            Fetch::Running { job, .. } => job.label().to_string(),
            Fetch::Paused { fraction } => format!("{} of {size} · {what}", percent(*fraction)),
            Fetch::Done(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Fetch::Failed(why) => why.clone(),
        }
    }

    /// A word for its state, beside it in lists.
    pub fn badge(&self) -> String {
        match &self.state {
            Fetch::Waiting => "Waiting".into(),
            Fetch::Running { job, .. } => percent(job.fraction().unwrap_or(0.0)),
            Fetch::Paused { .. } => "Paused".into(),
            Fetch::Done(_) => "Done".into(),
            Fetch::Failed(_) => "Stopped".into(),
        }
    }
}

#[derive(Debug, Default)]
pub struct Queue {
    pub items: Vec<Download>,
    /// The unfinished files of downloads taken off the list while running,
    /// to delete once their workers have stopped: (run, file).
    leaving: Vec<(u64, PathBuf)>,
    runs: u64,
}

impl Queue {
    pub fn running(&self) -> Option<&Download> {
        self.items.iter().find(|d| d.job().is_some())
    }

    pub fn is_running(&self) -> bool {
        self.running().is_some()
    }

    pub fn get(&self, id: &str) -> Option<&Download> {
        self.items.iter().find(|d| d.id() == id)
    }

    /// The download a worker's message is about, if it is still running.
    pub fn by_run(&mut self, run: u64) -> Option<&mut Download> {
        self.items
            .iter_mut()
            .find(|d| matches!(d.state, Fetch::Running { run: r, .. } if r == run))
    }

    /// Adds a book to wait its turn; a book already on the list that
    /// stopped or was paused waits again instead.
    pub fn add(&mut self, book: Book) {
        match self.items.iter_mut().find(|d| d.id() == book.id) {
            Some(d) => {
                if matches!(d.state, Fetch::Paused { .. } | Fetch::Failed(_)) {
                    d.state = Fetch::Waiting;
                }
            }
            None => self.items.push(Download {
                book,
                state: Fetch::Waiting,
                dir: None,
            }),
        }
    }

    /// Starts the next waiting download with `job`, if nothing is
    /// downloading. Gives the worker what it needs: the run, the link and
    /// the folder (`dir` for a first start).
    pub fn start_next(&mut self, job: Job, dir: PathBuf) -> Option<(u64, String, PathBuf)> {
        if self.is_running() {
            return None;
        }
        let d = self
            .items
            .iter_mut()
            .find(|d| matches!(d.state, Fetch::Waiting))?;
        self.runs += 1;
        let dir = d.dir.get_or_insert(dir).clone();
        d.state = Fetch::Running {
            job,
            run: self.runs,
        };
        Some((self.runs, d.book.url.clone(), dir))
    }

    /// Takes a download off the list, with its unfinished file: a running
    /// one stops, and its file goes once it has. A finished download's file
    /// stays where it is.
    pub fn remove(&mut self, id: &str) {
        let Some(i) = self.items.iter().position(|d| d.id() == id) else {
            return;
        };
        let mut d = self.items.remove(i);
        let part = d.part();
        match &mut d.state {
            Fetch::Running { job, run } => {
                job.cancel();
                if let Some(part) = part {
                    self.leaving.push((*run, part));
                }
            }
            Fetch::Done(_) => {}
            _ => {
                if let Some(part) = part {
                    let _ = std::fs::remove_file(part);
                }
            }
        }
    }

    /// A worker has stopped. If its download was taken off the list, its
    /// unfinished file goes, unless the book is back and using it.
    pub fn stopped(&mut self, run: u64) {
        let Some(i) = self.leaving.iter().position(|(r, _)| *r == run) else {
            return;
        };
        let (_, part) = self.leaving.remove(i);
        if !self.items.iter().any(|d| d.part().as_ref() == Some(&part)) {
            let _ = std::fs::remove_file(part);
        }
    }

    /// What the list holds, in a line for the home screen.
    pub fn summary(&self) -> Option<String> {
        let count = |f: fn(&Fetch) -> bool| self.items.iter().filter(|d| f(&d.state)).count();
        let mut parts = Vec::new();
        if let Some(d) = self.running() {
            parts.push(format!(
                "{} {}",
                d.book.title,
                percent(d.fraction().unwrap_or(0.0))
            ));
        }
        for (n, what) in [
            (count(|s| matches!(s, Fetch::Waiting)), "waiting"),
            (count(|s| matches!(s, Fetch::Paused { .. })), "paused"),
            (count(|s| matches!(s, Fetch::Failed(_))), "stopped"),
            (count(|s| matches!(s, Fetch::Done(_))), "downloaded"),
        ] {
            if n > 0 {
                parts.push(format!("{n} {what}"));
            }
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

/// How fast bytes arrive, over the last few seconds.
#[derive(Default)]
pub struct Rate {
    samples: std::collections::VecDeque<(Instant, u64)>,
    speed: Option<f64>,
}

impl Rate {
    const WINDOW: Duration = Duration::from_secs(5);
    /// Workers report every few kilobytes; a sample this often is plenty.
    const STEP: Duration = Duration::from_millis(100);

    /// Records `done` bytes now, and gives the speed in bytes a second once
    /// there is a second or more to go on.
    pub fn add(&mut self, done: u64) -> Option<f64> {
        let now = Instant::now();
        if self
            .samples
            .back()
            .is_some_and(|(t, _)| now.duration_since(*t) < Self::STEP)
        {
            return self.speed;
        }
        self.samples.push_back((now, done));
        while self
            .samples
            .front()
            .is_some_and(|(t, _)| now.duration_since(*t) > Self::WINDOW)
        {
            self.samples.pop_front();
        }
        let (t0, d0) = *self.samples.front()?;
        let span = now.duration_since(t0).as_secs_f64();
        self.speed = (span >= 1.0 && done >= d0).then(|| (done - d0) as f64 / span);
        self.speed
    }
}

/// A speed for people: 12 MB/s.
pub fn speed_text(bytes_per_second: f64) -> String {
    let mb = bytes_per_second / 1e6;
    if mb >= 10.0 {
        format!("{mb:.0} MB/s")
    } else if mb >= 0.1 {
        format!("{mb:.1} MB/s")
    } else {
        format!("{:.0} kB/s", bytes_per_second / 1e3)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(id: &str) -> Book {
        Book {
            id: id.into(),
            name: id.into(),
            title: id.to_uppercase(),
            summary: String::new(),
            language: "eng".into(),
            flavour: "maxi".into(),
            category: "wikipedia".into(),
            tags: String::new(),
            date: "2026-10".into(),
            article_count: 1,
            media_count: 0,
            size: 2_000_000_000,
            url: format!("https://example.org/{id}.zim.meta4"),
        }
    }

    #[test]
    fn books_wait_their_turn() {
        let mut q = Queue::default();
        q.add(book("a"));
        q.add(book("b"));
        q.add(book("a"));
        assert_eq!(q.items.len(), 2, "a book is listed once");
        let dir = PathBuf::from("/downloads");
        let (run, url, _) = q.start_next(Job::start().0, dir.clone()).unwrap();
        assert_eq!(url, "https://example.org/a.zim.meta4");
        assert!(
            q.start_next(Job::start().0, dir.clone()).is_none(),
            "one at a time"
        );
        assert_eq!(q.summary().as_deref(), Some("A 0% · 1 waiting"));
        assert_eq!(
            q.by_run(run).map(|d| d.id().to_string()).as_deref(),
            Some("a")
        );
        assert!(q.by_run(run + 1).is_none(), "another run's messages");

        q.items[0].state = Fetch::Paused { fraction: 0.4 };
        assert_eq!(
            q.items[0].line(),
            "40% of 2.0 GB · complete articles with pictures"
        );
        assert_eq!(q.items[0].part(), Some(dir.join("a.zim.part")));
        let (_, url, _) = q.start_next(Job::start().0, dir.clone()).unwrap();
        assert_eq!(url, "https://example.org/b.zim.meta4");
        // Asking again for a paused book puts it back in the queue.
        q.add(book("a"));
        assert!(matches!(q.items[0].state, Fetch::Waiting));
        assert_eq!(q.summary().as_deref(), Some("B 0% · 1 waiting"));
    }

    #[test]
    fn a_book_taken_off_the_list_takes_its_unfinished_file() {
        let dir = std::env::temp_dir().join(format!("xwa-queue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut q = Queue::default();
        q.add(book("a"));
        let (run, ..) = q.start_next(Job::start().0, dir.clone()).unwrap();
        let part = q.items[0].part().unwrap();
        std::fs::write(&part, b"half").unwrap();

        // Taken off while running: the file goes once the worker stops.
        q.remove("a");
        assert!(q.items.is_empty());
        assert!(part.exists(), "the worker may still be writing it");
        q.stopped(run);
        assert!(!part.exists());

        // Paused, then taken off: the file goes at once.
        q.add(book("a"));
        q.start_next(Job::start().0, dir.clone()).unwrap();
        std::fs::write(&part, b"half").unwrap();
        q.items[0].state = Fetch::Paused { fraction: 0.5 };
        q.remove("a");
        assert!(!part.exists());

        // Taken off and back again before the worker stopped: the new
        // start carries on from the file, so it stays.
        q.add(book("a"));
        let (old, ..) = q.start_next(Job::start().0, dir.clone()).unwrap();
        std::fs::write(&part, b"half").unwrap();
        q.remove("a");
        q.add(book("a"));
        let (new, ..) = q.start_next(Job::start().0, dir.clone()).unwrap();
        assert_ne!(old, new);
        q.stopped(old);
        assert!(part.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn speeds_read_naturally() {
        assert_eq!(speed_text(12_345_678.0), "12 MB/s");
        assert_eq!(speed_text(1_234_567.0), "1.2 MB/s");
        assert_eq!(speed_text(56_000.0), "56 kB/s");
        let mut rate = Rate::default();
        assert_eq!(rate.add(0), None, "too soon to tell");
    }
}
