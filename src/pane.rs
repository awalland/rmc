use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant},
};

use ratatui::widgets::ListState;
use walkdir::WalkDir;

/// Threshold after which we show "Loading..." indicator
const LOADING_INDICATOR_THRESHOLD: Duration = Duration::from_millis(100);

#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub enum SizeDisplayMode {
    #[default]
    None,
    /// Quick mode: show file sizes only (like ls)
    Quick,
    /// Full mode: calculate directory sizes recursively (like du)
    Full,
}

impl SizeDisplayMode {
    pub fn cycle(self) -> Self {
        match self {
            Self::None => Self::Quick,
            Self::Quick => Self::Full,
            Self::Full => Self::None,
        }
    }
}

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    /// File size in bytes (Some for files, None for directories in quick mode)
    pub size: Option<u64>,
}

#[derive(Default, PartialEq, Clone, Copy)]
pub enum Pane {
    #[default]
    Left,
    Right,
}

/// Result from async directory loading
pub struct LoadResult {
    pub path: PathBuf,
    pub entries: Result<Vec<Entry>, String>,
}

/// Result from async size calculation - uses path for safety across refreshes
pub struct SizeResult {
    pub path: PathBuf,
    pub size: u64,
}

pub struct PaneState {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub list_state: ListState,
    pub selected: HashSet<usize>,
    pub show_hidden: bool,
    /// Receiver for async directory loading results
    load_rx: Option<Receiver<LoadResult>>,
    /// When async loading started (for "Loading..." display)
    pub loading_since: Option<Instant>,
    /// Current size display mode
    pub size_mode: SizeDisplayMode,
    /// Receiver for async size calculation results
    size_rx: Option<Receiver<SizeResult>>,
    /// When size calculation started
    pub size_calc_since: Option<Instant>,
}

impl PaneState {
    pub fn new(path: PathBuf) -> std::io::Result<Self> {
        let mut state = Self {
            path,
            entries: Vec::new(),
            list_state: ListState::default(),
            selected: HashSet::new(),
            show_hidden: false,
            load_rx: None,
            loading_since: None,
            size_mode: SizeDisplayMode::None,
            size_rx: None,
            size_calc_since: None,
        };
        state.load_entries()?;
        if !state.entries.is_empty() {
            state.list_state.select(Some(0));
        }
        Ok(state)
    }

    /// Synchronous directory loading (used for initial load)
    pub fn load_entries(&mut self) -> std::io::Result<()> {
        self.entries.clear();
        self.selected.clear();
        // Cancel any pending size calculations
        self.size_rx = None;
        self.size_calc_since = None;

        self.entries = load_directory_entries(&self.path, self.show_hidden, self.size_mode)?;

        // If in full mode, start async size calculation for directories
        if self.size_mode == SizeDisplayMode::Full {
            self.start_size_calculation();
        }

        Ok(())
    }

    /// Start async directory loading in a background thread
    pub fn load_entries_async(&mut self) {
        let path = self.path.clone();
        let show_hidden = self.show_hidden;
        let size_mode = self.size_mode;

        // Cancel any pending size calculations
        self.size_rx = None;
        self.size_calc_since = None;

        let (tx, rx) = mpsc::channel();
        self.load_rx = Some(rx);
        self.loading_since = Some(Instant::now());

        thread::spawn(move || {
            let entries = load_directory_entries(&path, show_hidden, size_mode)
                .map_err(|e| format_io_error(&e));
            let _ = tx.send(LoadResult { path, entries });
        });
    }

