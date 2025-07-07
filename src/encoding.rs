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
    utils::{get_duration, get_frame_rate, get_resolution},
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
    let frame_rate = get_frame_rate(&config.input_video, &config.ffprobe_path)?;
    let resolution = get_resolution(&config.input_video, &config.ffprobe_path)?;
    let (width, height) = (resolution.0, resolution.1);

    let total_frames = (duration * frame_rate).ceil() as u32;

    let output_pattern = format!("{}_%04d.png", config.base_name);
    let output_path = config.output_dir.join(&output_pattern);

    let mut max_frame = 0;
    let mut found_any = false;
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
                        found_any = true;
                    }
                }
            }
        }
    }

    let start_frame = if found_any { max_frame } else { 0 };
    let start_time_secs = start_frame as f32 / frame_rate;
    let start_time_str = format!("{:.3}", start_time_secs);

    let temp_progress = tempfile::NamedTempFile::new()?;
    let progress_path = temp_progress.path().to_path_buf();

    let (target_width, target_height) = match config.resolution.target_size() {
        Some((w, h)) => (w, h),
        None => (width, height),
    };

    let filter_complex = if config.resolution != Resolution::K6 {
        format!(
            "[0:v]scale={}:{}:force_original_aspect_ratio=decrease,pad={}:{}:(ow-iw)/2:(oh-ih)/2[vid]; \
             [1:v]scale={}:{}[ovr]; \
             [vid][ovr]overlay=0:0",
            target_width, target_height, target_width, target_height, target_width, target_height
        )
    } else {
        format!(
            "[1:v]scale={}:{}[ovr]; \
             [0:v][ovr]overlay=0:0",
            width, height
        )
    };

    let mut cmd = Command::new(&config.ffmpeg_path);
    cmd.arg("-ss")
        .arg(&start_time_str)
        .arg("-i")
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

    let mut child = {
        #[cfg(windows)]
        {
            cmd.creation_flags(0x08000000).spawn()?
        }
        #[cfg(not(windows))]
        {
            cmd.spawn()?
        }
    };

    let start_time = Instant::now();

    let initial_progress = if total_frames > 0 {
        (start_frame as f32 / total_frames as f32 * 100.0).min(100.0)
    } else {
        0.0
    };

    let _ = progress_sender.send((
        initial_progress,
        start_frame,
        format!(
            "Processing | Res: {}x{} | Start: {:04} | ETA: --:--",
            target_width, target_height, start_frame
        ),
    ));

    let mut last_eta = "--:--".to_string();
    let mut last_frame = start_frame;

    while child.try_wait()?.is_none() {
        if cancel_receiver.try_recv().is_ok() {
            child.kill()?;
            let _ = progress_sender.send((-2.0, last_frame, format!("Paused | ETA: {}", last_eta)));
            return Ok(());
        }

        if let Ok(contents) = std::fs::read_to_string(&progress_path) {
            let mut progress_value = initial_progress;

            for line in contents.lines() {
                if line.starts_with("frame=") {
                    if let Some(frame_str) = line.split('=').nth(1) {
                        if let Ok(frame_index) = frame_str.trim().parse::<u32>() {
                            last_frame = start_frame + frame_index;

                            if total_frames > 0 {
                                progress_value =
                                    (last_frame as f32 / total_frames as f32 * 100.0).min(100.0);
                            }
                        }
                    }
                } else if line.starts_with("out_time_ms") {
                    if let Some((_, time_str)) = line.split_once('=') {
                        if let Ok(_out_time_ms) = time_str.parse::<u64>() {
                            if duration > 0.0 {
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

            let detailed_log = if config.resolution != Resolution::K6 {
                format!(
                    "Processing | Res: {}x{} | ETA: {}",
                    target_width, target_height, last_eta
                )
            } else {
                format!("Processing | Res: {}x{} | ETA: {}", width, height, last_eta)
            };

            let _ = progress_sender.send((progress_value, last_frame, detailed_log));
        }

        thread::sleep(Duration::from_millis(200));
    }

    let status = child.wait()?;
    if status.success() {
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
