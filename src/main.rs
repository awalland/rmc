mod dialog;
mod job;
mod pane;
mod state;
mod theme;

use std::{
    env,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers, MouseButton, MouseEventKind},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Sparkline},
    DefaultTerminal, Frame,
};

use dialog::{centered_rect, handle_yes_no_keys, render_dialog_frame, render_yes_no_buttons, DialogResult};
use job::{ConflictResolution, Job, JobId, JobManager, JobStatus, JobType, JobUpdate};
use pane::{Entry, Pane, PaneState};
use state::AppState;
use theme::THEME;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let mut app = App::new()?;
    ratatui::run(|terminal| app.run(terminal))?;
    Ok(())
}

// ============================================================================
// UI Mode
// ============================================================================

#[derive(Clone)]
enum UIMode {
    Normal,
    JobList { selected: usize },
    ConfirmOverwrite { job_id: JobId, file_path: PathBuf },
    ConfirmDelete { entries: Vec<Entry> },
    MkdirInput { input: String },
    RenameInput { original: PathBuf, input: String },
    CommandLine { input: String },
    ConfirmQuit,
    Search { query: String },
}

impl Default for UIMode {
    fn default() -> Self {
        UIMode::Normal
    }
}

// ============================================================================
// App
// ============================================================================

struct App {
    left: PaneState,
    right: PaneState,
    active_pane: Pane,
    should_quit: bool,
    job_manager: JobManager,
    ui_mode: UIMode,
    error_message: Option<(String, Instant)>,
    left_area: Rect,
    right_area: Rect,
    previous_path: Option<PathBuf>, // For cd -
}

impl App {
    fn new() -> std::io::Result<Self> {
        let cwd = std::env::current_dir()?;
        let state = AppState::load();

        let right_path = state.right_path.unwrap_or_else(|| cwd.clone());

        // Left pane always starts in current directory
        let left = PaneState::new(cwd.clone())?;
        // Right pane uses saved path, falls back to cwd if it fails
        let right = PaneState::new(right_path)
            .or_else(|_| PaneState::new(cwd))?;

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

            // Refresh panes for completed destinations
            for dest in completed_dests {
                if self.left.path == dest {
                    let _ = self.left.load_entries();
                }
                if self.right.path == dest {
                    let _ = self.right.load_entries();
                }
            }

            // Refresh panes for completed move sources
            for source in completed_sources {
                if self.left.path == source {
                    let _ = self.left.load_entries();
                }
                if self.right.path == source {
                    let _ = self.right.load_entries();
                }
            }

            self.job_manager.update_visibility();

            // Check for pending conflicts
            self.check_for_conflicts();

            // Clear old error messages (after 3 seconds)
            if let Some((_, timestamp)) = &self.error_message {
                if timestamp.elapsed() > Duration::from_secs(3) {
                    self.error_message = None;
                }
            }

            // Poll for input with timeout
            if event::poll(Duration::from_millis(50))? {
                self.handle_events(terminal)?;
            }
        }

        // Disable mouse capture
        std::io::stdout().execute(DisableMouseCapture)?;

        // Save state before exiting (only right pane path)
        AppState::save(&self.right.path);

