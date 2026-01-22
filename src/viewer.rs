use std::{
    path::{Path, PathBuf},
    process::Command,
};

/// Maximum file size to read (50 MB)
const MAX_FILE_SIZE: usize = 50 * 1024 * 1024;
/// Maximum lines to keep from tool output
const MAX_OUTPUT_LINES: usize = 50_000;

/// Different view modes for the file viewer
#[derive(Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum ViewMode {
    #[default]
    Text,
    Hex,
    // Binary analysis tools
    Disasm,      // objdump -d
    Strings,     // strings
    ElfHeader,   // readelf -h
    Sections,    // readelf -S
    Symbols,     // readelf --syms
    Ldd,         // ldd
    // General tools
    FileInfo,    // file
    Exif,        // exiftool
    Archive,     // tar -tvf / unzip -l
    Json,        // jq .
}

impl ViewMode {
    pub fn label(&self) -> &'static str {
        match self {
            ViewMode::Text => "Text",
            ViewMode::Hex => "Hex",
            ViewMode::Disasm => "Disasm",
            ViewMode::Strings => "Strings",
            ViewMode::ElfHeader => "ELF Header",
            ViewMode::Sections => "Sections",
            ViewMode::Symbols => "Symbols",
            ViewMode::Ldd => "Libraries",
            ViewMode::FileInfo => "File Info",
            ViewMode::Exif => "EXIF",
            ViewMode::Archive => "Archive",
            ViewMode::Json => "JSON",
        }
    }

    pub fn shortcut(&self) -> &'static str {
        match self {
            ViewMode::Text => "t",
            ViewMode::Hex => "x",
            ViewMode::Disasm => "d",
            ViewMode::Strings => "s",
            ViewMode::ElfHeader => "h",
            ViewMode::Sections => "S",
            ViewMode::Symbols => "y",
            ViewMode::Ldd => "l",
            ViewMode::FileInfo => "i",
            ViewMode::Exif => "e",
            ViewMode::Archive => "a",
            ViewMode::Json => "J", // Capital J since lowercase j is for scrolling
        }
    }
}

/// File type detection for showing relevant tools
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub enum FileType {
    Text,
    Binary,
    Elf,
    Archive,
    Image,
    Json,
    Unknown,
}

impl FileType {
    /// Get available view modes for this file type
    pub fn available_modes(&self) -> Vec<ViewMode> {
        match self {
            FileType::Text => vec![
                ViewMode::Text,
                ViewMode::Hex,
                ViewMode::FileInfo,
            ],
            FileType::Json => vec![
                ViewMode::Json,
                ViewMode::Text,
                ViewMode::Hex,
                ViewMode::FileInfo,
            ],
            FileType::Elf => vec![
                ViewMode::Hex,
                ViewMode::Disasm,
                ViewMode::Strings,
                ViewMode::ElfHeader,
                ViewMode::Sections,
                ViewMode::Symbols,
                ViewMode::Ldd,
                ViewMode::FileInfo,
            ],
            FileType::Archive => vec![
                ViewMode::Archive,
                ViewMode::Hex,
                ViewMode::FileInfo,
            ],
            FileType::Image => vec![
                ViewMode::Hex,
                ViewMode::Exif,
                ViewMode::FileInfo,
            ],
            FileType::Binary | FileType::Unknown => vec![
                ViewMode::Hex,
                ViewMode::Strings,
                ViewMode::FileInfo,
            ],
        }
    }
}

/// State for the file viewer
#[derive(Clone)]
pub struct FileViewer {
    pub path: PathBuf,
    pub file_type: FileType,
    pub mode: ViewMode,
    pub content: Vec<String>,
    pub scroll_offset: usize,
    pub raw_bytes: Vec<u8>,
    pub error: Option<String>,
    /// Whether the file was truncated due to size limit
    pub truncated: bool,
    /// Original file size (before truncation)
    pub original_size: u64,
    /// Cached tool outputs to avoid re-running
    tool_cache: std::collections::HashMap<ViewMode, Vec<String>>,
}

