use anyhow::{anyhow, Result};
use eframe::egui;
use rfd::FileDialog;
use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

// Add fs2 crate for disk space checking
use fs2::available_space;

// Resolution options
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resolution {
    K2,
    K4,
    K6,
}

impl Resolution {
    fn as_str(&self) -> &'static str {
        match self {
            Resolution::K2 => "2K (2048x2048)",
            Resolution::K4 => "4K (4096x4096)",
            Resolution::K6 => "6K (Original)",
        }
    }

    fn target_size(&self) -> Option<(u32, u32)> {
        match self {
            Resolution::K2 => Some((2048, 2048)),
            Resolution::K4 => Some((4096, 4096)),
            Resolution::K6 => None,
        }
    }
}

fn main() -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([500.0, 350.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Delivery Encoder",
        options,
        Box::new(|_| Box::new(DeliveryEncoderApp::new())),
    )
    .map_err(|e| anyhow!("Application error: {}", e))
}

struct DeliveryEncoderApp {
    output_dir: PathBuf,
    status: String,
    progress: f32,
    encoding: bool,
    worker_thread: Option<thread::JoinHandle<()>>,
    progress_receiver: Receiver<(f32, String)>,
    cancel_sender: Option<Sender<()>>,
    ffmpeg_path: PathBuf,
    ffprobe_path: PathBuf,
    current_frame: String,
    resolution: Resolution,
}

impl DeliveryEncoderApp {
    fn new() -> Self {
        let (ffmpeg_path, ffprobe_path, _) = find_ffmpeg();

        Self {
            output_dir: PathBuf::from("output"),
            status: "Ready".to_string(),
            progress: 0.0,
            encoding: false,
            worker_thread: None,
            progress_receiver: mpsc::channel().1,
            cancel_sender: None,
            ffmpeg_path,
            ffprobe_path,
            current_frame: "Frame: 0000 | Idle | ETA: --:--".to_string(),
            resolution: Resolution::K6,
        }
    }

    fn start_encoding(&mut self) {
        if self.encoding {
            return;
        }

        // Hardcoded paths to assets
        let input_video = PathBuf::from("assets/video.mov");
        let overlay_image = PathBuf::from("assets/overlay.png");

        // Validation checks
        let validation_errors = [
            (
                !self.ffmpeg_path.exists(),
                format!("Error: FFmpeg not found at {}", self.ffmpeg_path.display()),
            ),
            (
                !self.ffprobe_path.exists(),
                format!(
                    "Error: FFprobe not found at {}",
                    self.ffprobe_path.display()
                ),
            ),
            (
                !input_video.exists(),
                "Error: Input video not found in assets".into(),
            ),
            (
                !overlay_image.exists(),
                "Error: Overlay image not found in assets".into(),
            ),
            (
                fs::create_dir_all(&self.output_dir).is_err(),
                "Error creating output directory".into(),
            ),
        ];

        if let Some((_, error)) = validation_errors.iter().find(|(cond, _)| *cond) {
            self.status = error.clone();
            self.current_frame = format!("Frame: 0000 | {} | ETA: --:--", error);
            return;
        }

        // NEW: Storage availability check
        match self.check_storage_availability() {
            Ok(required_gb) => {
                self.status = format!(
                    "Starting... | Free space available: {:.2}GB required",
                    required_gb
                );
            }
            Err(e) => {
                self.status = format!("Storage error: {}", e);
                self.current_frame = format!("Frame: 0000 | {} | ETA: --:--", self.status);
                return;
            }
        }

        self.status = "Encoding...".to_string();
        self.encoding = true;
        self.progress = 0.0;
        self.current_frame = "Frame: 0000 | Starting FFmpeg | ETA: --:--".to_string();

        let (progress_sender, progress_receiver) = mpsc::channel();
        let (cancel_sender, cancel_receiver) = mpsc::channel();

        self.progress_receiver = progress_receiver;
        self.cancel_sender = Some(cancel_sender);

        // Clone only what's needed for the thread
        let config = EncodingConfig {
            input_video,
            overlay_image,
            output_dir: self.output_dir.clone(),
            ffmpeg_path: self.ffmpeg_path.clone(),
            ffprobe_path: self.ffprobe_path.clone(),
            resolution: self.resolution,
        };

        let frame_sender = progress_sender.clone();
        self.worker_thread = Some(thread::spawn(move || {
            if let Err(e) = run_encoding(&config, progress_sender, cancel_receiver) {
                let _ = frame_sender.send((-1.0, format!("Error: {}", e)));
            }
        }));
    }

