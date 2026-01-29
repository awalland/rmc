//! Input handling for the file manager
//!
//! This module contains all keyboard and mouse event handlers.

use std::{
    env,
    path::{Path, PathBuf},
    time::Instant,
};

use crossterm::{
    event::{KeyCode, KeyModifiers, MouseButton, MouseEventKind},
    terminal::{EnterAlternateScreen, LeaveAlternateScreen},
    ExecutableCommand,
};
use ratatui::DefaultTerminal;

use crate::{
    dialog::{handle_yes_no_keys, DialogResult},
    job::{ConflictResolution, JobId, JobStatus, JobType},
    pane::{Entry, Pane},
    util::{PAGE_SCROLL_SIZE, RENAME_DIALOG_TIMEOUT_SECS},
    viewer::{FileViewer, ViewMode},
    App, UIMode,
};

impl App {
    /// Main event handler - dispatches to mode-specific handlers
    pub fn handle_key_event(
        &mut self,
        key: KeyCode,
        modifiers: KeyModifiers,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        // Clear error on any key press
        self.error_message = None;

        // Use mem::take pattern for FileViewer to avoid cloning large data
        // For other modes, we can work with references or small clones
        match &self.ui_mode {
            UIMode::Normal => {
                self.handle_normal_mode(key, modifiers, terminal)?;
            }
            UIMode::JobList { selected } => {
                let selected = *selected;
                self.handle_job_list_mode(key, selected);
            }
            UIMode::ConfirmOverwrite { job_id, .. } => {
                let job_id = *job_id;
                self.handle_confirm_overwrite(key, job_id);
            }
            UIMode::ConfirmDelete { .. } => {
                // Take the entries out temporarily to avoid borrow issues
                if let UIMode::ConfirmDelete { entries, .. } =
                    std::mem::replace(&mut self.ui_mode, UIMode::Normal)
                {
                    self.handle_confirm_delete(key, entries);
                }
            }
            UIMode::MkdirInput { input } => {
                let input = input.clone();
                self.handle_mkdir_input(key, input);
            }
            UIMode::RenameInput { original, input } => {
                let original = original.clone();
                let input = input.clone();
                self.handle_rename_input(key, original, input);
            }
            UIMode::RenameInProgress { job_id, .. } => {
                let job_id = *job_id;
                self.handle_rename_in_progress(key, job_id);
            }
            UIMode::CommandLine { input } => {
                let input = input.clone();
                self.handle_command_line(key, input, terminal)?;
            }
            UIMode::ConfirmQuit => {
                self.handle_confirm_quit(key);
            }
            UIMode::Search { query } => {
                let query = query.clone();
                self.handle_search(key, modifiers, query);
            }
            UIMode::FileViewer { .. } => {
                // Use take pattern to avoid cloning the potentially huge FileViewer
                if let UIMode::FileViewer { viewer } =
                    std::mem::replace(&mut self.ui_mode, UIMode::Normal)
                {
                    self.handle_file_viewer(key, viewer);
                }
            }
        }
        Ok(())
    }

    pub fn handle_normal_mode(
        &mut self,
        key: KeyCode,
        modifiers: KeyModifiers,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        // Handle Ctrl+S for search
        if modifiers.contains(KeyModifiers::CONTROL) && key == KeyCode::Char('s') {
            self.ui_mode = UIMode::Search {
                query: String::new(),
            };
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
            KeyCode::PageUp => self.active_pane_mut().page_up(PAGE_SCROLL_SIZE),
            KeyCode::PageDown => self.active_pane_mut().page_down(PAGE_SCROLL_SIZE),
            KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                if let Err(msg) = self.active_pane_mut().enter_selected() {
                    self.error_message = Some((msg, Instant::now()));
                }
            }
            KeyCode::Left | KeyCode::Char('h') => {
                self.navigate_to_parent();
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
            KeyCode::Char('*') => {
                self.active_pane_mut().select_all();
            }
            KeyCode::Delete | KeyCode::F(8) => {
                self.initiate_delete();
            }
            KeyCode::F(3) => {
                self.view_selected();
            }
            KeyCode::Char('e') | KeyCode::F(4) => {
                if let Err(msg) = self.edit_selected(terminal) {
                    self.error_message = Some((msg, Instant::now()));
                }
            }
            KeyCode::Char('H') => {
                self.active_pane_mut().toggle_hidden();
            }
            KeyCode::Char('S') => {
                self.active_pane_mut().cycle_size_mode();
            }
            KeyCode::F(7) => {
                self.ui_mode = UIMode::MkdirInput {
                    input: String::new(),
                };
            }
            KeyCode::F(2) => {
                self.initiate_rename();
            }
            KeyCode::Char('U') => {
                self.swap_panes();
            }
            KeyCode::Char(':') => {
                self.ui_mode = UIMode::CommandLine {
                    input: String::new(),
                };
            }
            _ => {}
        }
        Ok(())
    }