impl FileViewer {
    /// Create a new file viewer for the given path
    pub fn new(path: PathBuf) -> Self {
        let mut viewer = Self {
            path,
            file_type: FileType::Unknown,
            mode: ViewMode::Text,
            content: Vec::new(),
            scroll_offset: 0,
            raw_bytes: Vec::new(),
            error: None,
            truncated: false,
            original_size: 0,
            tool_cache: std::collections::HashMap::new(),
        };
        viewer.load_file();
        viewer
    }

    /// Load the file and detect its type
    fn load_file(&mut self) {
        // Get file size first
        let metadata = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) => {
                self.error = Some(format!("Failed to read file: {}", e));
                return;
            }
        };

        self.original_size = metadata.len();

        // Check if file is too large
        if metadata.len() > MAX_FILE_SIZE as u64 {
            self.truncated = true;
        }

        match std::fs::read(&self.path) {
            Ok(bytes) => {
                let bytes = if bytes.len() > MAX_FILE_SIZE {
                    bytes[..MAX_FILE_SIZE].to_vec()
                } else {
                    bytes
                };

                self.file_type = detect_file_type(&self.path, &bytes);
                self.raw_bytes = bytes;

                // Set default mode based on file type
                self.mode = match self.file_type {
                    FileType::Text => ViewMode::Text,
                    FileType::Json => ViewMode::Json,
                    FileType::Elf => ViewMode::Hex,
                    FileType::Archive => ViewMode::Archive,
                    FileType::Image => ViewMode::Hex,
                    FileType::Binary | FileType::Unknown => ViewMode::Hex,
                };

                self.load_content_for_mode();
            }
            Err(e) => {
                self.error = Some(format!("Failed to read file: {}", e));
            }
        }
    }

    /// Load content for the current view mode
    fn load_content_for_mode(&mut self) {
        self.error = None;

        // Check cache first
        if let Some(cached) = self.tool_cache.get(&self.mode) {
            self.content = cached.clone();
            return;
        }

        let content = match self.mode {
            ViewMode::Text => self.load_text(),
            ViewMode::Hex => self.load_hex(),
            ViewMode::Disasm => self.run_tool("objdump", &["-d", "-M", "intel"]),
            ViewMode::Strings => self.run_tool("strings", &["-a"]),
            ViewMode::ElfHeader => self.run_tool("readelf", &["-h"]),
            ViewMode::Sections => self.run_tool("readelf", &["-S", "-W"]),
            ViewMode::Symbols => self.run_tool("readelf", &["--syms", "-W"]),
            ViewMode::Ldd => self.run_tool("ldd", &[]),
            ViewMode::FileInfo => self.run_tool("file", &["-b"]),
            ViewMode::Exif => self.run_tool("exiftool", &[]),
            ViewMode::Archive => self.load_archive(),
            ViewMode::Json => self.load_json(),
        };

        match content {
            Ok(lines) => {
                // Cache the result for tools (not for text/hex which are already in memory)
                if !matches!(self.mode, ViewMode::Text | ViewMode::Hex) {
                    self.tool_cache.insert(self.mode, lines.clone());
                }
                self.content = lines;
            }
            Err(e) => {
                self.error = Some(e);
                self.content = Vec::new();
            }
        }
    }

    /// Load file as text
    fn load_text(&self) -> Result<Vec<String>, String> {
        Ok(String::from_utf8_lossy(&self.raw_bytes)
            .lines()
            .map(|s| s.to_owned())
            .collect())
    }

    /// Load file as hex dump
    fn load_hex(&self) -> Result<Vec<String>, String> {
        let mut lines = Vec::new();

        for (i, chunk) in self.raw_bytes.chunks(16).enumerate() {
            let offset = i * 16;

            // Build hex part
            let mut hex_part = String::with_capacity(48);
            for (j, byte) in chunk.iter().enumerate() {
                if j == 8 {
                    hex_part.push(' ');
                }
                hex_part.push_str(&format!("{:02x} ", byte));
            }
            // Pad if less than 16 bytes
            let padding = 16 - chunk.len();
            for j in 0..padding {
                if chunk.len() + j == 8 {
                    hex_part.push(' ');
                }
                hex_part.push_str("   ");
            }

            // Build ASCII part
            let ascii_part: String = chunk
                .iter()
                .map(|&b| {
                    if b.is_ascii_graphic() || b == b' ' {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();

            lines.push(format!("{:08x}  {} |{}|", offset, hex_part, ascii_part));
        }

        Ok(lines)
    }

    /// Load and pretty-print JSON
    fn load_json(&self) -> Result<Vec<String>, String> {
        // Try jq first for nice formatting
        if let Ok(result) = self.run_tool("jq", &["."]) {
            return Ok(result);
        }

        // Fall back to basic JSON parsing
        let text = String::from_utf8_lossy(&self.raw_bytes);
        match serde_json::from_str::<serde_json::Value>(&text) {
            Ok(value) => {
                let pretty = serde_json::to_string_pretty(&value)
                    .unwrap_or_else(|_| text.to_string());
                Ok(pretty.lines().map(|s| s.to_owned()).collect())
            }
            Err(e) => {
                // Show as text with error
                let mut lines: Vec<String> = vec![
                    format!("JSON parse error: {}", e),
                    String::new(),
                    "--- Raw content ---".to_owned(),
                ];
                lines.extend(text.lines().map(|s| s.to_owned()));
                Ok(lines)
            }
        }
    }

    /// Load archive contents
    fn load_archive(&self) -> Result<Vec<String>, String> {
        let path_str = self.path.to_string_lossy();

        // Detect archive type and use appropriate tool
        if path_str.ends_with(".tar")
            || path_str.ends_with(".tar.gz")
            || path_str.ends_with(".tgz")
            || path_str.ends_with(".tar.bz2")
            || path_str.ends_with(".tar.xz")
        {
            self.run_tool("tar", &["-tvf"])
        } else if path_str.ends_with(".zip") || path_str.ends_with(".jar") {
            self.run_tool("unzip", &["-l"])
        } else if path_str.ends_with(".gz") && !path_str.ends_with(".tar.gz") {
            self.run_tool("gzip", &["-l"])
        } else if path_str.ends_with(".xz") && !path_str.ends_with(".tar.xz") {
            self.run_tool("xz", &["-l"])
        } else if path_str.ends_with(".7z") {
            self.run_tool("7z", &["l"])
        } else if path_str.ends_with(".rar") {
            self.run_tool("unrar", &["l"])
        } else {
            Err("Unknown archive format".to_owned())
        }
    }

    /// Run an external tool and capture its output
    fn run_tool(&self, tool: &str, args: &[&str]) -> Result<Vec<String>, String> {
        // Build command with the file path
        let mut cmd = Command::new(tool);
        cmd.args(args);
        cmd.arg(&self.path);

        match cmd.output() {
            Ok(output) => {
                if output.status.success() {
                    let text = String::from_utf8_lossy(&output.stdout);
                    let mut lines: Vec<String> = text
                        .lines()
                        .take(MAX_OUTPUT_LINES)
                        .map(|s| s.to_owned())
                        .collect();

                    // Add truncation notice if needed
                    let total_lines = text.lines().count();
                    if total_lines > MAX_OUTPUT_LINES {
                        lines.push(String::new());
                        lines.push(format!(
                            "--- Output truncated ({} of {} lines shown) ---",
                            MAX_OUTPUT_LINES, total_lines
                        ));
                    }
                    Ok(lines)
                } else {
                    let stderr = String::from_utf8_lossy(&output.stderr);
                    if stderr.is_empty() {
                        Err(format!("{} exited with status {}", tool, output.status))
                    } else {
                        Err(stderr.trim().to_owned())
                    }
                }
            }
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    Err(format!("'{}' not found - install it to use this feature", tool))
                } else {
                    Err(format!("Failed to run {}: {}", tool, e))
                }
            }
        }
    }

    /// Switch to a different view mode
    pub fn set_mode(&mut self, mode: ViewMode) {
        if self.mode != mode {
            self.mode = mode;
            self.scroll_offset = 0;
            self.load_content_for_mode();
        }
    }

    /// Scroll up by n lines
    pub fn scroll_up(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }

    /// Scroll down by n lines
    pub fn scroll_down(&mut self, n: usize, visible_height: usize) {
        let max_offset = self.content.len().saturating_sub(visible_height);
        self.scroll_offset = (self.scroll_offset + n).min(max_offset);
    }

    /// Jump to top
    pub fn scroll_to_top(&mut self) {
        self.scroll_offset = 0;
    }

    /// Jump to bottom
    pub fn scroll_to_bottom(&mut self, visible_height: usize) {
        self.scroll_offset = self.content.len().saturating_sub(visible_height);
    }

    /// Get visible lines for rendering
    pub fn visible_lines(&self, height: usize) -> &[String] {
        let start = self.scroll_offset;
        let end = (start + height).min(self.content.len());
        if start < self.content.len() {
            &self.content[start..end]
        } else {
            &[]
        }
    }

    /// Get available modes for this file
    pub fn available_modes(&self) -> Vec<ViewMode> {
        self.file_type.available_modes()
    }

    /// Get file size
    pub fn file_size(&self) -> usize {
        self.raw_bytes.len()
    }

    /// Get current position info for status bar
    pub fn position_info(&self, visible_height: usize) -> String {
        let total = self.content.len();
        if total == 0 {
            return "Empty".to_owned();
        }

        let top = self.scroll_offset + 1;
        let bottom = (self.scroll_offset + visible_height).min(total);
        let percent = if total > visible_height {
            (self.scroll_offset * 100) / (total - visible_height).max(1)
        } else {
            100
        };

        format!("{}-{}/{} ({}%)", top, bottom, total, percent)
    }
}