    // NEW: Storage check function
    fn check_storage_availability(&self) -> Result<f64> {
        // Get target resolution dimensions
        let (width, height) = match self.resolution {
            Resolution::K2 => (2048, 2048),
            Resolution::K4 => (4096, 4096),
            Resolution::K6 => {
                get_resolution(&PathBuf::from("assets/video.mov"), &self.ffprobe_path)?
            }
        };

        // Calculate bytes per frame (conservative estimate for PNG)
        let bytes_per_frame = (width as u64) * (height as u64) * 4; // 4 bytes per pixel (RGBA)

        // Get video duration and frame rate to estimate total frames
        let duration = get_duration(&PathBuf::from("assets/video.mov"), &self.ffprobe_path)?;
        let frame_rate = get_frame_rate(&PathBuf::from("assets/video.mov"), &self.ffprobe_path)?;
        let total_frames = (duration * frame_rate).ceil() as u64;

        // Calculate total required space with 20% buffer
        let required_bytes = bytes_per_frame * total_frames;
        let required_bytes_with_buffer = (required_bytes as f64 * 1.2) as u64;

        // Get available space
        let free_space = available_space(&self.output_dir)?;

        // Check if sufficient space is available
        if free_space < required_bytes_with_buffer {
            let required_gb = required_bytes_with_buffer as f64 / (1024.0 * 1024.0 * 1024.0);
            let available_gb = free_space as f64 / (1024.0 * 1024.0 * 1024.0);
            return Err(anyhow!(
                "Insufficient storage: {:.2}GB required, {:.2}GB available",
                required_gb,
                available_gb
            ));
        }

        Ok(required_bytes_with_buffer as f64 / (1024.0 * 1024.0 * 1024.0))
    }

    fn cancel_encoding(&mut self) {
        if let Some(sender) = self.cancel_sender.take() {
            let _ = sender.send(());
        }
        self.encoding = false;
        self.status = "Paused".to_string();
    }
}

