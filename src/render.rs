//! Rendering for the file manager
//!
//! This module contains all UI rendering functions.

use std::{path::Path, time::Duration};

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph, Sparkline, Wrap},
    Frame,
};

use crate::{
    dialog::{centered_rect, render_dialog_frame, render_yes_no_buttons},
    job::{Job, JobStatus},
    pane::{Entry, Pane, SizeDisplayMode},
    theme::THEME,
    util::{format_bytes, format_size},
    viewer::FileViewer,
    App, UIMode,
};

impl App {
    pub fn render(&mut self, frame: &mut Frame) {
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
            UIMode::ConfirmDelete {
                entries,
                has_job_conflict,
            } => {
                self.render_delete_dialog(frame, entries, *has_job_conflict);
            }
            UIMode::MkdirInput { input } => {
                self.render_mkdir_dialog(frame, input);
            }
            UIMode::RenameInput { input, .. } => {
                self.render_rename_dialog(frame, input);
            }
            UIMode::RenameInProgress {
                started_at,
                original_name,
                new_name,
                ..
            } => {
                self.render_rename_progress(frame, *started_at, original_name, new_name);
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
            UIMode::FileViewer { viewer } => {
                self.render_file_viewer(frame, viewer);
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

        // Build title with loading/calculating indicators
        let mut title = format!(" {} ", pane_state.path.display());
        if pane_state.is_loading() {
            title.push_str("[Loading...] ");
        } else if pane_state.is_calculating_sizes() {
            title.push_str("[Calculating...] ");
        }

        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(THEME.pane_title))
            .borders(Borders::ALL)
            .border_style(border_style);

        // Calculate available width for size column
        let inner_width = area.width.saturating_sub(2) as usize; // -2 for borders
        let size_mode = pane_state.size_mode;

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
                let name_with_marker = if entry.is_dir {
                    format!("{}{}/", marker, entry.name)
                } else {
                    format!("{}{}", marker, entry.name)
                };

                // Format size if available and mode is not None
                let display = if size_mode != SizeDisplayMode::None {
                    let size_str = match entry.size {
                        Some(size) => format_size(size),
                        // Only show "..." for directories in Full mode while calculating
                        None if entry.is_dir && size_mode == SizeDisplayMode::Full => {
                            "...".to_owned()
                        }
                        None => String::new(),
                    };
                    // Right-align size with 8 char width
                    let size_width = 8;
                    let name_width = inner_width.saturating_sub(size_width + 4); // 4 for highlight symbol
                    let truncated_name = if name_with_marker.len() > name_width {
                        format!("{}…", &name_with_marker[..name_width.saturating_sub(1)])
                    } else {
                        name_with_marker
                    };
                    format!(
                        "{:<width$}{:>8}",
                        truncated_name,
                        size_str,
                        width = name_width
                    )
                } else {
                    name_with_marker
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
            Style::default()
                .bg(THEME.cursor_inactive_bg)
                .fg(THEME.cursor_inactive_fg)
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
            Style::default()
                .fg(THEME.status_error_fg)
                .bg(THEME.status_error_bg)
        } else {
            Style::default()
                .fg(THEME.status_info_fg)
                .bg(THEME.status_info_bg)
        };

        let paragraph = Paragraph::new(content).style(style);
        frame.render_widget(paragraph, area);
    }

    fn render_help_bar(&self, frame: &mut Frame, area: Rect) {
        let key_style = Style::default()
            .fg(THEME.help_key_fg)
            .bg(THEME.help_key_bg);
        let desc_style = Style::default()
            .fg(THEME.help_desc_fg)
            .bg(THEME.help_desc_bg);
        let sep_style = Style::default().bg(THEME.help_desc_bg);

        let shortcuts = [
            ("Ins", "Select"),
            ("F2", "Rename"),
            ("F3", "View"),
            ("F4/e", "Edit"),
            ("F5/c", "Copy"),
            ("F6/m", "Move"),
            ("F7", "Mkdir"),
            ("F8/Del", "Delete"),
            ("H", "Hidden"),
            ("S", "Sizes"),
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
        let h_layout =
            Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)])
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
        let footer =
            Paragraph::new("j/k: navigate | P: pause | K: kill | d: dismiss | Esc: close")
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

