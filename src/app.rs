use anyhow::{anyhow, Result};
use eframe::egui;
use rfd::FileDialog;
use std::{
    path::PathBuf,
    sync::mpsc::{Receiver, Sender},
    thread,
};

use crate::{
    encoding::{run_encoding, EncodingConfig},
    models::Resolution,
    utils::{find_ffmpeg, get_duration, get_frame_rate, get_resolution, open_folder},
};

pub struct DeliveryEncoderApp {
    pub output_dir: PathBuf,
    pub status: String,
    pub progress: f32,
    pub encoding: bool,
    pub worker_thread: Option<thread::JoinHandle<()>>,
    pub progress_receiver: Receiver<(f32, u32, String)>,
    pub cancel_sender: Option<Sender<()>>,
    pub ffmpeg_path: PathBuf,
    pub ffprobe_path: PathBuf,
    pub current_frame: String,
    pub resolution: Resolution,
    pub input_video: PathBuf,
    pub sufficient_storage: bool,
    pub storage_error: Option<String>,
    pub base_name: String, // Added to store base name
}

impl DeliveryEncoderApp {
    pub fn new() -> Self {
        let (ffmpeg_path, ffprobe_path, _) = find_ffmpeg();

        // Find first .mov file in assets directory
        let input_video = std::fs::read_dir("assets")
            .and_then(|entries| {
                entries
                    .filter_map(Result::ok)
                    .find(|entry| {
                        entry.path().is_file()
                            && entry.path().extension().map_or(false, |ext| ext == "mov")
                    })
                    .map(|entry| entry.path())
                    .ok_or_else(|| {
                        std::io::Error::new(std::io::ErrorKind::NotFound, "No .mov file found")
                    })
            })
            .unwrap_or_else(|_| PathBuf::from("assets/video.mov"));

        // Get base filename
        let base_name = input_video
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "video".to_string());

        // Set output directory based on video filename
        let output_dir = PathBuf::from("output").join(&base_name);

        let mut app = Self {
            output_dir,
            status: "Ready".to_string(),
            progress: 0.0,
            encoding: false,
            worker_thread: None,
            progress_receiver: std::sync::mpsc::channel().1,
            cancel_sender: None,
            ffmpeg_path,
            ffprobe_path,
            current_frame: "File: -- | Idle | ETA: --:--".to_string(),
            resolution: Resolution::K6,
            input_video,
            sufficient_storage: false,
            storage_error: None,
            base_name, // Store base name
        };
        app.update_storage_status();
        app
    }

    pub fn update_storage_status(&mut self) {
        match self.check_storage_availability() {
            Ok(_) => {
                self.sufficient_storage = true;
                self.storage_error = None;
            }
            Err(e) => {
                self.sufficient_storage = false;
                self.storage_error = Some(e.to_string());
            }
        }
    }

    pub fn start_encoding(&mut self) {
        if self.encoding {
            return;
        }

        // Use found input video
        let input_video = self.input_video.clone();

        // Select overlay based on resolution
        let overlay_image = match self.resolution {
            Resolution::K2 => PathBuf::from("assets/overlay_2k.png"),
            Resolution::K4 => PathBuf::from("assets/overlay_4k.png"),
            Resolution::K6 => PathBuf::from("assets/overlay_6k.png"),
        };

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
                format!("Error: Input video not found at {}", input_video.display()),
            ),
            (
                !overlay_image.exists(),
                format!(
                    "Error: Overlay image not found at {}",
                    overlay_image.display()
                ),
            ),
            (
                std::fs::create_dir_all(&self.output_dir).is_err(),
                "Error creating output directory".into(),
            ),
        ];

        if let Some((_, error)) = validation_errors.iter().find(|(cond, _)| *cond) {
            self.status = error.clone();
            self.current_frame = format!("File: -- | {} | ETA: --:--", error);
            return;
        }

        // Storage availability check
        match self.check_storage_availability() {
            Ok(required_gb) => {
                self.status = format!(
                    "Starting... | Free space available: {:.2}GB required",
                    required_gb
                );
            }
            Err(e) => {
                self.status = format!("Storage error: {}", e);
                self.current_frame = format!("File: -- | {} | ETA: --:--", self.status);
                return;
            }
        }

        self.status = "Encoding...".to_string();
        self.encoding = true;
        self.progress = 0.0;

        // Find existing frames to determine start number
        let mut max_frame = 0;
        if let Ok(entries) = std::fs::read_dir(&self.output_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                    // Match files with base name followed by HYPHEN
                    if file_name.starts_with(&self.base_name)
                        && file_name.contains('-')
                        && file_name.ends_with(".png")
                    {
                        let num_part = file_name
                            .trim_start_matches(&self.base_name)
                            .trim_start_matches('-') // Changed to hyphen
                            .trim_end_matches(".png");
                        if let Ok(num) = num_part.parse::<u32>() {
                            if num > max_frame {
                                max_frame = num;
                            }
                        }
                    }
                }
            }
        }

        // Generate first file name with HYPHEN
        let first_file = format!("{}-{:04}.png", self.base_name, max_frame);
        self.current_frame = format!("File: {} | Starting FFmpeg | ETA: --:--", first_file);

        let (progress_sender, progress_receiver) = std::sync::mpsc::channel();
        let (cancel_sender, cancel_receiver) = std::sync::mpsc::channel();

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
            base_name: self.base_name.clone(), // Use app's base name
        };

        let frame_sender = progress_sender.clone();
        self.worker_thread = Some(thread::spawn(move || {
            if let Err(e) = run_encoding(&config, progress_sender, cancel_receiver) {
                let _ = frame_sender.send((-1.0, 0, format!("Error: {}", e)));
            }
        }));
    }

    // Storage check function
    fn check_storage_availability(&self) -> Result<f64> {
        use fs2::available_space;

        // Get target resolution dimensions
        let (width, height) = match self.resolution {
            Resolution::K2 => (2048, 2048),
            Resolution::K4 => (4096, 4096),
            Resolution::K6 => get_resolution(&self.input_video, &self.ffprobe_path)?,
        };

        // Calculate bytes per frame
        let bytes_per_frame = (width as u64) * (height as u64) * 4; // 4 bytes per pixel (RGBA)

        // Get video duration and frame rate
        let duration = get_duration(&self.input_video, &self.ffprobe_path)?;
        let frame_rate = get_frame_rate(&self.input_video, &self.ffprobe_path)?;
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

    pub fn cancel_encoding(&mut self) {
        if let Some(sender) = self.cancel_sender.take() {
            let _ = sender.send(());
        }
        self.encoding = false;
        self.status = "Paused".to_string();
    }
}

