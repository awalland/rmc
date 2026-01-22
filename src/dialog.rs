use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::Style,
    widgets::{Block, Borders, Clear},
    Frame,
};

use crate::theme::THEME;

// ============================================================================
// Dialog Result
// ============================================================================

pub enum DialogResult {
    Accept,
    Reject,
    Pending,
}

/// Handle common yes/no key bindings for dialogs.
/// Returns Accept for Y/y/Enter, Reject for N/n/Esc, Pending otherwise.
pub fn handle_yes_no_keys(key: KeyCode) -> DialogResult {
    match key {
        KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => DialogResult::Accept,
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => DialogResult::Reject,
        _ => DialogResult::Pending,
    }
}

// ============================================================================
// Dialog Frame Rendering
// ============================================================================

/// Renders the common dialog frame: shadow, clear, bordered block with title.
/// Returns the inner area (inside the block) for content rendering.
pub fn render_dialog_frame(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    border_color: ratatui::style::Color,
) -> Rect {
    // Draw shadow
    let shadow_area = Rect {
        x: area.x.saturating_add(2),
        y: area.y.saturating_add(1),
        width: area.width,
        height: area.height,
    };
    frame.render_widget(
        Block::default().style(Style::default().bg(THEME.dialog_shadow)),
        shadow_area,
    );

    // Clear the dialog area
    frame.render_widget(Clear, area);

    // Render the bordered block
    let block = Block::default()
        .title(format!(" {} ", title))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(THEME.dialog_bg));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    inner
}

// ============================================================================
// Common Button Layouts
// ============================================================================

/// Renders a centered Yes/No button row.
pub fn render_yes_no_buttons(frame: &mut Frame, area: Rect) {
    use ratatui::{layout::Alignment, text::Span, widgets::Paragraph};

    let button_layout = Layout::horizontal([
        Constraint::Percentage(25),
        Constraint::Percentage(20),
        Constraint::Percentage(10),
        Constraint::Percentage(20),
        Constraint::Percentage(25),
    ])
    .split(area);

    let yes_button = Paragraph::new(Span::raw(" [Y]es "))
        .style(Style::default().fg(THEME.dialog_button_fg).bg(THEME.dialog_button_bg))
        .alignment(Alignment::Center);
    frame.render_widget(yes_button, button_layout[1]);

    let no_button = Paragraph::new(Span::raw(" [N]o "))
        .style(Style::default().fg(THEME.dialog_button_fg).bg(THEME.dialog_button_bg))
        .alignment(Alignment::Center);
    frame.render_widget(no_button, button_layout[3]);
}

// ============================================================================
// Centered Rect Helper
// ============================================================================

/// Creates a centered rectangle within an area.
pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::vertical([
        Constraint::Percentage((100 - percent_y) / 2),
        Constraint::Percentage(percent_y),
        Constraint::Percentage((100 - percent_y) / 2),
    ])
    .split(area);

    Layout::horizontal([
        Constraint::Percentage((100 - percent_x) / 2),
        Constraint::Percentage(percent_x),
        Constraint::Percentage((100 - percent_x) / 2),
    ])
    .split(popup_layout[1])[1]
}
