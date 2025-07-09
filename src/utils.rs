use anyhow::{anyhow, Result};
use std::{
    env,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

pub fn open_folder(path: &Path) {
    let command = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };

    let _ = Command::new(command).arg(path).spawn();
}

pub fn find_ffmpeg() -> (PathBuf, PathBuf, String) {
    let (ffmpeg_name, ffprobe_name) = if cfg!(windows) {
        ("ffmpeg.exe", "ffprobe.exe")
    } else {
        ("ffmpeg", "ffprobe")
    };

    let locations = [
        PathBuf::from(ffmpeg_name),
        PathBuf::from("assets").join("ffmpeg").join(ffmpeg_name),
        PathBuf::from("ffmpeg").join(ffmpeg_name),
    ];

    for path in &locations {
        let ffprobe_path = path.with_file_name(ffprobe_name);
        if path.exists() && ffprobe_path.exists() {
            return (path.clone(), ffprobe_path, String::new());
        }
    }

    if let Ok(path) = env::var("PATH") {
        for dir in env::split_paths(&path) {
            let ffmpeg_path = dir.join(ffmpeg_name);
            let ffprobe_path = dir.join(ffprobe_name);
            if ffmpeg_path.exists() && ffprobe_path.exists() {
                return (ffmpeg_path, ffprobe_path, String::new());
            }
        }
    }

    (
        PathBuf::from(ffmpeg_name),
        PathBuf::from(ffprobe_name),
        String::new(),
    )
}

pub fn get_resolution(input: &Path, ffprobe_path: &Path) -> Result<(u32, u32)> {
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

pub fn get_duration(input: &Path, ffprobe_path: &Path) -> Result<f32> {
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

pub fn get_frame_rate(input: &Path, ffprobe_path: &Path) -> Result<f32> {
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
            "stream=avg_frame_rate", // Changed to avg_frame_rate
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
