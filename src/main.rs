mod dialog;
mod input;
mod job;
mod pane;
mod render;
mod state;
mod theme;
mod util;
mod viewer;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    ExecutableCommand,
};
use ratatui::{layout::Rect, DefaultTerminal};

use job::{JobId, JobManager, JobType};
use pane::{Entry, Pane, PaneState};
use state::AppState;
use util::{ERROR_DISPLAY_SECS, EVENT_POLL_MS};
use viewer::FileViewer;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let mut app = App::new()?;
    ratatui::run(|terminal| app.run(terminal))?;
    Ok(())
}

// ============================================================================
// UI Mode
// ============================================================================

/// UI mode determines what the user is currently interacting with.
///
/// Most variants contain small Copy types or Strings which are cheap to clone.
/// FileViewer is boxed because it can contain up to 50MB of file data.
#[derive(Clone)]
pub enum UIMode {
    Normal,
    JobList { selected: usize },
    ConfirmOverwrite { job_id: JobId, file_path: PathBuf },
    ConfirmDelete {
        entries: Vec<Entry>,
        /// Cached result of conflict check (computed once when dialog opens)
        has_job_conflict: bool,
    },
    MkdirInput { input: String },
    RenameInput { original: PathBuf, input: String },
    /// Rename is in progress - show countdown if it takes too long
    RenameInProgress {
        job_id: JobId,
        started_at: Instant,
        original_name: String,
        new_name: String,
    },
    CommandLine { input: String },
    ConfirmQuit,
    Search { query: String },
    /// File viewer - boxed because it contains potentially large file data.
    /// Handlers use mem::take to avoid cloning this variant.
    FileViewer { viewer: Box<FileViewer> },
}

impl Default for UIMode {
    fn default() -> Self {
        UIMode::Normal
    }
}

// ============================================================================
// App
// ============================================================================

pub struct App {
    pub left: PaneState,
    pub right: PaneState,
    pub active_pane: Pane,
    pub should_quit: bool,
    pub job_manager: JobManager,
    pub ui_mode: UIMode,
    pub error_message: Option<(String, Instant)>,
    pub left_area: Rect,
    pub right_area: Rect,
    pub previous_path: Option<PathBuf>, // For cd -
}

impl App {
    fn new() -> std::io::Result<Self> {
        let cwd = std::env::current_dir()?;
        let state = AppState::load();

        let right_path = state.right_path.unwrap_or_else(|| cwd.clone());

        // Left pane always starts in current directory
        let left = PaneState::new(cwd.clone())?;
        // Right pane uses saved path, falls back to cwd if it fails
        let right = PaneState::new(right_path).or_else(|_| PaneState::new(cwd))?;

        Ok(Self {
            left,
            right,
            active_pane: Pane::Left,
            should_quit: false,
            job_manager: JobManager::new(),
            ui_mode: UIMode::Normal,
            error_message: None,
            left_area: Rect::default(),
            right_area: Rect::default(),
            previous_path: None,
        })
    }

    fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        // Enable mouse capture
        std::io::stdout().execute(EnableMouseCapture)?;

        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;

            // Process job updates
            let (completed_dests, completed_sources) = self.job_manager.process_updates();

            // Refresh panes asynchronously for completed destinations
            for dest in completed_dests {
                if self.left.path == dest && !self.left.is_loading_any() {
                    self.left.load_entries_async();
                }
                if self.right.path == dest && !self.right.is_loading_any() {
                    self.right.load_entries_async();
                }
            }

            // Refresh panes asynchronously for completed move/delete sources
            for source in completed_sources {
                if self.left.path == source && !self.left.is_loading_any() {
                    self.left.load_entries_async();
                }
                if self.right.path == source && !self.right.is_loading_any() {
                    self.right.load_entries_async();
                }
            }

            // Poll for async directory loading results
            if let Some(Err(e)) = self.left.poll_load_result() {
                self.error_message = Some((e, Instant::now()));
            }
            if let Some(Err(e)) = self.right.poll_load_result() {
                self.error_message = Some((e, Instant::now()));
            }

            // Poll for size calculation results
            self.left.poll_size_results();
            self.right.poll_size_results();

