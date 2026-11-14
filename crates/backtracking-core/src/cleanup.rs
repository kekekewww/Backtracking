//! Optional housekeeping for the drag-and-drop folder.
//!
//! A drag that another program accepts leaves its WAV file in that folder for good, because
//! the program may keep reading it. Nothing else ever removes those files, so the user can
//! ask for the old ones to be deleted. Only files the exporter named are candidates.

use std::io;
use std::path::{Path, PathBuf};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use crate::is_export_file_name;

/// The ages the Options form offers, in days. Off is the absence of a choice.
pub const AGE_CHOICES: [u16; 5] = [10, 20, 30, 60, 90];

/// How often a running app looks again. Ages are counted in days, so a few hours is plenty.
pub const INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// The wait after start-up, a new choice or a new folder before anything is deleted.
/// Deleting is permanent, so a wrong pick can be put right first.
pub const SETTLE: Duration = Duration::from_secs(30);

pub fn age_from_days(days: u16) -> Option<u16> {
    AGE_CHOICES.contains(&days).then_some(days)
}

pub fn age_label(age: Option<u16>) -> String {
    match age {
        Some(days) => format!("After {days} days"),
        None => "Off".to_owned(),
    }
}

/// Whether two folder settings name the same folder on disk.
pub fn same_folder(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        // A folder that does not exist yet can only be compared by name. Case is ignored, as
        // Windows does: a wrong match only leaves a folder uncleaned.
        _ => a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()),
    }
}

/// Decides when the drag-and-drop folder is cleaned and runs each pass off the UI thread.
pub struct Schedule {
    age: Option<u16>,
    next: Instant,
    worker: Option<JoinHandle<io::Result<usize>>>,
}

impl Schedule {
    pub fn new(age: Option<u16>, now: Instant) -> Self {
        Self { age, next: now + SETTLE, worker: None }
    }

    pub fn age(&self) -> Option<u16> {
        self.age
    }

    pub fn set_age(&mut self, age: Option<u16>, now: Instant) {
        self.age = age;
        self.restart(now);
    }

    /// Puts the next pass off by [`SETTLE`]. Call it when the folder changes.
    pub fn restart(&mut self, now: Instant) {
        self.next = now + SETTLE;
    }

    /// Whether [`Self::poll`] would start a pass now, so its arguments are worth working out.
    pub fn due(&self, now: Instant) -> bool {
        self.age.is_some() && self.worker.is_none() && now >= self.next
    }

    /// Starts a pass over `folder` when one is due and returns the result of a finished one.
    /// `None` for the folder means it must not be cleaned at the moment, and puts the pass
    /// off by [`SETTLE`]. `keep` lists files that stay whatever their age.
    pub fn poll(
        &mut self,
        folder: Option<&Path>,
        keep: &[PathBuf],
        now: Instant,
    ) -> Option<io::Result<usize>> {
        let finished = self
            .worker
            .take_if(|worker| worker.is_finished())
            .map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|_| Err(io::Error::other("cleanup worker stopped unexpectedly")))
            });
        if let Some(days) = self.age.filter(|_| self.due(now)) {
            let Some(folder) = folder else {
                self.next = now + SETTLE;
                return finished;
            };
            self.next = now + INTERVAL;
            let folder = folder.to_owned();
            let keep = keep.to_vec();
            let spawned = thread::Builder::new()
                .name("backtracking-cleanup".into())
                .spawn(move || remove_old_exports(&folder, days, &keep, SystemTime::now()));
            match spawned {
                Ok(worker) => self.worker = Some(worker),
                Err(error) => return Some(Err(error)),
            }
        }
        finished
    }
}