        Ok(())
    }

    fn check_for_conflicts(&mut self) {
        // Check if there's a pending conflict
        if let Ok(update) = self.job_manager.progress_rx.try_recv() {
            if let JobUpdate::ConflictDetected { job_id, file_path } = update {
                self.ui_mode = UIMode::ConfirmOverwrite { job_id, file_path };
            }
        }
    }

    fn handle_events(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        match event::read()? {
            Event::Key(key) => {
                if key.kind != KeyEventKind::Press {
                    return Ok(());
                }

                // Clear error on any key press
                self.error_message = None;

                match &self.ui_mode.clone() {
                    UIMode::Normal => self.handle_normal_mode(key.code, key.modifiers, terminal)?,
                    UIMode::JobList { selected } => self.handle_job_list_mode(key.code, *selected),
                    UIMode::ConfirmOverwrite { job_id, .. } => {
                        self.handle_confirm_overwrite(key.code, *job_id)
                    }
                    UIMode::ConfirmDelete { entries } => {
                        self.handle_confirm_delete(key.code, entries.clone())
                    }
                    UIMode::MkdirInput { input } => {
                        self.handle_mkdir_input(key.code, input.clone())
                    }
                    UIMode::RenameInput { original, input } => {
                        self.handle_rename_input(key.code, original.clone(), input.clone())
                    }
                    UIMode::CommandLine { input } => {
                        self.handle_command_line(key.code, input.clone(), terminal)?
                    }
                    UIMode::ConfirmQuit => {
                        self.handle_confirm_quit(key.code);
                    }
                    UIMode::Search { query } => {
                        self.handle_search(key.code, key.modifiers, query.clone());
                    }
                }
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

    fn handle_normal_mode(&mut self, key: KeyCode, modifiers: KeyModifiers, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        // Handle Ctrl+S for search
        if modifiers.contains(KeyModifiers::CONTROL) && key == KeyCode::Char('s') {
            self.ui_mode = UIMode::Search { query: String::new() };
            return Ok(());
        }

        match key {
            KeyCode::Char('q') | KeyCode::Esc => {
                if self.job_manager.active_job_count() > 0 {
                    self.ui_mode = UIMode::ConfirmQuit;
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Tab => self.toggle_pane(),
            KeyCode::Up | KeyCode::Char('k') => self.active_pane_mut().move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.active_pane_mut().move_down(),
            KeyCode::PageUp => self.active_pane_mut().page_up(10),
            KeyCode::PageDown => self.active_pane_mut().page_down(10),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Err(msg) = self.active_pane_mut().enter_selected() {
                    self.error_message = Some((msg, Instant::now()));
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                let pane = self.active_pane_mut();
                if let Some(parent) = pane.path.parent().map(|p| p.to_path_buf()) {
                    let old_path = pane.path.clone();
                    let old_entries = std::mem::take(&mut pane.entries);
                    let old_selection = pane.list_state.selected();
                    let old_selected = std::mem::take(&mut pane.selected);

                    pane.path = parent;

                    if let Err(e) = pane.load_entries() {
                        pane.path = old_path;
                        pane.entries = old_entries;
                        pane.list_state.select(old_selection);
                        pane.selected = old_selected;

                        let msg = if e.kind() == std::io::ErrorKind::PermissionDenied {
                            "Permission denied".to_owned()
                        } else {
                            format!("Cannot open directory: {}", e)
                        };
                        self.error_message = Some((msg, Instant::now()));
                    } else {
                        self.active_pane_mut().list_state.select(Some(0));
                    }
                }
            }
            KeyCode::Char('c') | KeyCode::F(5) => {
                self.transfer_selected_to_other_pane(JobType::Copy);
            }
            KeyCode::Char('m') | KeyCode::F(6) => {
                self.transfer_selected_to_other_pane(JobType::Move);
            }
            KeyCode::Char('J') => {
                self.ui_mode = UIMode::JobList { selected: 0 };
            }
            KeyCode::Insert => {
                self.active_pane_mut().toggle_selection();
            }
            KeyCode::Delete | KeyCode::F(8) => {
                self.initiate_delete();
            }
            KeyCode::Char('e') | KeyCode::F(4) => {
                if let Err(msg) = self.edit_selected(terminal) {
                    self.error_message = Some((msg, Instant::now()));
                }
            }
            KeyCode::Char('H') => {
                self.active_pane_mut().toggle_hidden();
            }
            KeyCode::F(7) => {
                self.ui_mode = UIMode::MkdirInput { input: String::new() };
            }
            KeyCode::F(2) => {
                self.initiate_rename();
            }
            KeyCode::Char('U') => {
                self.swap_panes();
            }
            KeyCode::Char(':') => {
                self.ui_mode = UIMode::CommandLine { input: String::new() };
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_job_list_mode(&mut self, key: KeyCode, selected: usize) {
        let job_count = self.job_manager.all_jobs().len();

        match key {
            KeyCode::Char('J') | KeyCode::Esc => {
                self.ui_mode = UIMode::Normal;
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if selected > 0 {
                    self.ui_mode = UIMode::JobList {
                        selected: selected - 1,
                    };
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if selected < job_count.saturating_sub(1) {
                    self.ui_mode = UIMode::JobList {
                        selected: selected + 1,
                    };
                }
            }
            KeyCode::Char('K') => {
                // Kill selected job
                let jobs: Vec<_> = self.job_manager.all_jobs().iter().map(|j| j.id).collect();
                if let Some(&job_id) = jobs.get(selected) {
                    self.job_manager.cancel_job(job_id);
                }
            }
            KeyCode::Char('P') => {
                // Pause/resume selected job
                let jobs: Vec<_> = self.job_manager.all_jobs().iter().map(|j| j.id).collect();
                if let Some(&job_id) = jobs.get(selected) {
                    self.job_manager.toggle_pause_job(job_id);
                }
            }
            KeyCode::Char('d') => {
                // Dismiss completed/failed job
                let jobs: Vec<_> = self.job_manager.all_jobs().iter().map(|j| j.id).collect();
                if let Some(&job_id) = jobs.get(selected) {
                    self.job_manager.dismiss_job(job_id);
                    // Adjust selection if needed
                    let new_count = self.job_manager.all_jobs().len();
                    if selected >= new_count && new_count > 0 {
                        self.ui_mode = UIMode::JobList {
                            selected: new_count - 1,
                        };
                    }
                }
            }
            _ => {}
        }
    }

    fn handle_confirm_overwrite(&mut self, key: KeyCode, job_id: JobId) {
        let resolution = match key {
            KeyCode::Char('o') => Some(ConflictResolution::Overwrite),
            KeyCode::Char('s') => Some(ConflictResolution::Skip),
            KeyCode::Char('a') => Some(ConflictResolution::OverwriteAll),
            KeyCode::Char('n') => Some(ConflictResolution::SkipAll),
            KeyCode::Esc => Some(ConflictResolution::Cancel),
            _ => None,
        };

        if let Some(res) = resolution {
            self.job_manager.send_conflict_resolution(job_id, res);
            self.ui_mode = UIMode::Normal;
        }
    }

    fn initiate_delete(&mut self) {
        let pane = match self.active_pane {
            Pane::Left => &self.left,
            Pane::Right => &self.right,
        };

        let entries: Vec<Entry> = pane
            .selected_entries()
            .into_iter()
            .filter(|e| e.name != "..")
            .cloned()
            .collect();

        if entries.is_empty() {
            return;
        }

        self.ui_mode = UIMode::ConfirmDelete { entries };
    }

    fn handle_confirm_delete(&mut self, key: KeyCode, entries: Vec<Entry>) {
        match handle_yes_no_keys(key) {
            DialogResult::Accept => {
                // Get parent directory for refresh after deletion
                let parent_dir = match self.active_pane {
                    Pane::Left => self.left.path.clone(),
                    Pane::Right => self.right.path.clone(),
                };

                // Collect paths to delete
                let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();

                // Start background delete job
                self.job_manager.start_delete_job(paths, parent_dir);

                // Clear selection
                match self.active_pane {
                    Pane::Left => self.left.selected.clear(),
                    Pane::Right => self.right.selected.clear(),
                }

                self.ui_mode = UIMode::Normal;
            }
            DialogResult::Reject => {
                self.ui_mode = UIMode::Normal;
            }
            DialogResult::Pending => {}
        }
    }

    fn handle_confirm_quit(&mut self, key: KeyCode) {
        match handle_yes_no_keys(key) {
            DialogResult::Accept => {
                // Cancel all active jobs
                let job_ids: Vec<_> = self
                    .job_manager
                    .all_jobs()
                    .iter()
                    .filter(|j| matches!(j.status, JobStatus::Running { .. } | JobStatus::Visible))
                    .map(|j| j.id)
                    .collect();

                for id in job_ids {
                    self.job_manager.cancel_job(id);
                }

                self.should_quit = true;
            }
            DialogResult::Reject => {
                self.ui_mode = UIMode::Normal;
            }
            DialogResult::Pending => {}
        }
    }

    fn handle_search(&mut self, key: KeyCode, modifiers: KeyModifiers, mut query: String) {
        // Ctrl+S jumps to next match
        if modifiers.contains(KeyModifiers::CONTROL) && key == KeyCode::Char('s') {
            if !query.is_empty() {
                self.search_next(&query);
            }
            return;
        }

        match key {
            KeyCode::Esc | KeyCode::Enter => {
                self.ui_mode = UIMode::Normal;
            }
            KeyCode::Backspace => {
                query.pop();
                if !query.is_empty() {
                    self.search_jump(&query);
                }
                self.ui_mode = UIMode::Search { query };
            }
            KeyCode::Char(c) => {
                query.push(c);
                self.search_jump(&query);
                self.ui_mode = UIMode::Search { query };
            }
            _ => {}
        }
    }

    fn search_jump(&mut self, query: &str) {
        let pane = self.active_pane_mut();
        let query_lower = query.to_lowercase();

        // Find first match starting from current position
        let current = pane.list_state.selected().unwrap_or(0);

        // First search from current position to end
        for i in current..pane.entries.len() {
            if pane.entries[i].name.to_lowercase().contains(&query_lower) {
                pane.list_state.select(Some(i));
                return;
            }
        }

        // Then wrap around from beginning
        for i in 0..current {
            if pane.entries[i].name.to_lowercase().contains(&query_lower) {
                pane.list_state.select(Some(i));
                return;
            }
        }
    }

    fn search_next(&mut self, query: &str) {
        let pane = self.active_pane_mut();
        let query_lower = query.to_lowercase();

        let current = pane.list_state.selected().unwrap_or(0);
        let start = current + 1;

        // Search from next position to end
        for i in start..pane.entries.len() {
            if pane.entries[i].name.to_lowercase().contains(&query_lower) {
                pane.list_state.select(Some(i));
                return;
            }
        }

        // Wrap around from beginning
        for i in 0..=current {
            if pane.entries[i].name.to_lowercase().contains(&query_lower) {
                pane.list_state.select(Some(i));
                return;
            }
        }
    }

    fn handle_mkdir_input(&mut self, key: KeyCode, mut input: String) {
        match key {
            KeyCode::Enter => {
                if !input.is_empty() {
                    let pane = match self.active_pane {
                        Pane::Left => &self.left,
                        Pane::Right => &self.right,
                    };
                    let new_dir = pane.path.join(&input);

                    match std::fs::create_dir(&new_dir) {
                        Ok(()) => {
                            // Refresh the pane
                            match self.active_pane {
                                Pane::Left => {
                                    let _ = self.left.load_entries();
                                }
                                Pane::Right => {
                                    let _ = self.right.load_entries();
                                }
                            }
                        }
                        Err(e) => {
                            self.error_message = Some((format!("mkdir failed: {}", e), Instant::now()));
                        }
                    }
                }
                self.ui_mode = UIMode::Normal;
            }
            KeyCode::Esc => {
                self.ui_mode = UIMode::Normal;
            }
            KeyCode::Backspace => {
                input.pop();
                self.ui_mode = UIMode::MkdirInput { input };
            }
            KeyCode::Char(c) => {
                input.push(c);
                self.ui_mode = UIMode::MkdirInput { input };
            }
            _ => {}
        }
    }

    fn initiate_rename(&mut self) {
        let pane = match self.active_pane {
            Pane::Left => &self.left,
            Pane::Right => &self.right,
        };

        let Some(entry) = pane.selected_entry() else {
            return;
        };

        if entry.name == ".." {
            return;
        }

        self.ui_mode = UIMode::RenameInput {
            original: entry.path.clone(),
            input: entry.name.clone(),
        };
    }

    fn handle_rename_input(&mut self, key: KeyCode, original: PathBuf, mut input: String) {
        match key {
            KeyCode::Enter => {
                if !input.is_empty() {
                    let new_path = original.parent().unwrap_or(Path::new(".")).join(&input);

                    if new_path != original {
                        match std::fs::rename(&original, &new_path) {
                            Ok(()) => {
                                // Refresh the pane
                                match self.active_pane {
                                    Pane::Left => {
                                        let _ = self.left.load_entries();
                                    }
                                    Pane::Right => {
                                        let _ = self.right.load_entries();
                                    }
                                }
                            }
                            Err(e) => {
                                self.error_message =
                                    Some((format!("Rename failed: {}", e), Instant::now()));
                            }
                        }
                    }
                }
                self.ui_mode = UIMode::Normal;
            }
            KeyCode::Esc => {
                self.ui_mode = UIMode::Normal;
            }
            KeyCode::Backspace => {
                input.pop();
                self.ui_mode = UIMode::RenameInput { original, input };
            }
            KeyCode::Char(c) => {
                input.push(c);
                self.ui_mode = UIMode::RenameInput { original, input };
            }
            _ => {}
        }
    }

    fn handle_command_line(
        &mut self,
        key: KeyCode,
        mut input: String,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        match key {
            KeyCode::Enter => {
                if !input.is_empty() {
                    self.execute_command(&input, terminal)?;
                }
                self.ui_mode = UIMode::Normal;
            }
            KeyCode::Esc => {
                self.ui_mode = UIMode::Normal;
            }
            KeyCode::Backspace => {
                input.pop();
                self.ui_mode = UIMode::CommandLine { input };
            }
            KeyCode::Tab => {
                let completed = self.complete_path(&input);
                self.ui_mode = UIMode::CommandLine { input: completed };
            }
            KeyCode::Char(c) => {
                input.push(c);
                self.ui_mode = UIMode::CommandLine { input };
            }
            _ => {}
        }
        Ok(())
    }

    fn complete_path(&self, input: &str) -> String {
        // Find the last "word" (space-separated) to complete
        let (prefix, word_to_complete) = match input.rfind(' ') {
            Some(pos) => (&input[..=pos], &input[pos + 1..]),
            None => ("", input),
        };

        if word_to_complete.is_empty() {
            return input.to_owned();
        }

        // Expand ~ to home directory for path resolution
        let expanded = if word_to_complete.starts_with("~/") {
            std::env::var("HOME")
                .map(|h| format!("{}/{}", h, &word_to_complete[2..]))
                .unwrap_or_else(|_| word_to_complete.to_owned())
        } else if word_to_complete == "~" {
            std::env::var("HOME").unwrap_or_else(|_| word_to_complete.to_owned())
        } else {
            word_to_complete.to_owned()
        };

        // Determine parent directory and prefix to match
        let path = PathBuf::from(&expanded);
        let (search_dir, match_prefix) = if expanded.ends_with('/') {
            // User typed "dir/" - list contents of dir
            (path.clone(), String::new())
        } else if path.is_absolute() {
            // Absolute path
            (
                path.parent().unwrap_or(Path::new("/")).to_path_buf(),
                path.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        } else {
            // Relative path
            let base = match self.active_pane {
                Pane::Left => &self.left.path,
                Pane::Right => &self.right.path,
            };
            let full_path = base.join(&path);
            (
                full_path.parent().unwrap_or(base).to_path_buf(),
                path.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default(),
            )
        };

        // Find matching entries
        let Ok(entries) = std::fs::read_dir(&search_dir) else {
            return input.to_owned();
        };

        let matches: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with(&match_prefix)
            })
            .map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                // Add trailing slash for directories
                if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    format!("{}/", name)
                } else {
                    name
                }
            })
            .collect();

        if matches.is_empty() {
            return input.to_owned();
        }

        // Find common prefix among matches
        let common = if matches.len() == 1 {
            matches[0].clone()
        } else {
            let first = &matches[0];
            let mut common_len = first.len();
            for m in &matches[1..] {
                common_len = first
                    .chars()
                    .zip(m.chars())
                    .take_while(|(a, b)| a == b)
                    .count()
                    .min(common_len);
            }
            first[..common_len].to_owned()
        };

        // Build the completed path
        let completed_word = if word_to_complete.starts_with("~/") {
            format!("~/{}", &common)
        } else if word_to_complete == "~" && !common.is_empty() {
            format!("~/{}", &common)
        } else if expanded.ends_with('/') {
            format!("{}{}", word_to_complete, common)
        } else {
            // Replace the filename part with the completion
            let word_path = PathBuf::from(word_to_complete);
            if let Some(parent) = word_path.parent() {
                if parent.as_os_str().is_empty() {
                    common
                } else if parent == Path::new("/") {
                    // Root directory - don't add extra slash
                    format!("/{}", common)
                } else {
                    format!("{}/{}", parent.display(), common)
                }
            } else {
                common
            }
        };

        format!("{}{}", prefix, completed_word)
    }

    fn execute_command(&mut self, command: &str, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        let command = command.trim();

        // Handle cd specially
        if command == "cd" || command.starts_with("cd ") {
            let path_str = if command == "cd" {
                ""
            } else {
                command.strip_prefix("cd ").unwrap_or("").trim()
            };

            let pane = self.active_pane_mut();
            let current_path = pane.path.clone();

            let target = if path_str.is_empty() || path_str == "~" {
                // cd or cd ~ -> home directory
                std::env::var("HOME")
                    .map(PathBuf::from)
                    .unwrap_or(current_path.clone())
            } else if path_str == "-" {
                // cd - -> previous directory
                self.previous_path.clone().unwrap_or(current_path.clone())
            } else if path_str.starts_with("~/") {
                // cd ~/something -> home + path
                std::env::var("HOME")
                    .map(|h| PathBuf::from(h).join(&path_str[2..]))
                    .unwrap_or_else(|_| current_path.join(path_str))
            } else {
                // Relative or absolute path
                let p = PathBuf::from(path_str);
                if p.is_absolute() {
                    p
                } else {
                    current_path.join(path_str)
                }
            };

            // Canonicalize to resolve . and ..
            let target = target.canonicalize().unwrap_or(target);

            // Try to navigate
            let pane = self.active_pane_mut();
            let old_path = pane.path.clone();

            if target.is_dir() {
                pane.path = target;
                if let Err(e) = pane.load_entries() {
                    pane.path = old_path;
                    let _ = pane.load_entries();
                    self.error_message = Some((format!("cd: {}", e), Instant::now()));
                } else {
                    pane.list_state.select(Some(0));
                    self.previous_path = Some(old_path);
                }
            } else {
                self.error_message = Some((format!("cd: not a directory: {}", path_str), Instant::now()));
            }

            return Ok(());
        }

        // For other commands, execute in shell
        let pane_path = self.active_pane_mut().path.clone();

        // Leave alternate screen and disable raw mode
        std::io::stdout().execute(LeaveAlternateScreen)?;
        crossterm::terminal::disable_raw_mode()?;

        // Run the command
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&pane_path)
            .status();

        // Wait for user to press enter
        if status.is_ok() {
            println!("\n[Press Enter to continue]");
            let mut buf = String::new();
            let _ = std::io::stdin().read_line(&mut buf);
        }

        // Restore terminal
        crossterm::terminal::enable_raw_mode()?;
        std::io::stdout().execute(EnterAlternateScreen)?;
        terminal.clear()?;

        // Refresh pane in case files changed
        let _ = self.active_pane_mut().load_entries();

        Ok(())
    }

    fn handle_mouse(&mut self, kind: MouseEventKind, col: u16, row: u16) {
        // Check which pane was clicked
        let in_left = col >= self.left_area.x
            && col < self.left_area.x + self.left_area.width
            && row >= self.left_area.y
            && row < self.left_area.y + self.left_area.height;

        let in_right = col >= self.right_area.x
            && col < self.right_area.x + self.right_area.width
            && row >= self.right_area.y
            && row < self.right_area.y + self.right_area.height;

        match kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if in_left {
                    self.active_pane = Pane::Left;
                    // Calculate which entry was clicked (account for border)
                    let inner_row = row.saturating_sub(self.left_area.y + 1);
                    if (inner_row as usize) < self.left.entries.len() {
                        self.left.list_state.select(Some(inner_row as usize));
                    }
                } else if in_right {
                    self.active_pane = Pane::Right;
                    let inner_row = row.saturating_sub(self.right_area.y + 1);
                    if (inner_row as usize) < self.right.entries.len() {
                        self.right.list_state.select(Some(inner_row as usize));
                    }
                }
            }
            MouseEventKind::ScrollUp => {
                if in_left {
                    self.active_pane = Pane::Left;
                    self.left.move_up();
                } else if in_right {
                    self.active_pane = Pane::Right;
                    self.right.move_up();
                }
            }
            MouseEventKind::ScrollDown => {
                if in_left {
                    self.active_pane = Pane::Left;
                    self.left.move_down();
                } else if in_right {
                    self.active_pane = Pane::Right;
                    self.right.move_down();
                }
            }
            _ => {}
        }
    }

    fn edit_selected(&mut self, terminal: &mut DefaultTerminal) -> Result<(), String> {
        let pane = match self.active_pane {
            Pane::Left => &self.left,
            Pane::Right => &self.right,
        };

        let Some(entry) = pane.selected_entry() else {
            return Ok(());
        };

        if entry.name == ".." {
            return Ok(());
        }

        let editor = env::var("EDITOR").unwrap_or_else(|_| "vi".to_owned());
        let path = entry.path.clone();

        // Leave alternate screen and disable raw mode
        let mut stdout = std::io::stdout();
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = stdout.execute(LeaveAlternateScreen);

        // Run the editor
        let status = std::process::Command::new(&editor)
            .arg(&path)
            .status();

        // Re-enter alternate screen and enable raw mode
        let _ = stdout.execute(EnterAlternateScreen);
        let _ = crossterm::terminal::enable_raw_mode();

        // Force ratatui to do a full redraw
        let _ = terminal.clear();

        match status {
            Ok(exit_status) => {
                if !exit_status.success() {
                    return Err(format!("Editor exited with status {}", exit_status));
                }
            }
            Err(e) => {
                return Err(format!("Failed to run '{}': {}", editor, e));
            }
        }

        Ok(())
    }

    fn toggle_pane(&mut self) {
        self.active_pane = match self.active_pane {
            Pane::Left => Pane::Right,
            Pane::Right => Pane::Left,
        };
    }

    fn swap_panes(&mut self) {
        std::mem::swap(&mut self.left, &mut self.right);
    }

    fn active_pane_mut(&mut self) -> &mut PaneState {
        match self.active_pane {
            Pane::Left => &mut self.left,
            Pane::Right => &mut self.right,
        }
    }

    fn transfer_selected_to_other_pane(&mut self, job_type: JobType) {
        let (source_pane, dest_pane) = match self.active_pane {
            Pane::Left => (&self.left, &self.right),
            Pane::Right => (&self.right, &self.left),
        };

        let entries: Vec<Entry> = source_pane.selected_entries()
            .into_iter()
            .filter(|e| e.name != "..")
            .cloned()
            .collect();

        if entries.is_empty() {
            return;
        }

        let dest_path = dest_pane.path.clone();

        // Validate and start job for each entry
        for entry in entries {
            if let Err(msg) = self.validate_transfer(&entry.path, &dest_path, job_type) {
                self.error_message = Some((msg, Instant::now()));
                continue;
            }

            self.job_manager.start_job(job_type, entry.path, dest_path.clone());
        }

        // Clear selection after transfer initiated
        match self.active_pane {
            Pane::Left => self.left.selected.clear(),
            Pane::Right => self.right.selected.clear(),
        }
    }

    fn validate_transfer(&self, source: &Path, dest_dir: &Path, job_type: JobType) -> Result<(), String> {
        let action = match job_type {
            JobType::Copy => "copy",
            JobType::Move => "move",
            JobType::Delete => "delete", // Not used, delete has its own validation
        };
        // Check source exists
        if !source.exists() {
            return Err("Source file not found".to_owned());
        }

        // Check same directory
        if source.parent() == Some(dest_dir) {
            return Err(format!("Cannot {} to same directory", action));
        }

        // Check destination inside source (for directories)
        if source.is_dir() {
            let dest_canonical = dest_dir.canonicalize().unwrap_or(dest_dir.to_path_buf());
            let source_canonical = source.canonicalize().unwrap_or(source.to_path_buf());
            if dest_canonical.starts_with(&source_canonical) {
                return Err(format!("Cannot {} directory into itself", action));
            }
        }

        // Check read permission
        if std::fs::metadata(source).is_err() {
            return Err("Permission denied: cannot read source".to_owned());
        }

        // Check write permission on destination
        let test_file = dest_dir.join(".rc_write_test");
        match std::fs::File::create(&test_file) {
            Ok(_) => {
                let _ = std::fs::remove_file(&test_file);
            }
            Err(_) => return Err("Permission denied: cannot write to destination".to_owned()),
        }

        Ok(())
    }

    // ========================================================================
    // Rendering
    // ========================================================================

    fn render(&mut self, frame: &mut Frame) {
        let active_jobs = self.job_manager.active_job_count();
        let has_status = active_jobs > 0 || self.error_message.is_some();

        // Main layout: panes + optional status bar + help bar
        let main_layout = if has_status {
            Layout::vertical([
                Constraint::Min(0),    // Panes
                Constraint::Length(1), // Status bar
                Constraint::Length(1), // Help bar
            ])
            .split(frame.area())
        } else {
            Layout::vertical([
                Constraint::Min(0),    // Panes
                Constraint::Length(1), // Help bar
            ])
            .split(frame.area())
        };

        // Pane layout
        let pane_layout = Layout::horizontal([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(main_layout[0]);

        self.left_area = pane_layout[0];
        self.right_area = pane_layout[1];

        self.render_pane(frame, pane_layout[0], Pane::Left);
        self.render_pane(frame, pane_layout[1], Pane::Right);

        // Status bar and help bar
        if has_status {
            self.render_status_bar(frame, main_layout[1]);
            self.render_help_bar(frame, main_layout[2]);
        } else {
            self.render_help_bar(frame, main_layout[1]);
        }

        // Overlays
        match &self.ui_mode {
            UIMode::JobList { selected } => {
                self.render_job_popup(frame, *selected);
            }
            UIMode::ConfirmOverwrite { file_path, .. } => {
                self.render_conflict_dialog(frame, file_path);
            }
            UIMode::ConfirmDelete { entries } => {
                self.render_delete_dialog(frame, entries);
            }
            UIMode::MkdirInput { input } => {
                self.render_mkdir_dialog(frame, input);
            }
            UIMode::RenameInput { input, .. } => {
                self.render_rename_dialog(frame, input);
            }
            UIMode::CommandLine { input } => {
                self.render_command_line(frame, input);
            }
            UIMode::ConfirmQuit => {
                self.render_quit_dialog(frame);
            }
            UIMode::Search { query } => {
                self.render_search_bar(frame, query);
            }
            UIMode::Normal => {}
        }
    }

    fn render_pane(&mut self, frame: &mut Frame, area: Rect, pane: Pane) {
        let is_active = self.active_pane == pane;
        let pane_state = match pane {
            Pane::Left => &mut self.left,
            Pane::Right => &mut self.right,
        };

        let border_style = if is_active {
            Style::default().fg(THEME.pane_active_border)
        } else {
            Style::default().fg(THEME.pane_inactive_border)
        };

        let block = Block::default()
            .title(format!(" {} ", pane_state.path.display()))
            .title_style(Style::default().fg(THEME.pane_title))
            .borders(Borders::ALL)
            .border_style(border_style);

        let items: Vec<ListItem> = pane_state
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                let is_multi_selected = pane_state.selected.contains(&i);
                let base_style = if entry.is_dir {
                    Style::default()
                        .fg(THEME.directory_fg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(THEME.file_fg)
                };
                let style = if is_multi_selected {
                    base_style.bg(THEME.selected_bg).fg(THEME.selected_fg)
                } else {
                    base_style
                };
                let marker = if is_multi_selected { "* " } else { "  " };
                let display = if entry.is_dir {
                    format!("{}{}/", marker, entry.name)
                } else {
                    format!("{}{}", marker, entry.name)
                };
                ListItem::new(display).style(style)
            })
            .collect();

        let highlight_style = if is_active {
            Style::default()
                .bg(THEME.cursor_active_bg)
                .fg(THEME.cursor_active_fg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(THEME.cursor_inactive_bg).fg(THEME.cursor_inactive_fg)
        };

        let list = List::new(items)
            .block(block)
            .highlight_style(highlight_style)
            .highlight_symbol("▶ ");

        frame.render_stateful_widget(list, area, &mut pane_state.list_state);
    }

    fn render_status_bar(&self, frame: &mut Frame, area: Rect) {
        let active_jobs = self.job_manager.active_job_count();

        let content = if let Some((msg, _)) = &self.error_message {
            format!("[Error] {}  ", msg)
        } else if active_jobs > 0 {
            // Calculate total throughput from all active jobs
            let total_throughput: u64 = self
                .job_manager
                .all_jobs()
                .iter()
                .filter(|j| matches!(j.status, JobStatus::Running { .. } | JobStatus::Visible))
                .map(|j| j.throughput.current_throughput())
                .sum();

            format!(
                "[{} job{} running @ {}/s] Press J to view",
                active_jobs,
                if active_jobs == 1 { "" } else { "s" },
                format_bytes(total_throughput)
            )
        } else {
            String::new()
        };

        let style = if self.error_message.is_some() {
            Style::default().fg(THEME.status_error_fg).bg(THEME.status_error_bg)
        } else {
            Style::default().fg(THEME.status_info_fg).bg(THEME.status_info_bg)
        };

        let paragraph = Paragraph::new(content).style(style);
        frame.render_widget(paragraph, area);
    }

    fn render_help_bar(&self, frame: &mut Frame, area: Rect) {
        let key_style = Style::default().fg(THEME.help_key_fg).bg(THEME.help_key_bg);
        let desc_style = Style::default().fg(THEME.help_desc_fg).bg(THEME.help_desc_bg);
        let sep_style = Style::default().bg(THEME.help_desc_bg);

        let shortcuts = [
            ("Ins", "Select"),
            ("F2", "Rename"),
            ("F4/e", "Edit"),
            ("F5/c", "Copy"),
            ("F6/m", "Move"),
            ("F7", "Mkdir"),
            ("F8/Del", "Delete"),
            ("H", "Hidden"),
            ("J", "Jobs"),
            ("q", "Quit"),
        ];

        let mut spans: Vec<Span> = Vec::new();
        for (i, (key, desc)) in shortcuts.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" ", sep_style));
            }
            spans.push(Span::styled(format!(" {} ", key), key_style));
            spans.push(Span::styled(format!("{} ", desc), desc_style));
        }

        // Fill remaining space with background
        let line = Line::from(spans);
        let paragraph = Paragraph::new(line).style(Style::default().bg(THEME.help_desc_bg));
        frame.render_widget(paragraph, area);
    }

    fn render_job_popup(&self, frame: &mut Frame, selected: usize) {
        let area = centered_rect(90, 70, frame.area());
        frame.render_widget(Clear, area);

        let block = Block::default()
            .title(" Jobs (J to close) ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(THEME.job_popup_border));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let jobs: Vec<&Job> = self.job_manager.all_jobs();

        if jobs.is_empty() {
            let msg = Paragraph::new("No jobs").style(Style::default().fg(THEME.job_no_jobs));
            frame.render_widget(msg, inner);
            return;
        }

        // Split into left (job list) and right (throughput chart) panes
        let h_layout = Layout::horizontal([
            Constraint::Percentage(50),
            Constraint::Percentage(50),
        ])
        .split(inner);

        let left_area = h_layout[0];
        let right_area = h_layout[1];

        // Left pane: Job list
        self.render_job_list(frame, left_area, &jobs, selected);

        // Right pane: Throughput chart for selected job
        if let Some(job) = jobs.get(selected) {
            self.render_throughput_chart(frame, right_area, job);
        }
    }

    fn render_job_list(&self, frame: &mut Frame, area: Rect, jobs: &[&Job], selected: usize) {
        let block = Block::default()
            .title(" Progress ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(THEME.pane_inactive_border));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        // Calculate layout for each job (3 lines per job + 1 for footer)
        let job_height = 3u16;
        let footer_height = 2u16;
        let available_height = inner.height.saturating_sub(footer_height);
        let max_jobs = (available_height / job_height) as usize;

        let visible_jobs: Vec<_> = jobs.iter().take(max_jobs).collect();

        let mut constraints: Vec<Constraint> = visible_jobs
            .iter()
            .map(|_| Constraint::Length(job_height))
            .collect();
        constraints.push(Constraint::Length(footer_height));
        constraints.push(Constraint::Min(0));

        let layout = Layout::vertical(constraints).split(inner);

        for (i, job) in visible_jobs.iter().enumerate() {
            let job_area = layout[i];
            let is_selected = i == selected;

            self.render_job_item(frame, job_area, job, is_selected);
        }

        // Footer
        let footer_area = layout[visible_jobs.len()];
        let footer = Paragraph::new("j/k: navigate | P: pause | K: kill | d: dismiss | Esc: close")
            .style(Style::default().fg(THEME.dialog_hint));
        frame.render_widget(footer, footer_area);
    }

    fn render_throughput_chart(&self, frame: &mut Frame, area: Rect, job: &Job) {
        let block = Block::default()
            .title(" Throughput ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(THEME.pane_inactive_border));

        let inner = block.inner(area);
        frame.render_widget(block, area);

        let history = &job.throughput.history;

        if history.is_empty() {
            let msg = Paragraph::new("Collecting data...")
                .style(Style::default().fg(THEME.job_no_jobs));
            frame.render_widget(msg, inner);
            return;
        }

        // Layout: sparkline chart + stats below
        let v_layout = Layout::vertical([
            Constraint::Min(3),    // Chart
            Constraint::Length(3), // Stats
        ])
        .split(inner);

        // Sparkline chart
        let max_throughput = history.iter().max().copied().unwrap_or(1);
        let sparkline = Sparkline::default()
            .data(history)
            .max(max_throughput)
            .style(Style::default().fg(THEME.job_gauge));
        frame.render_widget(sparkline, v_layout[0]);

        // Stats below chart
        let current = job.throughput.current_throughput();
        let avg = if !history.is_empty() {
            history.iter().sum::<u64>() / history.len() as u64
        } else {
            0
        };

        let stats = format!(
            "Current: {}/s | Avg: {}/s | Peak: {}/s",
            format_bytes(current),
            format_bytes(avg),
            format_bytes(max_throughput)
        );
        let stats_para = Paragraph::new(stats)
            .style(Style::default().fg(THEME.job_file_info));
        frame.render_widget(stats_para, v_layout[1]);
    }

    fn render_job_item(&self, frame: &mut Frame, area: Rect, job: &Job, is_selected: bool) {
        let layout = Layout::vertical([
            Constraint::Length(1), // Description
            Constraint::Length(1), // Progress bar
            Constraint::Length(1), // Current file
        ])
        .split(area);

        // Status icon and description
        let icon = match &job.status {
            JobStatus::Running { .. } | JobStatus::Visible => "●",
            JobStatus::Paused => "⏸",
            JobStatus::Completed => "✓",
            JobStatus::Failed(_) => "✗",
            JobStatus::Cancelled => "○",
        };

        let selector = if is_selected { "▶ " } else { "  " };
        let desc_style = if is_selected {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };

        let desc_line = format!("{}{} {}", selector, icon, job.description);
        let desc = Paragraph::new(desc_line).style(desc_style);
        frame.render_widget(desc, layout[0]);

        // Progress bar or status message
        match &job.status {
            JobStatus::Running { .. } | JobStatus::Visible => {
                let ratio = if job.progress.total_bytes > 0 {
                    job.progress.processed_bytes as f64 / job.progress.total_bytes as f64
                } else {
                    0.0
                };

                let label = format!(
                    "{}% ({}/{})",
                    (ratio * 100.0) as u32,
                    format_bytes(job.progress.processed_bytes),
                    format_bytes(job.progress.total_bytes)
                );

                let gauge = Gauge::default()
                    .gauge_style(Style::default().fg(THEME.job_gauge))
                    .ratio(ratio.min(1.0))
                    .label(Span::styled(label, Style::default().fg(THEME.cursor_active_fg)));
                frame.render_widget(gauge, layout[1]);

                // Current file
                if let Some(file) = &job.progress.current_file {
                    let file_info = format!(
                        "  {} ({}/{})",
                        file, job.progress.files_processed, job.progress.total_files
                    );
                    let file_para =
                        Paragraph::new(file_info).style(Style::default().fg(THEME.job_file_info));
                    frame.render_widget(file_para, layout[2]);
                }
            }
            JobStatus::Paused => {
                let ratio = if job.progress.total_bytes > 0 {
                    job.progress.processed_bytes as f64 / job.progress.total_bytes as f64
                } else {
                    0.0
                };

                let label = format!(
                    "PAUSED {}% ({}/{})",
                    (ratio * 100.0) as u32,
                    format_bytes(job.progress.processed_bytes),
                    format_bytes(job.progress.total_bytes)
                );

                let gauge = Gauge::default()
                    .gauge_style(Style::default().fg(THEME.dialog_warning_text))
                    .ratio(ratio.min(1.0))
                    .label(Span::styled(label, Style::default().fg(THEME.cursor_active_fg)));
                frame.render_widget(gauge, layout[1]);

                // Current file
                if let Some(file) = &job.progress.current_file {
                    let file_info = format!(
                        "  {} ({}/{})",
                        file, job.progress.files_processed, job.progress.total_files
                    );
                    let file_para =
                        Paragraph::new(file_info).style(Style::default().fg(THEME.job_file_info));
                    frame.render_widget(file_para, layout[2]);
                }
            }
            JobStatus::Completed => {
                let msg = Paragraph::new("  Completed").style(Style::default().fg(THEME.job_completed));
                frame.render_widget(msg, layout[1]);
            }
            JobStatus::Failed(err) => {
                let msg =
                    Paragraph::new(format!("  Error: {}", err)).style(Style::default().fg(THEME.job_error));
                frame.render_widget(msg, layout[1]);
            }
            JobStatus::Cancelled => {
                let msg =
                    Paragraph::new("  Cancelled").style(Style::default().fg(THEME.job_cancelled));
                frame.render_widget(msg, layout[1]);
            }
        }
    }

    fn render_conflict_dialog(&self, frame: &mut Frame, file_path: &Path) {
        let area = centered_rect(55, 30, frame.area());
        let inner = render_dialog_frame(frame, area, "File Exists", THEME.dialog_warning_border);

        let file_name = file_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();

        let layout = Layout::vertical([
            Constraint::Length(1), // spacer
            Constraint::Length(1), // filename
            Constraint::Length(1), // message
            Constraint::Length(1), // spacer
            Constraint::Length(1), // buttons row 1
            Constraint::Length(1), // buttons row 2
            Constraint::Min(0),
        ])
        .split(inner);

        let filename = Paragraph::new(format!("\"{}\"", file_name))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(filename, layout[1]);

        let msg = Paragraph::new("already exists. What do you want to do?")
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(msg, layout[2]);

        // Buttons row 1
        let btn_layout1 = Layout::horizontal([
            Constraint::Percentage(10),
            Constraint::Percentage(26),
            Constraint::Percentage(4),
            Constraint::Percentage(26),
            Constraint::Percentage(4),
            Constraint::Percentage(26),
            Constraint::Percentage(4),
        ])
        .split(layout[4]);

        let overwrite = Paragraph::new(" [O]verwrite ")
            .style(Style::default().fg(THEME.dialog_button_fg).bg(THEME.dialog_button_bg))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(overwrite, btn_layout1[1]);

        let skip = Paragraph::new(" [S]kip ")
            .style(Style::default().fg(THEME.dialog_button_fg).bg(THEME.dialog_button_bg))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(skip, btn_layout1[3]);

        let all = Paragraph::new(" [A]ll ")
            .style(Style::default().fg(THEME.dialog_button_fg).bg(THEME.dialog_button_bg))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(all, btn_layout1[5]);

        // Buttons row 2
        let btn_layout2 = Layout::horizontal([
            Constraint::Percentage(20),
            Constraint::Percentage(26),
            Constraint::Percentage(8),
            Constraint::Percentage(26),
            Constraint::Percentage(20),
        ])
        .split(layout[5]);

        let no_all = Paragraph::new(" [N]o all ")
            .style(Style::default().fg(THEME.dialog_button_fg).bg(THEME.dialog_button_bg))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(no_all, btn_layout2[1]);

        let cancel = Paragraph::new(" [Esc] Cancel ")
            .style(Style::default().fg(THEME.dialog_button_fg).bg(THEME.dialog_button_bg))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(cancel, btn_layout2[3]);
    }

    fn render_delete_dialog(&self, frame: &mut Frame, entries: &[Entry]) {
        let area = centered_rect(50, 45, frame.area());
        let inner = render_dialog_frame(frame, area, "Confirm Delete", THEME.dialog_delete_border);

        // Build the message
        let has_dirs = entries.iter().any(|e| e.is_dir);
        let count = entries.len();

        // Check for conflicts with active jobs
        let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
        let has_job_conflict = self.job_manager.paths_conflict_with_active_jobs(&paths);

        // Calculate content layout
        let content_layout = Layout::vertical([
            Constraint::Length(1), // spacer
            Constraint::Min(3),    // message content
            Constraint::Length(1), // dir warning (if any)
            Constraint::Length(1), // job conflict warning (if any)
            Constraint::Length(1), // spacer
            Constraint::Length(1), // buttons
            Constraint::Length(1), // spacer
        ])
        .split(inner);

        // Message
        let mut lines = Vec::new();
        if count == 1 {
            let entry = &entries[0];
            if entry.is_dir {
                lines.push(format!("Delete directory \"{}\"", entry.name));
                lines.push("and all its contents?".to_owned());
            } else {
                lines.push(format!("Delete file \"{}\"?", entry.name));
            }
        } else {
            lines.push(format!("Delete {} items?", count));
            lines.push(String::new());
            for entry in entries.iter().take(4) {
                let prefix = if entry.is_dir { "📁 " } else { "   " };
                lines.push(format!("{}{}", prefix, entry.name));
            }
            if count > 4 {
                lines.push(format!("   ... and {} more", count - 4));
            }
        }

        let msg = Paragraph::new(lines.join("\n"))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(msg, content_layout[1]);

        // Warning for directories
        if has_dirs {
            let warning = Paragraph::new("⚠ Directories will be deleted recursively!")
                .style(Style::default().fg(THEME.dialog_warning_text))
                .alignment(ratatui::layout::Alignment::Center);
            frame.render_widget(warning, content_layout[2]);
        }

        // Warning for job conflicts
        if has_job_conflict {
            let warning = Paragraph::new("⚠ CONFLICTS WITH ACTIVE COPY/MOVE JOB!")
                .style(Style::default().fg(THEME.status_error_fg))
                .alignment(ratatui::layout::Alignment::Center);
            frame.render_widget(warning, content_layout[3]);
        }

        // Buttons
        render_yes_no_buttons(frame, content_layout[5]);
    }

    fn render_mkdir_dialog(&self, frame: &mut Frame, input: &str) {
        let area = centered_rect(50, 20, frame.area());
        let inner = render_dialog_frame(frame, area, "Create Directory", THEME.dialog_border);

        let layout = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

        let label = Paragraph::new("Enter directory name:");
        frame.render_widget(label, layout[1]);

        let input_display = format!("{}█", input);
        let input_para = Paragraph::new(input_display)
            .style(Style::default().fg(THEME.dialog_input_fg).bg(THEME.dialog_input_bg));
        frame.render_widget(input_para, layout[2]);

        let hint = Paragraph::new("Enter to confirm, Esc to cancel")
            .style(Style::default().fg(THEME.dialog_hint));
        frame.render_widget(hint, layout[4]);
    }

    fn render_rename_dialog(&self, frame: &mut Frame, input: &str) {
        let area = centered_rect(50, 20, frame.area());
        let inner = render_dialog_frame(frame, area, "Rename", THEME.dialog_border);

        let layout = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

        let label = Paragraph::new("Enter new name:");
        frame.render_widget(label, layout[1]);

        let input_display = format!("{}█", input);
        let input_para = Paragraph::new(input_display)
            .style(Style::default().fg(THEME.dialog_input_fg).bg(THEME.dialog_input_bg));
        frame.render_widget(input_para, layout[2]);

        let hint = Paragraph::new("Enter to confirm, Esc to cancel")
            .style(Style::default().fg(THEME.dialog_hint));
        frame.render_widget(hint, layout[4]);
    }

    fn render_command_line(&self, frame: &mut Frame, input: &str) {
        // Render at the very bottom of the screen
        let area = Rect {
            x: 0,
            y: frame.area().height.saturating_sub(1),
            width: frame.area().width,
            height: 1,
        };

        frame.render_widget(Clear, area);

        let pane_path = match self.active_pane {
            Pane::Left => &self.left.path,
            Pane::Right => &self.right.path,
        };

        let prompt = format!("{}$ {}█", pane_path.display(), input);
        let line = Paragraph::new(prompt)
            .style(Style::default().fg(THEME.dialog_input_fg).bg(THEME.dialog_input_bg));
        frame.render_widget(line, area);
    }

    fn render_search_bar(&self, frame: &mut Frame, query: &str) {
        // Render at the very bottom of the screen
        let area = Rect {
            x: 0,
            y: frame.area().height.saturating_sub(1),
            width: frame.area().width,
            height: 1,
        };

        frame.render_widget(Clear, area);

        let prompt = format!("Search: {}█  (Ctrl+S: next, Esc: cancel)", query);
        let line = Paragraph::new(prompt)
            .style(Style::default().fg(THEME.dialog_input_fg).bg(THEME.dialog_input_bg));
        frame.render_widget(line, area);
    }

    fn render_quit_dialog(&self, frame: &mut Frame) {
        let area = centered_rect(40, 25, frame.area());
        let inner = render_dialog_frame(frame, area, "Quit", THEME.dialog_warning_border);

        let active_jobs = self.job_manager.active_job_count();

        let layout = Layout::vertical([
            Constraint::Length(1), // spacer
            Constraint::Length(1), // job count
            Constraint::Length(1), // question
            Constraint::Length(1), // spacer
            Constraint::Length(1), // buttons
            Constraint::Min(0),
        ])
        .split(inner);

        let msg = format!(
            "{} job{} still running.",
            active_jobs,
            if active_jobs == 1 { " is" } else { "s are" }
        );
        let warning = Paragraph::new(msg)
            .style(Style::default().fg(THEME.dialog_warning_text))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(warning, layout[1]);

        let confirm = Paragraph::new("Kill all jobs and quit?")
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(confirm, layout[2]);

        // Buttons
        render_yes_no_buttons(frame, layout[4]);
    }
}

// ============================================================================
// Helpers
// ============================================================================

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;

    if bytes >= GB {
        format!("{:.1}GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.1}MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.1}KB", bytes as f64 / KB as f64)
    } else {
        format!("{}B", bytes)
    }
}
