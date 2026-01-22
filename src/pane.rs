use std::{
    collections::HashSet,
    path::PathBuf,
};

use ratatui::widgets::ListState;

#[derive(Clone)]
pub struct Entry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Default, PartialEq, Clone, Copy)]
pub enum Pane {
    #[default]
    Left,
    Right,
}

pub struct PaneState {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
    pub list_state: ListState,
    pub selected: HashSet<usize>,
    pub show_hidden: bool,
}

impl PaneState {
    pub fn new(path: PathBuf) -> std::io::Result<Self> {
        let mut state = Self {
            path,
            entries: Vec::new(),
            list_state: ListState::default(),
            selected: HashSet::new(),
            show_hidden: false,
        };
        state.load_entries()?;
        if !state.entries.is_empty() {
            state.list_state.select(Some(0));
        }
        Ok(state)
    }

    pub fn load_entries(&mut self) -> std::io::Result<()> {
        self.entries.clear();
        self.selected.clear();

        if let Some(parent) = self.path.parent() {
            self.entries.push(Entry {
                name: "..".to_owned(),
                path: parent.to_path_buf(),
                is_dir: true,
            });
        }

        let mut entries: Vec<Entry> = std::fs::read_dir(&self.path)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                if self.show_hidden {
                    true
                } else {
                    !e.file_name().to_string_lossy().starts_with('.')
                }
            })
            .map(|e| {
                let is_dir = e.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                Entry {
                    name: e.file_name().to_string_lossy().into_owned(),
                    path: e.path(),
                    is_dir,
                }
            })
            .collect();

        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        self.entries.extend(entries);
        Ok(())
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

                    let msg = if e.kind() == std::io::ErrorKind::PermissionDenied {
                        "Permission denied".to_owned()
                    } else {
                        format!("Cannot open directory: {}", e)
                    };
                    return Err(msg);
                }

                self.list_state.select(Some(0));
            }
        }
        Ok(())
    }
}
