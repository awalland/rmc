use ratatui::style::Color;

pub struct Theme {
    // Pane borders
    pub pane_active_border: Color,
    pub pane_inactive_border: Color,
    pub pane_title: Color,

    // File list
    pub directory_fg: Color,
    pub file_fg: Color,
    pub selected_fg: Color,
    pub selected_bg: Color,

    // Cursor/highlight
    pub cursor_active_fg: Color,
    pub cursor_active_bg: Color,
    pub cursor_inactive_fg: Color,
    pub cursor_inactive_bg: Color,

    // Status bar
    pub status_error_fg: Color,
    pub status_error_bg: Color,
    pub status_info_fg: Color,
    pub status_info_bg: Color,

    // Help bar
    pub help_key_fg: Color,
    pub help_key_bg: Color,
    pub help_desc_fg: Color,
    pub help_desc_bg: Color,

    // Job popup
    pub job_popup_border: Color,
    pub job_no_jobs: Color,
    pub job_gauge: Color,
    pub job_file_info: Color,
    pub job_completed: Color,
    pub job_error: Color,
    pub job_cancelled: Color,

    // Dialogs
    pub dialog_bg: Color,
    pub dialog_border: Color,
    pub dialog_warning_border: Color,
    pub dialog_delete_border: Color,
    pub dialog_warning_text: Color,
    pub dialog_input_fg: Color,
    pub dialog_input_bg: Color,
    pub dialog_hint: Color,
    pub dialog_shadow: Color,
    pub dialog_button_fg: Color,
    pub dialog_button_bg: Color,
}

// Tokyo Night inspired color palette
pub const THEME: Theme = Theme {
    // Pane borders - muted blue for active, dark gray for inactive
    pane_active_border: Color::Rgb(122, 162, 247),    // #7aa2f7 - soft blue
    pane_inactive_border: Color::Rgb(86, 95, 137),    // #565f89 - muted gray
    pane_title: Color::Rgb(224, 175, 104),            // #e0af68 - muted yellow

    // File list
    directory_fg: Color::Rgb(122, 162, 247),          // #7aa2f7 - soft blue
    file_fg: Color::Rgb(169, 177, 214),               // #a9b1d6 - light gray
    selected_fg: Color::Rgb(224, 175, 104),           // #e0af68 - muted orange
    selected_bg: Color::Rgb(41, 46, 66),              // #292e42 - dark highlight

    // Cursor/highlight
    cursor_active_fg: Color::Rgb(26, 27, 38),         // #1a1b26 - dark bg
    cursor_active_bg: Color::Rgb(122, 162, 247),      // #7aa2f7 - soft blue
    cursor_inactive_fg: Color::Rgb(169, 177, 214),    // #a9b1d6 - light gray
    cursor_inactive_bg: Color::Rgb(41, 46, 66),       // #292e42 - dark highlight

    // Status bar
    status_error_fg: Color::Rgb(247, 118, 142),       // #f7768e - soft red
    status_error_bg: Color::Rgb(26, 27, 38),          // #1a1b26 - dark bg
    status_info_fg: Color::Rgb(224, 175, 104),        // #e0af68 - muted orange
    status_info_bg: Color::Rgb(26, 27, 38),           // #1a1b26 - dark bg

    // Help bar
    help_key_fg: Color::Rgb(26, 27, 38),              // #1a1b26 - dark bg
    help_key_bg: Color::Rgb(140, 160, 210),           // #8ca0d2 - soft periwinkle
    help_desc_fg: Color::Rgb(169, 177, 214),          // #a9b1d6 - light gray
    help_desc_bg: Color::Rgb(36, 40, 59),             // #24283b - slightly lighter bg

    // Job popup
    job_popup_border: Color::Rgb(187, 154, 247),      // #bb9af7 - purple
    job_no_jobs: Color::Rgb(86, 95, 137),             // #565f89 - muted gray
    job_gauge: Color::Rgb(110, 136, 166),             // #6e88a6 - steel blue
    job_file_info: Color::Rgb(86, 95, 137),           // #565f89 - muted gray
    job_completed: Color::Rgb(158, 206, 106),         // #9ece6a - soft green
    job_error: Color::Rgb(247, 118, 142),             // #f7768e - soft red
    job_cancelled: Color::Rgb(86, 95, 137),           // #565f89 - muted gray

    // Dialogs
    dialog_bg: Color::Rgb(26, 27, 38),                // #1a1b26 - dark bg
    dialog_border: Color::Rgb(122, 162, 247),         // #7aa2f7 - soft blue
    dialog_warning_border: Color::Rgb(224, 175, 104), // #e0af68 - muted orange
    dialog_delete_border: Color::Rgb(247, 118, 142),  // #f7768e - soft red
    dialog_warning_text: Color::Rgb(224, 175, 104),   // #e0af68 - muted orange
    dialog_input_fg: Color::Rgb(169, 177, 214),       // #a9b1d6 - light gray
    dialog_input_bg: Color::Rgb(41, 46, 66),          // #292e42 - dark highlight
    dialog_hint: Color::Rgb(86, 95, 137),             // #565f89 - muted gray
    dialog_shadow: Color::Rgb(15, 15, 20),            // #0f0f14 - very dark
    dialog_button_fg: Color::Rgb(169, 177, 214),      // #a9b1d6 - light gray
    dialog_button_bg: Color::Rgb(56, 62, 87),         // #383e57 - button bg
};