        let history = job.throughput.history_slice();

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
            .data(&history)
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
        let stats_para = Paragraph::new(stats).style(Style::default().fg(THEME.job_file_info));
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
                self.render_progress_gauge(frame, layout[1], job, THEME.job_gauge);
                self.render_current_file(frame, layout[2], job);
            }
            JobStatus::Paused => {
                self.render_paused_gauge(frame, layout[1], job);
                self.render_current_file(frame, layout[2], job);
            }
            JobStatus::Completed => {
                let msg =
                    Paragraph::new("  Completed").style(Style::default().fg(THEME.job_completed));
                frame.render_widget(msg, layout[1]);
            }
            JobStatus::Failed(err) => {
                let msg = Paragraph::new(format!("  Error: {}", err))
                    .style(Style::default().fg(THEME.job_error));
                frame.render_widget(msg, layout[1]);
            }
            JobStatus::Cancelled => {
                let msg =
                    Paragraph::new("  Cancelled").style(Style::default().fg(THEME.job_cancelled));
                frame.render_widget(msg, layout[1]);
            }
        }
    }

    fn render_progress_gauge(&self, frame: &mut Frame, area: Rect, job: &Job, color: ratatui::style::Color) {
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
            .gauge_style(Style::default().fg(color))
            .ratio(ratio.min(1.0))
            .label(Span::styled(
                label,
                Style::default().fg(THEME.cursor_active_fg),
            ));
        frame.render_widget(gauge, area);
    }

    fn render_paused_gauge(&self, frame: &mut Frame, area: Rect, job: &Job) {
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
            .label(Span::styled(
                label,
                Style::default().fg(THEME.cursor_active_fg),
            ));
        frame.render_widget(gauge, area);
    }

    fn render_current_file(&self, frame: &mut Frame, area: Rect, job: &Job) {
        if let Some(file) = &job.progress.current_file {
            let file_info = format!(
                "  {} ({}/{})",
                file, job.progress.files_processed, job.progress.total_files
            );
            let file_para = Paragraph::new(file_info).style(Style::default().fg(THEME.job_file_info));
            frame.render_widget(file_para, area);
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
            .style(
                Style::default()
                    .fg(THEME.dialog_button_fg)
                    .bg(THEME.dialog_button_bg),
            )
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(overwrite, btn_layout1[1]);

        let skip = Paragraph::new(" [S]kip ")
            .style(
                Style::default()
                    .fg(THEME.dialog_button_fg)
                    .bg(THEME.dialog_button_bg),
            )
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(skip, btn_layout1[3]);

        let all = Paragraph::new(" [A]ll ")
            .style(
                Style::default()
                    .fg(THEME.dialog_button_fg)
                    .bg(THEME.dialog_button_bg),
            )
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
            .style(
                Style::default()
                    .fg(THEME.dialog_button_fg)
                    .bg(THEME.dialog_button_bg),
            )
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(no_all, btn_layout2[1]);

        let cancel = Paragraph::new(" [Esc] Cancel ")
            .style(
                Style::default()
                    .fg(THEME.dialog_button_fg)
                    .bg(THEME.dialog_button_bg),
            )
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(cancel, btn_layout2[3]);
    }

    fn render_delete_dialog(&self, frame: &mut Frame, entries: &[Entry], has_job_conflict: bool) {
        let area = centered_rect(50, 45, frame.area());
        let inner = render_dialog_frame(frame, area, "Confirm Delete", THEME.dialog_delete_border);

        // Build the message
        let has_dirs = entries.iter().any(|e| e.is_dir);
        let count = entries.len();

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

        let msg =
            Paragraph::new(lines.join("\n")).alignment(ratatui::layout::Alignment::Center);
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
        let input_para = Paragraph::new(input_display).style(
            Style::default()
                .fg(THEME.dialog_input_fg)
                .bg(THEME.dialog_input_bg),
        );
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
        let input_para = Paragraph::new(input_display).style(
            Style::default()
                .fg(THEME.dialog_input_fg)
                .bg(THEME.dialog_input_bg),
        );
        frame.render_widget(input_para, layout[2]);

        let hint = Paragraph::new("Enter to confirm, Esc to cancel")
            .style(Style::default().fg(THEME.dialog_hint));
        frame.render_widget(hint, layout[4]);
    }

    fn render_rename_progress(
        &self,
        frame: &mut Frame,
        started_at: std::time::Instant,
        original_name: &str,
        new_name: &str,
    ) {
        let area = centered_rect(50, 20, frame.area());
        let inner = render_dialog_frame(frame, area, "Renaming", THEME.dialog_border);

        let elapsed = started_at.elapsed();

        let layout = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

        // Show what's being renamed
        let msg = format!("'{}' → '{}'", original_name, new_name);
        let label = Paragraph::new(msg).alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(label, layout[1]);

        // Show progress message
        let progress_msg = if elapsed < Duration::from_secs(1) {
            "Renaming...".to_owned()
        } else {
            // Show countdown
            let remaining = crate::util::RENAME_DIALOG_TIMEOUT_SECS.saturating_sub(elapsed.as_secs());
            format!("Backgrounding in {}...", remaining)
        };

        let progress = Paragraph::new(progress_msg)
            .style(Style::default().fg(THEME.dialog_warning_text))
            .alignment(ratatui::layout::Alignment::Center);
        frame.render_widget(progress, layout[2]);

        let hint = Paragraph::new("Esc to cancel")
            .style(Style::default().fg(THEME.dialog_hint))
            .alignment(ratatui::layout::Alignment::Center);
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

        let pane_path = &self.active_pane().path;

        let prompt = format!("{}$ {}█", pane_path.display(), input);
        let line = Paragraph::new(prompt).style(
            Style::default()
                .fg(THEME.dialog_input_fg)
                .bg(THEME.dialog_input_bg),
        );
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
        let line = Paragraph::new(prompt).style(
            Style::default()
                .fg(THEME.dialog_input_fg)
                .bg(THEME.dialog_input_bg),
        );
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

    fn render_file_viewer(&self, frame: &mut Frame, viewer: &FileViewer) {
        // Full-screen viewer
        let area = frame.area();
        frame.render_widget(Clear, area);

        // Layout: title bar + content + status/help bar
        let layout = Layout::vertical([
            Constraint::Length(1), // Title bar
            Constraint::Min(0),    // Content
            Constraint::Length(1), // Mode selector
            Constraint::Length(1), // Help bar
        ])
        .split(area);

        // Title bar
        let file_name = viewer
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy();
        let size_info = if viewer.truncated {
            format!(
                "{} of {} TRUNCATED",
                format_bytes(viewer.file_size() as u64),
                format_bytes(viewer.original_size)
            )
        } else {
            format_bytes(viewer.original_size)
        };
        let title = format!(
            " {} - {} ({}) ",
            file_name,
            viewer.mode.label(),
            size_info
        );
        let title_style = if viewer.truncated {
            Style::default()
                .fg(THEME.cursor_active_fg)
                .bg(THEME.dialog_warning_border)
        } else {
            Style::default()
                .fg(THEME.cursor_active_fg)
                .bg(THEME.cursor_active_bg)
        };
        let title_bar = Paragraph::new(title).style(title_style);
        frame.render_widget(title_bar, layout[0]);

        // Content area
        let content_area = layout[1];
        let visible_height = content_area.height as usize;

        if let Some(error) = &viewer.error {
            // Show error
            let error_para = Paragraph::new(format!("Error: {}", error))
                .style(Style::default().fg(THEME.status_error_fg))
                .block(Block::default().borders(Borders::ALL));
            frame.render_widget(error_para, content_area);
        } else {
            // Show content
            let lines = viewer.visible_lines(visible_height);
            let content: Vec<Line> = lines.iter().map(|s| Line::raw(s.as_str())).collect();
            let mut para = Paragraph::new(content)
                .style(Style::default().fg(THEME.file_fg).bg(THEME.dialog_bg));

            // Wrap text for modes where it makes sense (not hex view)
            if viewer.mode != crate::viewer::ViewMode::Hex {
                para = para.wrap(Wrap { trim: false });
            }
            frame.render_widget(para, content_area);
        }

        // Mode selector - show available modes
        let available = viewer.available_modes();
        let mut mode_spans: Vec<Span> = Vec::new();
        for (i, mode) in available.iter().enumerate() {
            if i > 0 {
                mode_spans.push(Span::raw(" "));
            }
            let style = if *mode == viewer.mode {
                Style::default()
                    .fg(THEME.cursor_active_fg)
                    .bg(THEME.cursor_active_bg)
            } else {
                Style::default()
                    .fg(THEME.help_key_fg)
                    .bg(THEME.help_key_bg)
            };
            mode_spans.push(Span::styled(
                format!(" {}:{} ", mode.shortcut(), mode.label()),
                style,
            ));
        }
        let mode_line = Line::from(mode_spans);
        let mode_bar = Paragraph::new(mode_line).style(Style::default().bg(THEME.help_desc_bg));
        frame.render_widget(mode_bar, layout[2]);

        // Help bar with position info
        let position = viewer.position_info(visible_height);
        let help_text = format!(
            " j/k:scroll  PgUp/Dn:page  g/G:top/bottom  q/Esc:close  │  {} ",
            position
        );
        let help_bar = Paragraph::new(help_text).style(
            Style::default()
                .fg(THEME.help_desc_fg)
                .bg(THEME.help_desc_bg),
        );
        frame.render_widget(help_bar, layout[3]);
    }
}
