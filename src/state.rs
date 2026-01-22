use std::{
    env,
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

const APP_NAME: &str = "rc";

/// Get the state file path following XDG Base Directory specification
pub fn get_state_file_path() -> PathBuf {
    let state_home = env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = env::var("HOME").unwrap_or_else(|_| ".".to_owned());
            PathBuf::from(home).join(".local/state")
        });

    state_home.join(APP_NAME).join("state")
}

#[derive(Default)]
pub struct AppState {
    pub right_path: Option<PathBuf>,
}

impl AppState {
    pub fn load() -> Self {
        let path = get_state_file_path();
        let Ok(file) = std::fs::File::open(&path) else {
            return Self::default();
        };

        let reader = BufReader::new(file);
        let mut state = Self::default();

        for line in reader.lines().map_while(Result::ok) {
            if let Some((key, value)) = line.split_once('=') {
                let path = PathBuf::from(value);
                // Only use the path if it still exists
                if path.is_dir() && key == "right" {
                    state.right_path = Some(path);
                }
            }
        }

        state
    }

    pub fn save(right_path: &Path) {
        let path = get_state_file_path();

        // Create parent directories if needed
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let Ok(file) = std::fs::File::create(&path) else {
            return;
        };

        let mut writer = BufWriter::new(file);
        let _ = writeln!(writer, "right={}", right_path.display());
    }
}