// NEW: Frame rate detection function
fn get_frame_rate(input: &Path, ffprobe_path: &Path) -> Result<f32> {
    let input_str = input
        .to_str()
        .ok_or_else(|| anyhow!("Invalid video path"))?;

    let mut command = Command::new(ffprobe_path);
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=r_frame_rate",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            input_str,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = {
        #[cfg(windows)]
        {
            command.creation_flags(0x08000000).output()?
        }
        #[cfg(not(windows))]
        {
            command.output()?
        }
    };

    if !output.status.success() {
        return Err(anyhow!(
            "FFprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let rate_str = String::from_utf8(output.stdout)?;
    let rate_str = rate_str.trim();

    // Handle fractional frame rates (e.g., "30000/1001")
    if let Some((num, den)) = rate_str.split_once('/') {
        let numerator: f32 = num.parse()?;
        let denominator: f32 = den.parse()?;
        Ok(numerator / denominator)
    } else {
        rate_str
            .parse::<f32>()
            .map_err(|e| anyhow!("Frame rate parse error: {}", e))
    }
}

struct EncodingConfig {
    input_video: PathBuf,
    overlay_image: PathBuf,
    output_dir: PathBuf,
    ffmpeg_path: PathBuf,
    ffprobe_path: PathBuf,
    resolution: Resolution,
}

impl eframe::App for DeliveryEncoderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle progress updates
        while let Ok((progress, message)) = self.progress_receiver.try_recv() {
            if progress < 0.0 {
                // Error message
                self.status = message.clone();
                self.encoding = false;
                self.current_frame = message;
            } else if progress >= 100.0 {
                // Completion message
                self.progress = 100.0;
                self.status = "Done!".to_string();
                self.encoding = false;
                self.current_frame = message;
            } else {
                // Update progress percentage regardless of message type
                self.progress = progress;

                // Then check if it's a frame status update
                if message.starts_with("FRAME:") {
                    // Frame status update
                    self.current_frame = message[6..].to_string();
                }
            }
        }

        // Clean up finished worker thread
        if let Some(handle) = self.worker_thread.take() {
            if handle.is_finished() {
                self.cancel_sender = None;
            } else {
                self.worker_thread = Some(handle);
            }
        }

        // Create a frame with padding around the entire UI
        egui::CentralPanel::default()
            .frame(egui::Frame {
                inner_margin: egui::Margin::symmetric(20.0, 20.0),
                fill: ctx.style().visuals.panel_fill,
                ..Default::default()
            })
            .show(ctx, |ui| {
                // Resolution selection - changed to dropdown
                ui.horizontal(|ui| {
                    ui.label("Resolution:");
                    egui::ComboBox::from_id_source("resolution_combo")
                        .selected_text(self.resolution.as_str())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut self.resolution,
                                Resolution::K2,
                                Resolution::K2.as_str(),
                            );
                            ui.selectable_value(
                                &mut self.resolution,
                                Resolution::K4,
                                Resolution::K4.as_str(),
                            );
                            ui.selectable_value(
                                &mut self.resolution,
                                Resolution::K6,
                                Resolution::K6.as_str(),
                            );
                        });
                });

                ui.separator();

                // Output Directory
                ui.horizontal(|ui| {
                    ui.label("Output Directory:");
                    if ui.button("📂 Browse...").clicked() {
                        if let Some(path) = FileDialog::new().pick_folder() {
                            self.output_dir = path;
                        }
                    }
                    ui.label(self.output_dir.display().to_string());
                });

                ui.separator();

                // Detailed progress information (includes ETA)
                ui.label(&self.current_frame);

                // Progress bar with percentage
                ui.add(
                    egui::ProgressBar::new(self.progress / 100.0)
                        .text(format!("{:.1}%", self.progress)),
                );

                ui.add_space(10.0);

                // Action buttons
                ui.horizontal(|ui| {
                    if self.encoding {
                        if ui.button("⛔ Stop").clicked() {
                            self.cancel_encoding();
                        }
                    } else if ui.button("▶ Start Encoding").clicked() {
                        self.start_encoding();
                    }

                    if ui.button("📂 Open Output Folder").clicked() {
                        open_folder(&self.output_dir);
                    }
                });
            });
    }
}

