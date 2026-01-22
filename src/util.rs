// Utility functions and constants for the file manager

// ============================================================================
// Constants
// ============================================================================

/// Event loop polling interval in milliseconds
pub const EVENT_POLL_MS: u64 = 50;

/// Error message display duration in seconds
pub const ERROR_DISPLAY_SECS: u64 = 3;

/// Job visibility threshold in milliseconds (show in status bar after this)
pub const JOB_VISIBILITY_THRESHOLD_MS: u64 = 500;

/// Page up/down scroll amount
pub const PAGE_SCROLL_SIZE: usize = 10;

/// Copy buffer size (64 KB)
pub const COPY_BUFFER_SIZE: usize = 64 * 1024;

/// Throughput history sample count
pub const THROUGHPUT_HISTORY_SIZE: usize = 60;

/// Throughput sampling interval in milliseconds
pub const THROUGHPUT_SAMPLE_INTERVAL_MS: u64 = 200;

/// Rename progress dialog auto-close delay in seconds
pub const RENAME_DIALOG_TIMEOUT_SECS: u64 = 4;

// ============================================================================
// Byte Formatting
// ============================================================================

/// Format bytes with long suffixes (e.g., "1.5GB", "250KB")
/// Used in status bars and progress displays
pub fn format_bytes(bytes: u64) -> String {
    format_bytes_impl(bytes, true)
}

/// Format bytes with short suffixes (e.g., "1.5G", "250K")
/// Used in file list size columns where space is limited
pub fn format_size(bytes: u64) -> String {
    format_bytes_impl(bytes, false)
}

fn format_bytes_impl(bytes: u64, long_suffix: bool) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    let (value, suffix) = if bytes >= TB {
        (bytes as f64 / TB as f64, if long_suffix { "TB" } else { "T" })
    } else if bytes >= GB {
        (bytes as f64 / GB as f64, if long_suffix { "GB" } else { "G" })
    } else if bytes >= MB {
        (bytes as f64 / MB as f64, if long_suffix { "MB" } else { "M" })
    } else if bytes >= KB {
        (bytes as f64 / KB as f64, if long_suffix { "KB" } else { "K" })
    } else {
        return if long_suffix {
            format!("{}B", bytes)
        } else {
            format!("{}", bytes)
        };
    };

    format!("{:.1}{}", value, suffix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_bytes() {
        assert_eq!(format_bytes(0), "0B");
        assert_eq!(format_bytes(512), "512B");
        assert_eq!(format_bytes(1024), "1.0KB");
        assert_eq!(format_bytes(1536), "1.5KB");
        assert_eq!(format_bytes(1024 * 1024), "1.0MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0GB");
    }

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(0), "0");
        assert_eq!(format_size(512), "512");
        assert_eq!(format_size(1024), "1.0K");
        assert_eq!(format_size(1024 * 1024), "1.0M");
        assert_eq!(format_size(1024 * 1024 * 1024), "1.0G");
    }
}