            self.job_manager.update_visibility();

            // Check for pending conflicts from JobManager
            self.check_for_conflicts();

            // Handle RenameInProgress: auto-close dialog when done or after timeout
            self.check_rename_progress();

            // Clear old error messages
            if let Some((_, timestamp)) = &self.error_message {
                if timestamp.elapsed() > Duration::from_secs(ERROR_DISPLAY_SECS) {
                    self.error_message = None;
                }
            }

            // Poll for input with timeout
            if event::poll(Duration::from_millis(EVENT_POLL_MS))? {
                self.handle_events(terminal)?;
            }
        }

        // Disable mouse capture
        std::io::stdout().execute(DisableMouseCapture)?;

        // Save state before exiting (only right pane path)
        AppState::save(&self.right.path);

        Ok(())
    }

    /// Check for pending conflicts and show dialog if needed
    fn check_for_conflicts(&mut self) {
        // Only check if we're in Normal mode (don't interrupt other dialogs)
        if !matches!(self.ui_mode, UIMode::Normal) {
            return;
        }

        // Get next pending conflict from JobManager
        if let Some((job_id, file_path)) = self.job_manager.next_pending_conflict() {
            self.ui_mode = UIMode::ConfirmOverwrite { job_id, file_path };
        }
    }

    fn handle_events(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        match event::read()? {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return Ok(());
                }
                self.handle_key_event(key.code, key.modifiers, terminal)?;
            }
            Event::Mouse(mouse) => {
                if matches!(self.ui_mode, UIMode::Normal) {
                    self.handle_mouse(mouse.kind, mouse.column, mouse.row);
                }
            }
            _ => {}
        }
        Ok(())
    }

    // ========================================================================
    // Pane Access Helpers
    // ========================================================================

    /// Get a reference to the active pane
    pub fn active_pane(&self) -> &PaneState {
        match self.active_pane {
            Pane::Left => &self.left,
            Pane::Right => &self.right,
        }
    }

    /// Get a mutable reference to the active pane
    pub fn active_pane_mut(&mut self) -> &mut PaneState {
        match self.active_pane {
            Pane::Left => &mut self.left,
            Pane::Right => &mut self.right,
        }
    }

    /// Get a reference to the inactive pane
    pub fn other_pane(&self) -> &PaneState {
        match self.active_pane {
            Pane::Left => &self.right,
            Pane::Right => &self.left,
        }
    }

    /// Get a mutable reference to the inactive pane
    #[allow(dead_code)]
    pub fn other_pane_mut(&mut self) -> &mut PaneState {
        match self.active_pane {
            Pane::Left => &mut self.right,
            Pane::Right => &mut self.left,
        }
    }

    pub fn toggle_pane(&mut self) {
        self.active_pane = match self.active_pane {
            Pane::Left => Pane::Right,
            Pane::Right => Pane::Left,
        };
    }

    pub fn swap_panes(&mut self) {
        std::mem::swap(&mut self.left.path, &mut self.right.path);
        std::mem::swap(&mut self.left.entries, &mut self.right.entries);
        std::mem::swap(&mut self.left.selected, &mut self.right.selected);
        std::mem::swap(&mut self.left.list_state, &mut self.right.list_state);
    }

    // ========================================================================
    // File Operations
    // ========================================================================

    pub fn transfer_selected_to_other_pane(&mut self, job_type: JobType) {
        let (source_pane, dest_pane) = match self.active_pane {
            Pane::Left => (&self.left, &self.right),
            Pane::Right => (&self.right, &self.left),
        };

        let entries_to_transfer: Vec<PathBuf> = source_pane
            .selected_entries()
            .iter()
            .filter(|e| e.name != "..")
            .map(|e| e.path.clone())
            .collect();

        if entries_to_transfer.is_empty() {
            return;
        }

        let dest_dir = dest_pane.path.clone();

        // Clear selection
        self.active_pane_mut().selected.clear();

        // Start a job for each selected item
        for source in entries_to_transfer {
            self.job_manager.start_job(job_type, source, dest_dir.clone());
        }
    }
}