fn run_encoding(
    config: &EncodingConfig,
    progress_sender: Sender<(f32, String)>,
    cancel_receiver: Receiver<()>,
) -> Result<()> {
    let duration = get_duration(&config.input_video, &config.ffprobe_path)?;
    let resolution = get_resolution(&config.input_video, &config.ffprobe_path)?;
    let (width, height) = (resolution.0, resolution.1);

    // Find existing frames to determine start number
    let mut max_frame = 0;
    if let Ok(entries) = fs::read_dir(&config.output_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                if file_name.starts_with("frame_") && file_name.ends_with(".png") {
                    let num_str = &file_name[6..file_name.len() - 4];
                    if let Ok(num) = num_str.parse::<u32>() {
                        if num > max_frame {
                            max_frame = num;
                        }
                    }
                }
            }
        }
    }
    // Start from last frame
    let start_frame = max_frame;

    let temp_progress = tempfile::NamedTempFile::new()?;
    let progress_path = temp_progress.path().to_path_buf();

    // Handle resolution scaling
    let (target_width, target_height) = match config.resolution.target_size() {
        Some((w, h)) => (w, h),
        None => (width, height),
    };

    let filter_complex = if config.resolution != Resolution::K6 {
        format!(
            "[0:v]select=gte(n\\,{}),scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2[vid]; \
             [1:v]scale={}:{}[ovr]; \
             [vid][ovr]overlay=0:0",
            max_frame, target_width, target_height, target_width, target_height, target_width, target_height
        )
    } else {
        format!(
            "[0:v]select=gte(n\\,{})[selected]; \
             [1:v]scale={}:{}[ovr]; \
             [selected][ovr]overlay=0:0",
            max_frame, width, height
        )
    };

    let mut cmd = Command::new(&config.ffmpeg_path);
    cmd.arg("-i")
        .arg(&config.input_video)
        .arg("-i")
        .arg(&config.overlay_image)
        .arg("-filter_complex")
        .arg(&filter_complex)
        .arg("-vsync")
        .arg("0")
        .arg("-start_number")
        .arg(start_frame.to_string())
        .arg("-progress")
        .arg(&progress_path)
        .arg(config.output_dir.join("frame_%04d.png"))
        .arg("-y")
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // Spawn FFmpeg with hidden window on Windows
    let mut child = {
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000).spawn()? // CREATE_NO_WINDOW
        }
        #[cfg(not(windows))]
        {
            cmd.spawn()?
        }
    };

    let start_time = Instant::now();

    // Send immediate update that FFmpeg has started
    let _ = progress_sender.send((
        0.0,
        format!(
            "FRAME:Frame: {:04} | Processing | Res: {}x{} | Start: {:04} | ETA: --:--",
            start_frame, target_width, target_height, start_frame
        ),
    ));

    // Track last frame and ETA for consistent status
    let mut last_frame = start_frame;
    let mut last_eta = "--:--".to_string();

    while child.try_wait()?.is_none() {
        // Handle cancel requests
        if cancel_receiver.try_recv().is_ok() {
            child.kill()?;
            let _ = progress_sender.send((
                -2.0,
                format!(
                    "FRAME:Frame: {:04} | Paused | ETA: {}",
                    last_frame, last_eta
                ),
            ));
            return Ok(());
        }

        // Read and parse progress file
        if let Ok(contents) = fs::read_to_string(&progress_path) {
            let mut progress_value = 0.0;
            let mut current_frame = start_frame;

            for line in contents.lines() {
                if line.starts_with("frame=") {
                    if let Some(frame_str) = line.split('=').nth(1) {
                        if let Ok(frame_num) = frame_str.trim().parse::<u32>() {
                            current_frame = frame_num;
                            last_frame = frame_num;
                        }
                    }
                } else if line.starts_with("out_time_ms") {
                    if let Some((_, time_str)) = line.split_once('=') {
                        if let Ok(out_time_ms) = time_str.parse::<u64>() {
                            let current_secs = out_time_ms / 1_000_000;
                            if duration > 0.0 {
                                progress_value =
                                    (current_secs as f32 / duration * 100.0).min(100.0);

                                let elapsed = start_time.elapsed().as_secs_f32();
                                if progress_value > 0.1 {
                                    let total_estimated = (elapsed * 100.0) / progress_value;
                                    let eta_secs = (total_estimated - elapsed) as u64;
                                    last_eta = format!("{:02}:{:02}", eta_secs / 60, eta_secs % 60);
                                } else {
                                    last_eta = "--:--".to_string();
                                }
                            }
                        }
                    }
                }
            }

            // Create detailed single-line log with frame and ETA
            let detailed_log = if config.resolution != Resolution::K6 {
                format!(
                    "FRAME:Frame: {:04} | Progress: {:.1}% | Res: {}x{} | ETA: {}",
                    current_frame, progress_value, target_width, target_height, last_eta
                )
            } else {
                format!(
                    "FRAME:Frame: {:04} | Progress: {:.1}% | Res: {}x{} (orig) | ETA: {}",
                    current_frame, progress_value, width, height, last_eta
                )
            };

            // Send detailed log update with FRAME: prefix
            let _ = progress_sender.send((progress_value, detailed_log));
        }

        thread::sleep(Duration::from_millis(200));
    }

    // Final check after process completes
    let status = child.wait()?;
    if status.success() {
        // Final detailed status with frame and ETA
        let detailed_log = if config.resolution != Resolution::K6 {
            format!(
                "FRAME:Frame: {:04} | Progress: 100.0% | Res: {}x{} | ETA: 00:00",
                last_frame, target_width, target_height
            )
        } else {
            format!(
                "FRAME:Frame: {:04} | Progress: 100.0% | Res: {}x{} (orig) | ETA: 00:00",
                last_frame, width, height
            )
        };

        let _ = progress_sender.send((100.0, detailed_log));
        Ok(())
    } else {
        Err(anyhow!(
            "FFmpeg exited with error at frame {} (ETA: {}): {}",
            last_frame,
            last_eta,
            status
        ))
    }
}