impl Drop for Schedule {
    /// Waits for a pass that is still running. A plug-in's code can be unloaded as soon as
    /// its editor is gone, and a pass only lists one folder.
    fn drop(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// Permanently deletes the exporter's WAV files directly inside `folder` that were last
/// written at least `days` days before `now`, and returns how many went.
///
/// Subfolders, links, the files in `keep` and anything not named by the exporter are left
/// alone. A file that cannot be read or removed, for example one still open in another
/// program, is skipped and tried again on a later pass. A folder that does not exist has
/// nothing to remove.
pub fn remove_old_exports(
    folder: &Path,
    days: u16,
    keep: &[PathBuf],
    now: SystemTime,
) -> io::Result<usize> {
    let limit = Duration::from_secs(u64::from(days) * 24 * 60 * 60);
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        // `DirEntry::file_type` does not follow links, so a link to a file is not a file here.
        if !entry.file_type().is_ok_and(|kind| kind.is_file()) {
            continue;
        }
        if !entry.file_name().to_str().is_some_and(is_export_file_name) {
            continue;
        }
        let path = entry.path();
        if keep.contains(&path) {
            continue;
        }
        let old_enough = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            // A file dated in the future has no age yet.
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= limit);
        if old_enough && std::fs::remove_file(path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    struct Folder(PathBuf);

    impl Folder {
        fn new(name: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "backtracking-cleanup-{name}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&path);
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        /// Creates a file last written at `modified`.
        fn file(&self, name: &str, modified: SystemTime) -> PathBuf {
            let path = self.0.join(name);
            File::create(&path).unwrap().set_modified(modified).unwrap();
            path
        }
    }

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_old_exports_are_removed() {
        let folder = Folder::new("ages");
        let now = SystemTime::now();
        let old = folder.file("Backtracking_2026-10-01_09-00-00_12.4s.wav", now - 31 * DAY);
        let old_pair =
            folder.file("Backtracking_2026-10-01_09-00-05_8.0s_120bpm_2-B.wav", now - 45 * DAY);
        let old_style = folder.file("Backtracking-1763100000000-4242-7.wav", now - 400 * DAY);
        let exact = folder.file("Backtracking_2026-10-15_09-00-00_1.0s.wav", now - 30 * DAY);
        let recent = folder.file("Backtracking_2026-11-10_09-00-00_12.4s.wav", now - 29 * DAY);
        let future = folder.file("Backtracking_2026-11-10_09-00-01_12.4s.wav", now + DAY);

        assert_eq!(remove_old_exports(&folder.0, 30, &[], now).unwrap(), 4);
        for gone in [&old, &old_pair, &old_style, &exact] {
            assert!(!gone.exists(), "{}", gone.display());
        }
        assert!(recent.exists());
        assert!(future.exists());

        // A second pass has nothing left to do, and a shorter age takes the next file.
        assert_eq!(remove_old_exports(&folder.0, 30, &[], now).unwrap(), 0);
        assert_eq!(remove_old_exports(&folder.0, 20, &[], now).unwrap(), 1);
        assert!(!recent.exists());
    }

    #[test]
    fn files_the_exporter_did_not_name_are_never_removed() {
        let folder = Folder::new("names");
        let now = SystemTime::now();
        let long_ago = now - 365 * DAY;
        let kept = [
            folder.file("song.wav", long_ago),
            folder.file("Backtracking_notes.wav", long_ago),
            folder.file("Backtracking_2026-10-01_09-00-00_12.4s.mp3", long_ago),
            folder.file("Backtracking_2026-10-01_09-00-00_12.4s_final.wav", long_ago),
            folder.file("project.als", long_ago),
        ];
        // A subfolder is left alone, whatever it is called and whatever it holds.
        let nested = folder.0.join("Backtracking_2026-10-01_09-00-00_12.4s.wav.d");
        fs::create_dir(&nested).unwrap();
        let inside = nested.join("Backtracking_2026-10-01_09-00-00_12.4s.wav");
        File::create(&inside).unwrap().set_modified(long_ago).unwrap();

        assert_eq!(remove_old_exports(&folder.0, 10, &[], now).unwrap(), 0);
        for file in kept.iter().chain([&inside]) {
            assert!(file.exists(), "{}", file.display());
        }
    }

    #[test]
    fn a_missing_folder_has_nothing_to_remove() {
        let folder = Folder::new("missing");
        let absent = folder.0.join("never-created");
        assert_eq!(remove_old_exports(&absent, 10, &[], SystemTime::now()).unwrap(), 0);
        assert!(!absent.exists());
    }

    #[test]
    fn a_file_the_app_still_offers_is_kept() {
        let folder = Folder::new("kept");
        let now = SystemTime::now();
        let offered = folder.file("Backtracking_2026-10-01_09-00-00_12.4s-A.wav", now - 40 * DAY);
        let other = folder.file("Backtracking_2026-10-01_09-00-00_12.4s-B.wav", now - 40 * DAY);

        assert_eq!(remove_old_exports(&folder.0, 10, &[offered.clone()], now).unwrap(), 1);
        assert!(offered.exists());
        assert!(!other.exists());
    }

    #[test]
    fn the_same_folder_is_recognised_under_another_spelling() {
        let folder = Folder::new("same");
        let other = Folder::new("other");
        fs::create_dir(folder.0.join("nested")).unwrap();
        assert!(same_folder(&folder.0, &folder.0.join("nested").join("..")));
        assert!(!same_folder(&folder.0, &other.0));
        assert!(!same_folder(&folder.0, &folder.0.join("nested")));

        let absent = folder.0.join("Never-Created");
        assert!(same_folder(&absent, &folder.0.join("never-created")));
        assert!(!same_folder(&absent, &folder.0.join("never-created-2")));
    }

    fn finish(schedule: &mut Schedule, folder: Option<&Path>, now: Instant) -> io::Result<usize> {
        for _ in 0..500 {
            if let Some(result) = schedule.poll(folder, &[], now) {
                return result;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("the cleanup pass did not finish");
    }

    #[test]
    fn a_pass_waits_for_a_choice_then_runs_once_per_interval() {
        let folder = Folder::new("schedule");
        let old = || {
            folder.file("Backtracking_2026-10-01_09-00-00_12.4s.wav", SystemTime::now() - 40 * DAY)
        };
        let file = old();
        let start = Instant::now();

        // Off never deletes, however long the app runs.
        let mut schedule = Schedule::new(None, start);
        assert_eq!(schedule.age(), None);
        assert!(schedule.poll(Some(&folder.0), &[], start + 2 * INTERVAL).is_none());
        assert!(schedule.worker.is_none());
        assert!(file.exists());

        // A new choice waits before its first pass.
        schedule.set_age(Some(30), start);
        assert!(schedule.poll(Some(&folder.0), &[], start + SETTLE / 2).is_none());
        assert!(schedule.worker.is_none());
        // A folder that must not be cleaned is skipped until it may be.
        assert!(schedule.due(start + SETTLE));
        assert!(schedule.poll(None, &[], start + SETTLE).is_none());
        assert!(schedule.worker.is_none());
        assert!(!schedule.due(start + SETTLE));
        assert!(file.exists());

        let first = start + 2 * SETTLE;
        assert!(schedule.poll(Some(&folder.0), &[], first).is_none());
        assert_eq!(finish(&mut schedule, Some(&folder.0), first).unwrap(), 1);
        assert!(!file.exists());

        // Nothing runs again until the interval has passed.
        let file = old();
        assert!(schedule.poll(Some(&folder.0), &[], first + INTERVAL / 2).is_none());
        assert!(schedule.worker.is_none());
        assert!(file.exists());
        let later = first + INTERVAL;
        assert!(schedule.poll(Some(&folder.0), &[], later).is_none());
        assert_eq!(finish(&mut schedule, Some(&folder.0), later).unwrap(), 1);
        assert!(!file.exists());

        // Turning it off stops the passes, and a new folder waits again.
        let file = old();
        schedule.set_age(None, later);
        assert!(schedule.poll(Some(&folder.0), &[], later + 2 * INTERVAL).is_none());
        assert!(schedule.worker.is_none());
        schedule.set_age(Some(10), later);
        schedule.restart(later + SETTLE);
        assert!(schedule.poll(Some(&folder.0), &[], later + SETTLE).is_none());
        assert!(schedule.worker.is_none());
        assert!(file.exists());
    }

    #[test]
    fn only_the_offered_ages_are_accepted() {
        for days in AGE_CHOICES {
            assert_eq!(age_from_days(days), Some(days));
        }
        for days in [0, 1, 9, 15, 365] {
            assert_eq!(age_from_days(days), None);
        }
        assert_eq!(age_label(None), "Off");
        assert_eq!(age_label(Some(30)), "After 30 days");
    }
}