/// Detect file type from path extension and content
fn detect_file_type(path: &Path, bytes: &[u8]) -> FileType {
    // Check for ELF magic
    if bytes.len() >= 4 && &bytes[0..4] == b"\x7fELF" {
        return FileType::Elf;
    }

    // Check extension
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    match ext.as_str() {
        // Archives
        "tar" | "gz" | "tgz" | "bz2" | "xz" | "zip" | "jar" | "7z" | "rar" => {
            FileType::Archive
        }
        // Images
        "jpg" | "jpeg" | "png" | "gif" | "bmp" | "webp" | "tiff" | "ico" | "svg" => {
            FileType::Image
        }
        // JSON
        "json" => FileType::Json,
        // Likely text
        "txt" | "md" | "rst" | "log" | "cfg" | "conf" | "ini" | "yaml" | "yml"
        | "toml" | "xml" | "html" | "htm" | "css" | "js" | "ts" | "jsx" | "tsx"
        | "py" | "rs" | "go" | "c" | "h" | "cpp" | "hpp" | "java" | "kt" | "scala"
        | "rb" | "php" | "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd"
        | "sql" | "vim" | "lua" | "pl" | "pm" | "r" | "R" | "jl" | "swift"
        | "m" | "mm" | "hs" | "ml" | "mli" | "ex" | "exs" | "erl" | "hrl"
        | "clj" | "cljs" | "cljc" | "lisp" | "el" | "scm" | "rkt"
        | "asm" | "s" | "S" | "nasm" | "Makefile" | "makefile" | "cmake"
        | "dockerfile" | "Dockerfile" | "gitignore" | "gitattributes"
        | "editorconfig" | "prettierrc" | "eslintrc" | "babelrc"
        | "csv" | "tsv" => {
            FileType::Text
        }
        _ => {
            // Check content for binary vs text
            if is_likely_text(bytes) {
                FileType::Text
            } else {
                FileType::Binary
            }
        }
    }
}

/// Check if content is likely text (no null bytes, mostly printable)
fn is_likely_text(bytes: &[u8]) -> bool {
    if bytes.is_empty() {
        return true;
    }

    // Sample first 8KB
    let sample = if bytes.len() > 8192 { &bytes[..8192] } else { bytes };

    let mut non_text_count = 0;
    for &b in sample {
        // Null byte is a strong indicator of binary
        if b == 0 {
            return false;
        }
        // Count non-printable, non-whitespace bytes
        if b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r') {
            non_text_count += 1;
        }
    }

    // Allow up to 5% non-text bytes (for things like form feeds, etc.)
    non_text_count * 20 < sample.len()
}
