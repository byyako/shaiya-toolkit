//! `--shot`: the pieces that can be tested without a window. A scene is set up by the tool, the shell waits until the background work
//! is idle (a condition, not a frame count), asks the window for a screenshot, and writes the PNG.

use std::path::Path;

/// Consecutive calm frames that count as idle: no task running and no task started since the frame before. The last of them is a frame
/// after every finished task's result was read, so a follow-up task that a result starts is seen before the scene is called ready.
pub const CALM_FRAMES: u32 = 3;

/// Tells when the background work of a window has settled.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Idle {
    spawned: u64,
    calm: u32,
}

impl Idle {
    /// Call once per frame with the number of running tasks and the number of tasks started so far; true when the window is idle.
    pub fn observe(&mut self, running: usize, spawned: u64) -> bool {
        if running == 0 && spawned == self.spawned {
            self.calm = self.calm.saturating_add(1);
        } else {
            self.calm = 0;
        }
        self.spawned = spawned;
        self.calm >= CALM_FRAMES
    }

    /// Starts counting again (after the scene took another step).
    pub fn reset(&mut self) {
        self.calm = 0;
    }
}

/// How a saved folder is shown. Normally its path; under `--shot` (`hide_paths`) the name it was saved under, or, when it has none, the
/// last part of the path, so that a screenshot never shows a user name, a machine name or any absolute path.
pub fn folder_label(hide_paths: bool, name: &str, path: &Path) -> String {
    if !hide_paths {
        return path.display().to_string();
    }
    if !name.is_empty() {
        return name.to_string();
    }
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "(folder)".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_needs_calm_frames_in_a_row_with_nothing_started() {
        let mut i = Idle::default();
        assert!(!i.observe(0, 5), "a task was started since the frame before");
        assert!(!i.observe(0, 5));
        assert!(!i.observe(0, 5));
        assert!(i.observe(0, 5));
        assert!(!i.observe(1, 5), "a task is running");
        assert!(!i.observe(0, 5));
        assert!(!i.observe(0, 6), "a finished task's result started another one");
        assert!(!i.observe(0, 6) && !i.observe(0, 6));
        assert!(i.observe(0, 6));
        i.reset();
        assert!(!i.observe(0, 6));
    }

    #[test]
    fn a_folder_is_shown_by_name_never_by_path_under_shot() {
        let p = Path::new("/home/somebody/Games/Shaiya");
        assert_eq!(folder_label(true, "Demo client", p), "Demo client");
        assert_eq!(folder_label(true, "", p), "Shaiya");
        assert_eq!(folder_label(true, "", Path::new("/")), "(folder)");
        assert_eq!(folder_label(false, "Demo client", p), p.display().to_string());
        let shown = folder_label(true, "Demo client", p);
        assert!(!shown.contains("somebody") && !shown.contains('/') && !shown.contains(std::path::MAIN_SEPARATOR));
    }
}