    fn navigate_to_parent(&mut self) {
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

    pub fn handle_job_list_mode(&mut self, key: KeyCode, selected: usize) {
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

    pub fn handle_confirm_overwrite(&mut self, key: KeyCode, job_id: JobId) {
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

    pub fn initiate_delete(&mut self) {
        let pane = self.active_pane();
        let entries: Vec<Entry> = pane
            .selected_entries()
            .into_iter()
            .filter(|e| e.name != "..")
            .cloned()
            .collect();

        if entries.is_empty() {
            return;
        }

        // Canonicalize paths once and check for conflicts
        let paths_canonical: Vec<PathBuf> = entries
            .iter()
            .map(|e| e.path.canonicalize().unwrap_or_else(|_| e.path.clone()))
            .collect();
        let has_job_conflict = self
            .job_manager
            .paths_conflict_with_active_jobs(&paths_canonical);

        self.ui_mode = UIMode::ConfirmDelete {
            entries,
            has_job_conflict,
        };
    }

    pub fn handle_confirm_delete(&mut self, key: KeyCode, entries: Vec<Entry>) {
        match handle_yes_no_keys(key) {
            DialogResult::Accept => {
                // Get parent directory for refresh after deletion
                let parent_dir = self.active_pane().path.clone();

                // Collect paths to delete
                let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();

                // Start background delete job
                self.job_manager.start_delete_job(paths, parent_dir);

                // Clear selection
                self.active_pane_mut().selected.clear();
                self.ui_mode = UIMode::Normal;
            }
            DialogResult::Reject => {
                self.ui_mode = UIMode::Normal;
            }
            DialogResult::Pending => {
                // Put the entries back
                self.ui_mode = UIMode::ConfirmDelete {
                    entries,
                    has_job_conflict: false, // Recalculate if needed
                };
            }
        }
    }

    pub fn handle_confirm_quit(&mut self, key: KeyCode) {
        match handle_yes_no_keys(key) {
            DialogResult::Accept => {
                // Cancel all active jobs
                let job_ids: Vec<_> = self
                    .job_manager
                    .all_jobs()
                    .iter()
                    .filter(|j| {
                        matches!(j.status, JobStatus::Running { .. } | JobStatus::Visible)
                    })
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

    pub fn handle_search(&mut self, key: KeyCode, modifiers: KeyModifiers, mut query: String) {
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

    pub fn handle_mkdir_input(&mut self, key: KeyCode, mut input: String) {
        match key {
            KeyCode::Enter => {
                if !input.is_empty() {
                    let new_dir = self.active_pane().path.join(&input);

                    match std::fs::create_dir(&new_dir) {
                        Ok(()) => {
                            // Refresh the pane
                            let _ = self.active_pane_mut().load_entries();
                        }
                        Err(e) => {
                            self.error_message =
                                Some((format!("mkdir failed: {}", e), Instant::now()));
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

    pub fn initiate_rename(&mut self) {
        let entry = match self.active_pane().selected_entry() {
            Some(e) if e.name != ".." => e.clone(),
            _ => return,
        };

        self.ui_mode = UIMode::RenameInput {
            original: entry.path,
            input: entry.name,
        };
    }

    pub fn handle_rename_input(&mut self, key: KeyCode, original: PathBuf, mut input: String) {
        match key {
            KeyCode::Enter => {
                if !input.is_empty() {
                    let new_path = original.parent().unwrap_or(Path::new(".")).join(&input);

                    if new_path != original {
                        // Get parent directory for refresh
                        let parent_dir = self.active_pane().path.clone();

                        // Start async rename job
                        let job_id =
                            self.job_manager
                                .start_rename_job(original.clone(), new_path, parent_dir);

                        let original_name = original
                            .file_name()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .into_owned();

                        self.ui_mode = UIMode::RenameInProgress {
                            job_id,
                            started_at: Instant::now(),
                            original_name,
                            new_name: input,
                        };
                        return;
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

    pub fn handle_rename_in_progress(&mut self, key: KeyCode, job_id: JobId) {
        // Only handle Escape to cancel
        if key == KeyCode::Esc {
            self.job_manager.cancel_job(job_id);
            self.ui_mode = UIMode::Normal;
        }
    }

    pub fn handle_command_line(
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
            let base = &self.active_pane().path;
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

    pub fn execute_command(
        &mut self,
        command: &str,
        terminal: &mut DefaultTerminal,
    ) -> std::io::Result<()> {
        let command = command.trim();

        // Handle cd specially
        if command == "cd" || command.starts_with("cd ") {
            let path_str = if command == "cd" {
                ""
            } else {
                command.strip_prefix("cd ").unwrap_or("").trim()
            };

            let current_path = self.active_pane().path.clone();

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
                self.error_message = Some((
                    format!("cd: not a directory: {}", path_str),
                    Instant::now(),
                ));
            }

            return Ok(());
        }

        // For other commands, execute in shell
        let pane_path = self.active_pane().path.clone();

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

    pub fn handle_mouse(&mut self, kind: MouseEventKind, col: u16, row: u16) {
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

    pub fn edit_selected(&mut self, terminal: &mut DefaultTerminal) -> Result<(), String> {
        let entry = match self.active_pane().selected_entry() {
            Some(e) if e.name != ".." => e.clone(),
            _ => return Ok(()),
        };

        let editor = env::var("EDITOR").unwrap_or_else(|_| "vi".to_owned());

        // Leave alternate screen and disable raw mode
        let mut stdout = std::io::stdout();
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = stdout.execute(LeaveAlternateScreen);

        // Run the editor
        let status = std::process::Command::new(&editor).arg(&entry.path).status();

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

    pub fn view_selected(&mut self) {
        let entry = match self.active_pane().selected_entry() {
            Some(e) if e.name != ".." && !e.is_dir => e.clone(),
            _ => return,
        };

        let viewer = FileViewer::new(entry.path);
        self.ui_mode = UIMode::FileViewer {
            viewer: Box::new(viewer),
        };
    }

    pub fn handle_file_viewer(&mut self, key: KeyCode, mut viewer: Box<FileViewer>) {
        // Calculate visible height (will be set properly during render, use estimate)
        let visible_height = 20usize;

        match key {
            // Exit viewer
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::F(3) => {
                self.ui_mode = UIMode::Normal;
                return;
            }

            // Scrolling
            KeyCode::Up | KeyCode::Char('k') => viewer.scroll_up(1),
            KeyCode::Down | KeyCode::Char('j') => viewer.scroll_down(1, visible_height),
            KeyCode::PageUp => viewer.scroll_up(visible_height),
            KeyCode::PageDown => viewer.scroll_down(visible_height, visible_height),
            KeyCode::Home | KeyCode::Char('g') => viewer.scroll_to_top(),
            KeyCode::End | KeyCode::Char('G') => viewer.scroll_to_bottom(visible_height),

            // View mode switches
            KeyCode::Char('t') => viewer.set_mode(ViewMode::Text),
            KeyCode::Char('x') => viewer.set_mode(ViewMode::Hex),
            KeyCode::Char('d') => viewer.set_mode(ViewMode::Disasm),
            KeyCode::Char('s') => viewer.set_mode(ViewMode::Strings),
            KeyCode::Char('h') => viewer.set_mode(ViewMode::ElfHeader),
            KeyCode::Char('S') => viewer.set_mode(ViewMode::Sections),
            KeyCode::Char('y') => viewer.set_mode(ViewMode::Symbols),
            KeyCode::Char('l') => viewer.set_mode(ViewMode::Ldd),
            KeyCode::Char('i') => viewer.set_mode(ViewMode::FileInfo),
            KeyCode::Char('e') => viewer.set_mode(ViewMode::Exif),
            KeyCode::Char('a') => viewer.set_mode(ViewMode::Archive),
            KeyCode::Char('J') => viewer.set_mode(ViewMode::Json),

            _ => {}
        }

        self.ui_mode = UIMode::FileViewer { viewer };
    }

    // ========================================================================
    // Helper methods for updating rename progress
    // ========================================================================

    pub fn check_rename_progress(&mut self) {
        if let UIMode::RenameInProgress {
            job_id,
            started_at,
            ..
        } = &self.ui_mode
        {
            let job_id = *job_id;
            let elapsed = started_at.elapsed();

            // Check if job completed (or failed/cancelled)
            let job_status = self.job_manager.get_job(job_id).map(|j| j.status.clone());

            match job_status {
                Some(JobStatus::Completed) => {
                    self.job_manager.dismiss_job(job_id);
                    self.ui_mode = UIMode::Normal;
                }
                Some(JobStatus::Failed(e)) => {
                    self.error_message = Some((format!("Rename failed: {}", e), Instant::now()));
                    self.job_manager.dismiss_job(job_id);
                    self.ui_mode = UIMode::Normal;
                }
                Some(JobStatus::Cancelled) => {
                    self.job_manager.dismiss_job(job_id);
                    self.ui_mode = UIMode::Normal;
                }
                Some(_) => {
                    // Job still running - close dialog after timeout
                    if elapsed >= std::time::Duration::from_secs(RENAME_DIALOG_TIMEOUT_SECS) {
                        // Job takes too long, background it (leave in job list)
                        self.ui_mode = UIMode::Normal;
                    }
                }
                None => {
                    // Job doesn't exist anymore, close dialog
                    self.ui_mode = UIMode::Normal;
                }
            }
        }
    }
}
