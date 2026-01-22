use std::{
    collections::HashMap,
    io::{BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

use walkdir::WalkDir;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct JobId(u64);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobType {
    Copy,
    Move,
    Delete,
}

#[derive(Clone)]
pub enum JobStatus {
    Running { started_at: Instant },
    Visible,
    Paused,
    Completed,
    Failed(String),
    Cancelled,
}

#[derive(Clone, Default)]
pub struct JobProgress {
    pub total_bytes: u64,
    pub processed_bytes: u64,
    pub current_file: Option<String>,
    pub files_processed: u64,
    pub total_files: u64,
}

const THROUGHPUT_HISTORY_SIZE: usize = 60;
const THROUGHPUT_SAMPLE_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Clone)]
pub struct ThroughputTracker {
    pub history: Vec<u64>,          // bytes/sec samples
    last_sample_time: Instant,
    last_sample_bytes: u64,
}

impl ThroughputTracker {
    pub fn new() -> Self {
        Self {
            history: Vec::with_capacity(THROUGHPUT_HISTORY_SIZE),
            last_sample_time: Instant::now(),
            last_sample_bytes: 0,
        }
    }

    pub fn update(&mut self, current_bytes: u64) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_sample_time);

        if elapsed >= THROUGHPUT_SAMPLE_INTERVAL {
            let bytes_diff = current_bytes.saturating_sub(self.last_sample_bytes);
            let secs = elapsed.as_secs_f64();
            let throughput = if secs > 0.0 {
                (bytes_diff as f64 / secs) as u64
            } else {
                0
            };

            self.history.push(throughput);
            if self.history.len() > THROUGHPUT_HISTORY_SIZE {
                self.history.remove(0);
            }

            self.last_sample_time = now;
            self.last_sample_bytes = current_bytes;
        }
    }

    pub fn current_throughput(&self) -> u64 {
        self.history.last().copied().unwrap_or(0)
    }
}

#[derive(Clone)]
pub struct Job {
    pub id: JobId,
    pub job_type: JobType,
    pub description: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub status: JobStatus,
    pub progress: JobProgress,
    pub throughput: ThroughputTracker,
}

pub enum JobUpdate {
    ScanComplete {
        job_id: JobId,
        total_bytes: u64,
        total_files: u64,
    },
    Progress {
        job_id: JobId,
        processed_bytes: u64,
        current_file: Option<String>,
        files_processed: u64,
    },
    Completed {
        job_id: JobId,
    },
    Failed {
        job_id: JobId,
        error: String,
    },
    ConflictDetected {
        job_id: JobId,
        file_path: PathBuf,
    },
}

#[derive(Clone, Copy, Debug)]
pub enum ConflictResolution {
    Overwrite,
    Skip,
    OverwriteAll,
    SkipAll,
    Cancel,
}

struct WorkerHandle {
    cancel_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
    conflict_tx: Sender<ConflictResolution>,
}

pub struct JobManager {
    jobs: HashMap<JobId, Job>,
    pub progress_rx: Receiver<JobUpdate>,
    progress_tx: Sender<JobUpdate>,
    workers: HashMap<JobId, WorkerHandle>,
    next_id: u64,
}

impl JobManager {
    pub fn new() -> Self {
        let (progress_tx, progress_rx) = mpsc::channel();
        Self {
            jobs: HashMap::new(),
            progress_rx,
            progress_tx,
            workers: HashMap::new(),
            next_id: 0,
        }
    }

    pub fn start_job(&mut self, job_type: JobType, source: PathBuf, dest_dir: PathBuf) -> JobId {
        let id = JobId(self.next_id);
        self.next_id += 1;

        let action = match job_type {
            JobType::Copy => "Copying",
            JobType::Move => "Moving",
            JobType::Delete => "Deleting", // Not used, delete has its own method
        };

        let description = format!(
            "{} '{}' to {}",
            action,
            source.file_name().unwrap_or_default().to_string_lossy(),
            dest_dir.display()
        );

        let job = Job {
            id,
            job_type,
            description,
            source: source.clone(),
            destination: dest_dir.clone(),
            status: JobStatus::Running {
                started_at: Instant::now(),
            },
            progress: JobProgress::default(),
            throughput: ThroughputTracker::new(),
        };

        self.jobs.insert(id, job);

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let pause_flag = Arc::new(AtomicBool::new(false));
        let (conflict_tx, conflict_rx) = mpsc::channel();

        let worker_handle = WorkerHandle {
            cancel_flag: Arc::clone(&cancel_flag),
            pause_flag: Arc::clone(&pause_flag),
            conflict_tx,
        };
        self.workers.insert(id, worker_handle);

        let progress_tx = self.progress_tx.clone();

        thread::spawn(move || {
            transfer_worker(id, job_type, source, dest_dir, progress_tx, cancel_flag, pause_flag, conflict_rx);
        });

        id
    }