    /// Check if async loading has completed, returns true if results were applied
    pub fn poll_load_result(&mut self) -> Option<Result<(), String>> {
        let rx = self.load_rx.as_ref()?;

        match rx.try_recv() {
            Ok(result) => {
                self.load_rx = None;
                self.loading_since = None;

                // Only apply if path still matches (user might have navigated away)
                if result.path == self.path {
                    match result.entries {
                        Ok(entries) => {
                            self.entries = entries;
                            self.selected.clear();
                            if !self.entries.is_empty() && self.list_state.selected().is_none() {
                                self.list_state.select(Some(0));
                            }
                            // Start size calculation for directories in full mode
                            if self.size_mode == SizeDisplayMode::Full {
                                self.start_size_calculation();
                            }
                            Some(Ok(()))
                        }
                        Err(e) => Some(Err(e)),
                    }
                } else {
                    // Path changed, ignore result
                    None
                }
            }
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.load_rx = None;
                self.loading_since = None;
                Some(Err("Loading thread disconnected".to_owned()))
            }
        }
    }

    /// Returns true if we're loading and should show the indicator
    pub fn is_loading(&self) -> bool {
        if let Some(since) = self.loading_since {
            since.elapsed() >= LOADING_INDICATOR_THRESHOLD
        } else {
            false
        }
    }

    /// Returns true if currently loading (regardless of threshold)
    pub fn is_loading_any(&self) -> bool {
        self.loading_since.is_some()
    }

    /// Returns true if size calculation is in progress
    pub fn is_calculating_sizes(&self) -> bool {
        self.size_rx.is_some()
    }

    /// Start async size calculation for directories
    pub fn start_size_calculation(&mut self) {
        // Collect directories that need size calculation
        let dirs_to_calc: Vec<PathBuf> = self
            .entries
            .iter()
            .filter(|e| e.is_dir && e.name != "..")
            .map(|e| e.path.clone())
            .collect();

        if dirs_to_calc.is_empty() {
            return;
        }

        let (tx, rx) = mpsc::channel();
        self.size_rx = Some(rx);
        self.size_calc_since = Some(Instant::now());

        thread::spawn(move || {
            for path in dirs_to_calc {
                let size = calculate_dir_size(&path);
                if tx.send(SizeResult { path, size }).is_err() {
                    break; // Receiver dropped, stop calculating
                }
            }
        });
    }

    /// Poll for size calculation results and update entries
    pub fn poll_size_results(&mut self) {
        let rx = match &self.size_rx {
            Some(rx) => rx,
            None => return,
        };

        // Process all available results
        loop {
            match rx.try_recv() {
                Ok(result) => {
                    // Find entry by path instead of index (safe across refreshes)
                    if let Some(entry) = self.entries.iter_mut().find(|e| e.path == result.path) {
                        entry.size = Some(result.size);
                    }
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    // All calculations complete
                    self.size_rx = None;
                    self.size_calc_since = None;
                    break;
                }
            }
        }
    }

    /// Cycle size display mode and reload entries
    pub fn cycle_size_mode(&mut self) {
        self.size_mode = self.size_mode.cycle();
        // Cancel any pending size calculations
        self.size_rx = None;
        self.size_calc_since = None;
        // Reload to get sizes
        let _ = self.load_entries();
    }

    pub fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        let _ = self.load_entries();
        self.list_state.select(Some(0));
    }

    pub fn selected_entry(&self) -> Option<&Entry> {
        self.list_state.selected().and_then(|i| self.entries.get(i))
    }

    pub fn move_up(&mut self) {
        if let Some(selected) = self.list_state.selected() {
            if selected > 0 {
                self.list_state.select(Some(selected - 1));
            }
        }
    }

    pub fn move_down(&mut self) {
        if let Some(selected) = self.list_state.selected() {
            if selected < self.entries.len().saturating_sub(1) {
                self.list_state.select(Some(selected + 1));
            }
        }
    }

    pub fn page_up(&mut self, page_size: usize) {
        if let Some(selected) = self.list_state.selected() {
            let new_pos = selected.saturating_sub(page_size);
            self.list_state.select(Some(new_pos));
        }
    }

    pub fn page_down(&mut self, page_size: usize) {
        if let Some(selected) = self.list_state.selected() {
            let max = self.entries.len().saturating_sub(1);
            let new_pos = (selected + page_size).min(max);
            self.list_state.select(Some(new_pos));
        }
    }

    pub fn toggle_selection(&mut self) {
        if let Some(idx) = self.list_state.selected() {
            // Don't allow selecting ".."
            if idx == 0 && self.entries.first().map(|e| e.name == "..").unwrap_or(false) {
                // Move down without selecting
                self.move_down();
                return;
            }

            // Toggle selection
            if self.selected.contains(&idx) {
                self.selected.remove(&idx);
            } else {
                self.selected.insert(idx);
            }

            // Move cursor down
            self.move_down();
        }
    }

    /// Select all items (except "..")
    pub fn select_all(&mut self) {
        let start = if self.entries.first().map(|e| e.name == "..").unwrap_or(false) {
            1
        } else {
            0
        };
        self.selected = (start..self.entries.len()).collect();
    }

    pub fn selected_entries(&self) -> Vec<&Entry> {
        if self.selected.is_empty() {
            // If nothing explicitly selected, return cursor item
            self.selected_entry().into_iter().collect()
        } else {
            self.selected
                .iter()
                .filter_map(|&i| self.entries.get(i))
                .collect()
        }
    }

    pub fn enter_selected(&mut self) -> Result<(), String> {
        if let Some(entry) = self.selected_entry().cloned() {
            if entry.is_dir {
                let old_path = self.path.clone();
                let old_entries = std::mem::take(&mut self.entries);
                let old_selection = self.list_state.selected();
                let old_selected = std::mem::take(&mut self.selected);

                self.path = entry.path.canonicalize().unwrap_or(entry.path);

                if let Err(e) = self.load_entries() {
                    // Restore previous state on failure
                    self.path = old_path;
                    self.entries = old_entries;
                    self.list_state.select(old_selection);
                    self.selected = old_selected;

                    return Err(format_io_error(&e));
                }

                self.list_state.select(Some(0));
            }
        }
        Ok(())
    }
}