impl eframe::App for DeliveryEncoderApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Handle progress updates
        while let Ok((progress, frame, message)) = self.progress_receiver.try_recv() {
            // Generate file name with HYPHEN
            let file_name = format!("{}-{:04}.png", self.base_name, frame);
            let full_message = format!("File: {} | {}", file_name, message);

            if progress < 0.0 {
                // Error message
                self.status = full_message.clone();
                self.encoding = false;
                self.current_frame = full_message;
            } else if progress >= 100.0 {
                // Completion message
                self.progress = 100.0;
                self.status = "Done!".to_string();
                self.encoding = false;
                self.current_frame = full_message;
            } else {
                // Update progress percentage
                self.progress = progress;
                // Always update the status line with the message
                self.current_frame = full_message;
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

        // Request continuous repaints during encoding
        if self.encoding {
            ctx.request_repaint();
        }

        // Create a frame with padding around the entire UI
        egui::CentralPanel::default()
            .frame(egui::Frame {
                inner_margin: egui::Margin::symmetric(20.0, 20.0),
                fill: ctx.style().visuals.panel_fill,
                ..Default::default()
            })
            .show(ctx, |ui| {
                // Resolution selection
                let prev_resolution = self.resolution;
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
                if prev_resolution != self.resolution {
                    self.update_storage_status();
                }

                ui.separator();

                // Output Directory
                ui.horizontal(|ui| {
                    ui.label("Output Directory:");
                    if ui.button("📂 Browse...").clicked() {
                        if let Some(path) = FileDialog::new().pick_folder() {
                            self.output_dir = path;
                            self.update_storage_status();
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

                // Show storage error if exists and not encoding
                if !self.encoding {
                    if let Some(err) = &self.storage_error {
                        ui.colored_label(egui::Color32::RED, err);
                    }
                }

                ui.add_space(10.0);

                // Action buttons
                ui.horizontal(|ui| {
                    if self.encoding {
                        if ui.button("⛔ Stop").clicked() {
                            self.cancel_encoding();
                        }
                    } else {
                        let start_enabled = self.sufficient_storage;
                        if ui
                            .add_enabled(start_enabled, egui::Button::new("▶ Start Encoding"))
                            .clicked()
                        {
                            self.start_encoding();
                        }
                    }

                    if ui.button("📂 Open Output Folder").clicked() {
                        open_folder(&self.output_dir);
                    }
                });
            });
    }
}