    pub fn cancel_job(&mut self, job_id: JobId) {
        if let Some(handle) = self.workers.get(&job_id) {
            handle.cancel_flag.store(true, Ordering::Relaxed);
        }
        if let Some(job) = self.jobs.get_mut(&job_id) {
            job.status = JobStatus::Cancelled;
        }
    }

    pub fn toggle_pause_job(&mut self, job_id: JobId) {
        if let Some(job) = self.jobs.get_mut(&job_id) {
            match job.status {
                JobStatus::Running { .. } | JobStatus::Visible => {
                    // Pause the job
                    if let Some(handle) = self.workers.get(&job_id) {
                        handle.pause_flag.store(true, Ordering::Relaxed);
                    }
                    job.status = JobStatus::Paused;
                }
                JobStatus::Paused => {
                    // Resume the job
                    if let Some(handle) = self.workers.get(&job_id) {
                        handle.pause_flag.store(false, Ordering::Relaxed);
                    }
                    job.status = JobStatus::Visible;
                }
                _ => {}
            }
        }
    }

    pub fn start_delete_job(&mut self, paths: Vec<PathBuf>, parent_dir: PathBuf) -> JobId {
        let id = JobId(self.next_id);
        self.next_id += 1;

        let description = if paths.len() == 1 {
            format!(
                "Deleting '{}'",
                paths[0].file_name().unwrap_or_default().to_string_lossy()
            )
        } else {
            format!("Deleting {} items", paths.len())
        };

        let job = Job {
            id,
            job_type: JobType::Delete,
            description,
            source: parent_dir.clone(),
            destination: PathBuf::new(), // Not used for delete
            status: JobStatus::Running {
                started_at: Instant::now(),
            },
            progress: JobProgress::default(),
            throughput: ThroughputTracker::new(),
        };

        self.jobs.insert(id, job);

        let cancel_flag = Arc::new(AtomicBool::new(false));
        let pause_flag = Arc::new(AtomicBool::new(false));
        let (conflict_tx, _conflict_rx) = mpsc::channel();

        let worker_handle = WorkerHandle {
            cancel_flag: Arc::clone(&cancel_flag),
            pause_flag: Arc::clone(&pause_flag),
            conflict_tx,
        };
        self.workers.insert(id, worker_handle);

        let progress_tx = self.progress_tx.clone();

        thread::spawn(move || {
            delete_worker(id, paths, progress_tx, cancel_flag, pause_flag);
        });

        id
    }

    pub fn send_conflict_resolution(&self, job_id: JobId, resolution: ConflictResolution) {
        if let Some(handle) = self.workers.get(&job_id) {
            let _ = handle.conflict_tx.send(resolution);
        }
    }

    /// Returns (completed_destinations, completed_sources_for_moves)
    pub fn process_updates(&mut self) -> (Vec<PathBuf>, Vec<PathBuf>) {
        let mut completed_destinations = Vec::new();
        let mut completed_sources = Vec::new();

        while let Ok(update) = self.progress_rx.try_recv() {
            match update {
                JobUpdate::ScanComplete {
                    job_id,
                    total_bytes,
                    total_files,
                } => {
                    if let Some(job) = self.jobs.get_mut(&job_id) {
                        job.progress.total_bytes = total_bytes;
                        job.progress.total_files = total_files;
                    }
                }
                JobUpdate::Progress {
                    job_id,
                    processed_bytes,
                    current_file,
                    files_processed,
                } => {
                    if let Some(job) = self.jobs.get_mut(&job_id) {
                        job.progress.processed_bytes = processed_bytes;
                        job.progress.current_file = current_file;
                        job.progress.files_processed = files_processed;
                        job.throughput.update(processed_bytes);
                    }
                }
                JobUpdate::Completed { job_id } => {
                    if let Some(job) = self.jobs.get_mut(&job_id) {
                        match job.job_type {
                            JobType::Copy => {
                                completed_destinations.push(job.destination.clone());
                            }
                            JobType::Move => {
                                completed_destinations.push(job.destination.clone());
                                if let Some(parent) = job.source.parent() {
                                    completed_sources.push(parent.to_path_buf());
                                }
                            }
                            JobType::Delete => {
                                // For delete, source holds the parent directory
                                completed_sources.push(job.source.clone());
                            }
                        }
                        job.status = JobStatus::Completed;
                    }
                    self.workers.remove(&job_id);
                }
                JobUpdate::Failed { job_id, error } => {
                    if let Some(job) = self.jobs.get_mut(&job_id) {
                        job.status = JobStatus::Failed(error);
                    }
                    self.workers.remove(&job_id);
                }
                JobUpdate::ConflictDetected { .. } => {
                    // Handled separately via UI
                }
            }
        }

        (completed_destinations, completed_sources)
    }

