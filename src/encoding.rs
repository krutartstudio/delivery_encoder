use anyhow::{anyhow, Result};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc::{Receiver, Sender},
    thread,
    time::{Duration, Instant},
};

use crate::{
    models::Resolution,
    utils::{get_duration, get_resolution},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub struct EncodingConfig {
    pub input_video: PathBuf,
    pub overlay_image: PathBuf,
    pub output_dir: PathBuf,
    pub ffmpeg_path: PathBuf,
    pub ffprobe_path: PathBuf,
    pub resolution: Resolution,
    pub base_name: String,
}

pub fn run_encoding(
    config: &EncodingConfig,
    progress_sender: Sender<(f32, u32, String)>,
    cancel_receiver: Receiver<()>,
) -> Result<()> {
    let duration = get_duration(&config.input_video, &config.ffprobe_path)?;
    let resolution = get_resolution(&config.input_video, &config.ffprobe_path)?;
    let (width, height) = (resolution.0, resolution.1);

    // Create output pattern using base name
    let output_pattern = format!("{}_%04d.png", config.base_name);
    let output_path = config.output_dir.join(&output_pattern);

    // Find existing frames to determine start number
    let mut max_frame = 0;
    if let Ok(entries) = std::fs::read_dir(&config.output_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                if file_name.starts_with(&config.base_name) && file_name.ends_with(".png") {
                    let num_str = file_name
                        .trim_start_matches(&config.base_name)
                        .trim_start_matches('_')
                        .trim_end_matches(".png");
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
    let mut last_frame = start_frame;

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
        .arg(output_path)
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
        start_frame,
        format!(
            "Processing | Res: {}x{} | Start: {:04} | ETA: --:--",
            target_width, target_height, start_frame
        ),
    ));

    // Track last frame and ETA for consistent status
    let mut last_eta = "--:--".to_string();

    while child.try_wait()?.is_none() {
        // Handle cancel requests
        if cancel_receiver.try_recv().is_ok() {
            child.kill()?;
            let _ = progress_sender.send((-2.0, last_frame, format!("Paused | ETA: {}", last_eta)));
            return Ok(());
        }

        // Read and parse progress file
        if let Ok(contents) = std::fs::read_to_string(&progress_path) {
            let mut progress_value = 0.0;
            let mut current_frame_index = 0;

            for line in contents.lines() {
                if line.starts_with("frame=") {
                    if let Some(frame_str) = line.split('=').nth(1) {
                        if let Ok(frame_index) = frame_str.trim().parse::<u32>() {
                            current_frame_index = frame_index;
                            // Calculate absolute frame number by adding start offset
                            last_frame = start_frame + frame_index;
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

            // Create detailed log without file name
            let detailed_log = if config.resolution != Resolution::K6 {
                format!(
                    "Processing | Res: {}x{} | ETA: {}",
                    target_width, target_height, last_eta
                )
            } else {
                format!("Processing | Res: {}x{} | ETA: {}", width, height, last_eta)
            };

            // Send detailed log update with absolute frame number
            let _ = progress_sender.send((progress_value, last_frame, detailed_log));
        }

        thread::sleep(Duration::from_millis(200));
    }

    // Final check after process completes
    let status = child.wait()?;
    if status.success() {
        // Final detailed status with file name and ETA
        let detailed_log = if config.resolution != Resolution::K6 {
            format!(
                "Processing | Res: {}x{} | ETA: 00:00",
                target_width, target_height
            )
        } else {
            format!("Processing | Res: {}x{} | ETA: 00:00", width, height)
        };

        let _ = progress_sender.send((100.0, last_frame, detailed_log));
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