fn get_resolution(input: &Path, ffprobe_path: &Path) -> Result<(u32, u32)> {
    let input_str = input
        .to_str()
        .ok_or_else(|| anyhow!("Invalid video path"))?;

    let mut command = Command::new(ffprobe_path);
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "csv=p=0",
            input_str,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000).output()?
        }
        #[cfg(not(windows))]
        {
            command.output()?
        }
    };

    if !output.status.success() {
        return Err(anyhow!(
            "FFprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let res_str = String::from_utf8(output.stdout)?.trim().to_string();
    let parts: Vec<&str> = res_str.split(',').collect();
    if parts.len() != 2 {
        return Err(anyhow!("Unexpected resolution format: {}", res_str));
    }

    let width = parts[0].parse::<u32>()?;
    let height = parts[1].parse::<u32>()?;

    Ok((width, height))
}

fn get_duration(input: &Path, ffprobe_path: &Path) -> Result<f32> {
    let input_str = input
        .to_str()
        .ok_or_else(|| anyhow!("Invalid video path"))?;

    let mut command = Command::new(ffprobe_path);
    command
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            input_str,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000).output()?
        }
        #[cfg(not(windows))]
        {
            command.output()?
        }
    };

    if !output.status.success() {
        return Err(anyhow!(
            "FFprobe failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    String::from_utf8(output.stdout)?
        .trim()
        .parse::<f32>()
        .map_err(|e| anyhow!("Duration parse error: {}", e))
}

fn open_folder(path: &Path) {
    let command = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };

    let _ = Command::new(command).arg(path).spawn();
}

fn find_ffmpeg() -> (PathBuf, PathBuf, String) {
    let (ffmpeg_name, ffprobe_name) = if cfg!(windows) {
        ("ffmpeg.exe", "ffprobe.exe")
    } else {
        ("ffmpeg", "ffprobe")
    };

    // Check common locations
    let locations = [
        PathBuf::from(ffmpeg_name),
        PathBuf::from("assets").join(ffmpeg_name),
        PathBuf::from("ffmpeg").join(ffmpeg_name),
    ];

    for path in &locations {
        let ffprobe_path = path.with_file_name(ffprobe_name);
        if path.exists() && ffprobe_path.exists() {
            return (path.clone(), ffprobe_path, String::new());
        }
    }

    // Check system PATH
    if let Ok(path) = env::var("PATH") {
        for dir in env::split_paths(&path) {
            let ffmpeg_path = dir.join(ffmpeg_name);
            let ffprobe_path = dir.join(ffprobe_name);
            if ffmpeg_path.exists() && ffprobe_path.exists() {
                return (ffmpeg_path, ffprobe_path, String::new());
            }
        }
    }

    // Default to executable names
    (
        PathBuf::from(ffmpeg_name),
        PathBuf::from(ffprobe_name),
        String::new(),
    )
}