    pub fn update_visibility(&mut self) {
        let threshold = Duration::from_millis(500);
        let now = Instant::now();

        for job in self.jobs.values_mut() {
            if let JobStatus::Running { started_at } = job.status {
                if now.duration_since(started_at) >= threshold {
                    job.status = JobStatus::Visible;
                }
            }
        }
    }

    pub fn active_job_count(&self) -> usize {
        self.jobs
            .values()
            .filter(|j| matches!(j.status, JobStatus::Running { .. } | JobStatus::Visible | JobStatus::Paused))
            .count()
    }

    pub fn all_jobs(&self) -> Vec<&Job> {
        let mut jobs: Vec<_> = self.jobs.values().collect();
        // Sort by JobId descending so newest jobs appear first
        jobs.sort_by(|a, b| b.id.0.cmp(&a.id.0));
        jobs
    }

    pub fn dismiss_job(&mut self, job_id: JobId) {
        if let Some(job) = self.jobs.get(&job_id) {
            if matches!(
                job.status,
                JobStatus::Completed | JobStatus::Failed(_) | JobStatus::Cancelled
            ) {
                self.jobs.remove(&job_id);
            }
        }
    }

    /// Check if any of the given paths conflict with active jobs
    /// Returns true if deleting these paths could interfere with running jobs
    pub fn paths_conflict_with_active_jobs(&self, paths: &[PathBuf]) -> bool {
        let active_jobs: Vec<_> = self
            .jobs
            .values()
            .filter(|j| matches!(j.status, JobStatus::Running { .. } | JobStatus::Visible))
            .filter(|j| j.job_type != JobType::Delete) // Only check copy/move jobs
            .collect();

        for path in paths {
            let path_canonical = path.canonicalize().unwrap_or_else(|_| path.clone());

            for job in &active_jobs {
                let source_canonical = job.source.canonicalize().unwrap_or_else(|_| job.source.clone());
                let dest_canonical = job.destination.canonicalize().unwrap_or_else(|_| job.destination.clone());

                // Check if path overlaps with source or destination
                if path_canonical.starts_with(&source_canonical)
                    || source_canonical.starts_with(&path_canonical)
                    || path_canonical.starts_with(&dest_canonical)
                    || dest_canonical.starts_with(&path_canonical)
                {
                    return true;
                }
            }
        }

        false
    }
}

// ============================================================================
// Transfer Worker (Copy/Move)
// ============================================================================

fn transfer_worker(
    job_id: JobId,
    job_type: JobType,
    source: PathBuf,
    dest_dir: PathBuf,
    progress_tx: Sender<JobUpdate>,
    cancel_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
    conflict_rx: Receiver<ConflictResolution>,
) {
    // Phase 1: Scan to calculate totals
    let mut total_bytes = 0u64;
    let mut total_files = 0u64;

    if source.is_file() {
        total_bytes = std::fs::metadata(&source).map(|m| m.len()).unwrap_or(0);
        total_files = 1;
    } else {
        for entry in WalkDir::new(&source).into_iter().filter_map(|e| e.ok()) {
            if cancel_flag.load(Ordering::Relaxed) {
                return;
            }
            if entry.file_type().is_file() {
                total_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                total_files += 1;
            }
        }
    }

    let _ = progress_tx.send(JobUpdate::ScanComplete {
        job_id,
        total_bytes,
        total_files,
    });

    // Phase 2: Copy with progress
    let mut processed_bytes = 0u64;
    let mut files_processed = 0u64;
    let mut overwrite_all = false;
    let mut skip_all = false;

    let dest_name = source.file_name().unwrap_or_default();
    let dest_path = dest_dir.join(dest_name);

    let result = if source.is_file() {
        copy_file_with_progress(
            &source,
            &dest_path,
            &progress_tx,
            job_id,
            &cancel_flag,
            &pause_flag,
            &conflict_rx,
            &mut processed_bytes,
            &mut files_processed,
            &mut overwrite_all,
            &mut skip_all,
        )
    } else {
        copy_dir_with_progress(
            &source,
            &dest_path,
            &progress_tx,
            job_id,
            &cancel_flag,
            &pause_flag,
            &conflict_rx,
            &mut processed_bytes,
            &mut files_processed,
            &mut overwrite_all,
            &mut skip_all,
        )
    };

    match result {
        Ok(()) => {
            // For move operations, delete the source after successful copy
            if job_type == JobType::Move {
                let delete_result = if source.is_file() {
                    std::fs::remove_file(&source)
                } else {
                    std::fs::remove_dir_all(&source)
                };

                if let Err(e) = delete_result {
                    let _ = progress_tx.send(JobUpdate::Failed {
                        job_id,
                        error: format!("Copied but failed to delete source: {}", e),
                    });
                    return;
                }
            }
            let _ = progress_tx.send(JobUpdate::Completed { job_id });
        }
        Err(e) => {
            let _ = progress_tx.send(JobUpdate::Failed {
                job_id,
                error: e.to_string(),
            });
        }
    }
}

