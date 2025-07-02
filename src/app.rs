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
    pub progress_receiver: Receiver<(f32, String)>,
    pub cancel_sender: Option<Sender<()>>,
    pub ffmpeg_path: PathBuf,
    pub ffprobe_path: PathBuf,
    pub current_frame: String,
    pub resolution: Resolution,
}

impl DeliveryEncoderApp {
    pub fn new() -> Self {
        let (ffmpeg_path, ffprobe_path, _) = find_ffmpeg();

        Self {
            output_dir: PathBuf::from("output"),
            status: "Ready".to_string(),
            progress: 0.0,
            encoding: false,
            worker_thread: None,
            progress_receiver: std::sync::mpsc::channel().1,
            cancel_sender: None,
            ffmpeg_path,
            ffprobe_path,
            current_frame: "Frame: 0000 | Idle | ETA: --:--".to_string(),
            resolution: Resolution::K6,
        }
    }

    pub fn start_encoding(&mut self) {
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
                std::fs::create_dir_all(&self.output_dir).is_err(),
                "Error creating output directory".into(),
            ),
        ];

        if let Some((_, error)) = validation_errors.iter().find(|(cond, _)| *cond) {
            self.status = error.clone();
            self.current_frame = format!("Frame: 0000 | {} | ETA: --:--", error);
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
                self.current_frame = format!("Frame: 0000 | {} | ETA: --:--", self.status);
                return;
            }
        }

        self.status = "Encoding...".to_string();
        self.encoding = true;
        self.progress = 0.0;
        self.current_frame = "Frame: 0000 | Starting FFmpeg | ETA: --:--".to_string();

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
        };

        let frame_sender = progress_sender.clone();
        self.worker_thread = Some(thread::spawn(move || {
            if let Err(e) = run_encoding(&config, progress_sender, cancel_receiver) {
                let _ = frame_sender.send((-1.0, format!("Error: {}", e)));
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
            Resolution::K6 => {
                get_resolution(&PathBuf::from("assets/video.mov"), &self.ffprobe_path)?
            }
        };

        // Calculate bytes per frame
        let bytes_per_frame = (width as u64) * (height as u64) * 4; // 4 bytes per pixel (RGBA)

        // Get video duration and frame rate
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
                // Update progress percentage
                self.progress = progress;
                // Always update the status line with the message
                self.current_frame = message.clone();
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