// ============================================================================
// Helper Functions
// ============================================================================

/// Format an IO error into a user-friendly message
fn format_io_error(e: &std::io::Error) -> String {
    if e.kind() == std::io::ErrorKind::PermissionDenied {
        "Permission denied".to_owned()
    } else {
        format!("Cannot open directory: {}", e)
    }
}

/// Load directory entries (shared implementation for sync and async loading)
fn load_directory_entries(
    path: &Path,
    show_hidden: bool,
    size_mode: SizeDisplayMode,
) -> std::io::Result<Vec<Entry>> {
    let mut entries = Vec::new();

    // Add parent directory entry
    if let Some(parent) = path.parent() {
        entries.push(Entry {
            name: "..".to_owned(),
            path: parent.to_path_buf(),
            is_dir: true,
            size: None,
        });
    }

    // Read directory entries
    let mut dir_entries: Vec<Entry> = std::fs::read_dir(path)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            if show_hidden {
                true
            } else {
                !e.file_name().to_string_lossy().starts_with('.')
            }
        })
        .map(|e| {
            let metadata = e.metadata().ok();
            let is_dir = metadata.as_ref().map(|m| m.is_dir()).unwrap_or(false);
            // In Quick mode: show entry size for all (files + directory inodes)
            // In Full mode: show file sizes now, directory sizes calculated async
            let size = match size_mode {
                SizeDisplayMode::None => None,
                SizeDisplayMode::Quick => metadata.map(|m| m.len()),
                SizeDisplayMode::Full if !is_dir => metadata.map(|m| m.len()),
                SizeDisplayMode::Full => None, // Directory sizes calculated separately
            };
            Entry {
                name: e.file_name().to_string_lossy().into_owned(),
                path: e.path(),
                is_dir,
                size,
            }
        })
        .collect();

    // Sort: directories first, then by name (case-insensitive)
    dir_entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
    });

    entries.extend(dir_entries);
    Ok(entries)
}

/// Calculate the total size of a directory recursively
fn calculate_dir_size(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.metadata().ok())
        .map(|m| m.len())
        .sum()
}