fn copy_dir_with_progress(
    source: &Path,
    dest: &Path,
    progress_tx: &Sender<JobUpdate>,
    job_id: JobId,
    cancel_flag: &Arc<AtomicBool>,
    pause_flag: &Arc<AtomicBool>,
    conflict_rx: &Receiver<ConflictResolution>,
    processed_bytes: &mut u64,
    files_processed: &mut u64,
    overwrite_all: &mut bool,
    skip_all: &mut bool,
) -> std::io::Result<()> {
    for entry in WalkDir::new(source).into_iter().filter_map(|e| e.ok()) {
        if cancel_flag.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Cancelled",
            ));
        }

        let relative = entry.path().strip_prefix(source).unwrap_or(entry.path());
        let target = dest.join(relative);

        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            // Ensure parent directory exists
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)?;
            }

            copy_file_with_progress(
                entry.path(),
                &target,
                progress_tx,
                job_id,
                cancel_flag,
                pause_flag,
                conflict_rx,
                processed_bytes,
                files_processed,
                overwrite_all,
                skip_all,
            )?;
        }
        // Skip symlinks
    }

    Ok(())
}

fn copy_file_with_progress(
    source: &Path,
    dest: &Path,
    progress_tx: &Sender<JobUpdate>,
    job_id: JobId,
    cancel_flag: &Arc<AtomicBool>,
    pause_flag: &Arc<AtomicBool>,
    conflict_rx: &Receiver<ConflictResolution>,
    processed_bytes: &mut u64,
    files_processed: &mut u64,
    overwrite_all: &mut bool,
    skip_all: &mut bool,
) -> std::io::Result<()> {
    // Check for conflict
    if dest.exists() {
        if *skip_all {
            *files_processed += 1;
            return Ok(());
        }

        if !*overwrite_all {
            // Send conflict notification and wait for resolution
            let _ = progress_tx.send(JobUpdate::ConflictDetected {
                job_id,
                file_path: dest.to_path_buf(),
            });

            // Wait for resolution (blocking)
            match conflict_rx.recv() {
                Ok(ConflictResolution::Overwrite) => {}
                Ok(ConflictResolution::Skip) => {
                    *files_processed += 1;
                    return Ok(());
                }
                Ok(ConflictResolution::OverwriteAll) => {
                    *overwrite_all = true;
                }
                Ok(ConflictResolution::SkipAll) => {
                    *skip_all = true;
                    *files_processed += 1;
                    return Ok(());
                }
                Ok(ConflictResolution::Cancel) | Err(_) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Interrupted,
                        "Cancelled",
                    ));
                }
            }
        }
    }

    let src_file = std::fs::File::open(source)?;
    let dest_file = std::fs::File::create(dest)?;

    let mut reader = BufReader::with_capacity(64 * 1024, src_file);
    let mut writer = BufWriter::with_capacity(64 * 1024, dest_file);
    let mut buffer = [0u8; 64 * 1024];

    let file_name = source
        .file_name()
        .map(|s| s.to_string_lossy().into_owned());

    loop {
        // Check cancel flag
        if cancel_flag.load(Ordering::Relaxed) {
            drop(writer);
            let _ = std::fs::remove_file(dest);
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "Cancelled",
            ));
        }

        // Wait while paused
        while pause_flag.load(Ordering::Relaxed) {
            if cancel_flag.load(Ordering::Relaxed) {
                drop(writer);
                let _ = std::fs::remove_file(dest);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Cancelled",
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }

        let bytes_read = reader.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }

        writer.write_all(&buffer[..bytes_read])?;
        *processed_bytes += bytes_read as u64;

        let _ = progress_tx.send(JobUpdate::Progress {
            job_id,
            processed_bytes: *processed_bytes,
            current_file: file_name.clone(),
            files_processed: *files_processed,
        });
    }

    writer.flush()?;
    *files_processed += 1;

    let _ = progress_tx.send(JobUpdate::Progress {
        job_id,
        processed_bytes: *processed_bytes,
        current_file: file_name,
        files_processed: *files_processed,
    });

    Ok(())
}

// ============================================================================
// Delete Worker
// ============================================================================

fn delete_worker(
    job_id: JobId,
    paths: Vec<PathBuf>,
    progress_tx: Sender<JobUpdate>,
    cancel_flag: Arc<AtomicBool>,
    pause_flag: Arc<AtomicBool>,
) {
    // Phase 1: Scan to calculate totals
    let mut total_bytes = 0u64;
    let mut total_files = 0u64;

    for path in &paths {
        if cancel_flag.load(Ordering::Relaxed) {
            return;
        }

        if path.is_file() {
            total_bytes += std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            total_files += 1;
        } else if path.is_dir() {
            for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
                if cancel_flag.load(Ordering::Relaxed) {
                    return;
                }
                if entry.file_type().is_file() {
                    total_bytes += entry.metadata().map(|m| m.len()).unwrap_or(0);
                    total_files += 1;
                }
            }
        }
    }

    let _ = progress_tx.send(JobUpdate::ScanComplete {
        job_id,
        total_bytes,
        total_files,
    });

    // Phase 2: Delete with progress
    let mut processed_bytes = 0u64;
    let mut files_processed = 0u64;

    for path in &paths {
        if cancel_flag.load(Ordering::Relaxed) {
            return;
        }

        let result = delete_path_with_progress(
            path,
            &progress_tx,
            job_id,
            &cancel_flag,
            &pause_flag,
            &mut processed_bytes,
            &mut files_processed,
        );

        if let Err(e) = result {
            let _ = progress_tx.send(JobUpdate::Failed {
                job_id,
                error: e.to_string(),
            });
            return;
        }
    }

    let _ = progress_tx.send(JobUpdate::Completed { job_id });
}

fn delete_path_with_progress(
    path: &Path,
    progress_tx: &Sender<JobUpdate>,
    job_id: JobId,
    cancel_flag: &Arc<AtomicBool>,
    pause_flag: &Arc<AtomicBool>,
    processed_bytes: &mut u64,
    files_processed: &mut u64,
) -> std::io::Result<()> {
    // Helper to wait while paused
    let wait_if_paused = || {
        while pause_flag.load(Ordering::Relaxed) {
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Cancelled",
                ));
            }
            thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    };

    if path.is_file() {
        wait_if_paused()?;

        let file_size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let file_name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned());

        std::fs::remove_file(path)?;

        *processed_bytes += file_size;
        *files_processed += 1;

        let _ = progress_tx.send(JobUpdate::Progress {
            job_id,
            processed_bytes: *processed_bytes,
            current_file: file_name,
            files_processed: *files_processed,
        });
    } else if path.is_dir() {
        // Collect all files first, then delete in reverse order (files before dirs)
        let mut files_to_delete: Vec<PathBuf> = Vec::new();
        let mut dirs_to_delete: Vec<PathBuf> = Vec::new();

        for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Cancelled",
                ));
            }

            let entry_path = entry.path().to_path_buf();
            if entry.file_type().is_file() {
                files_to_delete.push(entry_path);
            } else if entry.file_type().is_dir() {
                dirs_to_delete.push(entry_path);
            }
        }

        // Delete files first
        for file_path in files_to_delete {
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Cancelled",
                ));
            }

            wait_if_paused()?;

            let file_size = std::fs::metadata(&file_path).map(|m| m.len()).unwrap_or(0);
            let file_name = file_path
                .file_name()
                .map(|s| s.to_string_lossy().into_owned());

            std::fs::remove_file(&file_path)?;

            *processed_bytes += file_size;
            *files_processed += 1;

            let _ = progress_tx.send(JobUpdate::Progress {
                job_id,
                processed_bytes: *processed_bytes,
                current_file: file_name,
                files_processed: *files_processed,
            });
        }

        // Delete directories in reverse order (deepest first)
        dirs_to_delete.sort_by(|a, b| b.components().count().cmp(&a.components().count()));
        for dir_path in dirs_to_delete {
            if cancel_flag.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "Cancelled",
                ));
            }
            std::fs::remove_dir(&dir_path)?;
        }
    }

    Ok(())
}
