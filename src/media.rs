use std::io;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::fs;
use regex::Regex;



pub fn download_youtube_video(video_id: &str, relative_path: &Path, start_time: Option<u64>, end_time: Option<u64>, duration: Option<u64>) -> io::Result<PathBuf> {
    // Use the 'which' crate to resolve yt-dlp and ffmpeg
    let _ = which::which("yt-dlp").map_err(|_| {
        io::Error::new(ErrorKind::NotFound, "yt-dlp not found in PATH")
    })?;
    let _ = which::which("ffmpeg").map_err(|_| {
        io::Error::new(ErrorKind::NotFound, "ffmpeg not found in PATH")
    })?;
    
    // Remove any extension from the relative_path before passing to yt-dlp
    let mut output_path_str = relative_path.to_string_lossy().to_string();
    if let Some(dotidx) = output_path_str.rfind('.') {
        output_path_str.truncate(dotidx);
    }
    // Use yt-dlp's template to add the correct extension
    let output_template = format!("{}.%(ext)s", output_path_str);
    let output_path = Path::new(&output_template);
    let media_path = output_path.parent().unwrap_or_else(|| Path::new("media"));

    // Check if the media directory exists, if not, create it
    if !media_path.exists() {
        fs::create_dir_all(media_path)?;
    }
    // Compute the expected full output path
    let expected_full_output = media_path.join(format!("{}.full.mp4", video_id));
    // Construct the YouTube URL from the video_id
    let youtube_url = format!("https://www.youtube.com/watch?v={}", video_id);
    let resolved_full_output = if expected_full_output.exists() {
        expected_full_output
    } else {
        // Use yt-dlp to resolve the output filename for the full video (dry run)
        let output_template = format!("{}{}.full.%(ext)s", media_path.display(), video_id);
        let print_path = Command::new("yt-dlp")
            .arg("-o").arg(&output_template)
            .arg("--print").arg("after_move:filepath")
            .arg(&youtube_url)
            .output();
        match print_path {
            Ok(out) if out.status.success() => {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !s.is_empty() { PathBuf::from(s) } else { media_path.join(format!("{}.full.mp4", video_id)) }
            },
            _ => media_path.join(format!("{}.full.mp4", video_id)),
        }
    };
    let full_output = &resolved_full_output;
    // Determine the correct trimmed output file path
    let trimmed_output = if relative_path.is_dir() {
        // If only a directory is given, construct a filename
        let mut fname = format!("{}", video_id);
        if let Some(start) = start_time {
            fname.push_str(&format!("_start{}", start));
        }
        if let Some(end) = end_time {
            fname.push_str(&format!("_end{}", end));
        }
        fname.push_str(".mp4");
        relative_path.join(fname)
    } else {
        relative_path.to_path_buf()
    };
    // Only reuse a trimmed file if its _start and _end match the request exactly (and no extra params)
    let trimmed_file_stem = trimmed_output.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let re_start = Regex::new(r"_start(\d+)").unwrap();
    let re_end = Regex::new(r"_end(\d+)").unwrap();
    let file_start = re_start.captures(trimmed_file_stem).and_then(|cap| cap.get(1)).and_then(|m| m.as_str().parse::<u64>().ok());
    let file_end = re_end.captures(trimmed_file_stem).and_then(|cap| cap.get(1)).and_then(|m| m.as_str().parse::<u64>().ok());
    // Extra strict: if end_time is None, filename must not contain _end; if start_time is None, must not contain _start
    let has_start = re_start.is_match(trimmed_file_stem);
    let has_end = re_end.is_match(trimmed_file_stem);
    let start_match = start_time == file_start && (start_time.is_some() == has_start);
    let end_match = end_time == file_end && (end_time.is_some() == has_end);
    let should_reuse = match (start_time, end_time) {
        (Some(_), Some(_)) => start_match && end_match,
        (Some(_), None) => start_match && !has_end,
        (None, Some(_)) => end_match && !has_start,
        (None, None) => !has_start && !has_end,
    };
    // Download full video if not already present
    if !full_output.exists() {
        let mut cmd = Command::new("yt-dlp");
        cmd.arg("-f").arg("bestvideo[ext=mp4]+bestaudio[ext=m4a]/mp4");
        cmd.arg("--remux-video").arg("mp4");
        cmd.arg("-o").arg(full_output.to_str().unwrap());
        cmd.arg(&youtube_url);
        let status = dbg!(cmd.status());
        if !status.as_ref().map(|s| s.success()).unwrap_or(false) {
            return match status {
                Ok(status) => Err(io::Error::new(
                    ErrorKind::Other,
                    format!("yt-dlp failed with exit code: {}", status.code().unwrap_or(-1)),
                )),
                Err(e) => Err(e),
            };
        }
    } else {
        println!("Full video already exists: {}", full_output.display());
    }
    // Check for incomplete download files (.part, .ytdl)
    let part_file = full_output.with_extension(format!("{}part", full_output.extension().and_then(|e| e.to_str()).unwrap_or("")));
    let ytdl_file = full_output.with_extension(format!("{}ytdl", full_output.extension().and_then(|e| e.to_str()).unwrap_or("")));
    if part_file.exists() || ytdl_file.exists() {
        return Err(io::Error::new(
            ErrorKind::Other,
            format!("Full video is still downloading or incomplete: {}", full_output.display()),
        ));
    }
    // Parse start and end time from the filename if present (e.g. ..._start123_end456.mp4)
    let mut start_time = start_time;
    let mut end_time = end_time;
    let file_stem = trimmed_output.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let re = Regex::new(r"_start(\d+)").ok();
    if let Some(re) = &re {
        if let Some(cap) = re.captures(file_stem) {
            if let Some(m) = cap.get(1) {
                if let Ok(val) = m.as_str().parse::<u64>() {
                    start_time = Some(val);
                }
            }
        }
    }
    let re = Regex::new(r"_end(\d+)").ok();
    if let Some(re) = &re {
        if let Some(cap) = re.captures(file_stem) {
            if let Some(m) = cap.get(1) {
                if let Ok(val) = m.as_str().parse::<u64>() {
                    end_time = Some(val);
                }
            }
        }
    }
    // If duration is set and end_time is not, compute end_time = start_time + duration
    if duration.is_some() && end_time.is_none() {
        let s = start_time.unwrap_or(0);
        end_time = Some(s + duration.unwrap());
    }
    // If a section is requested, trim with ffmpeg (support start and end)
    if start_time.is_some() || end_time.is_some() {
        // Only trim if the trimmed file does not already exist with exact match
        if !trimmed_output.exists() || !should_reuse {
            let mut ffmpeg_cmd = Command::new("ffmpeg");
            ffmpeg_cmd.arg("-y");
            let (start, end) = (start_time, end_time);
            if let Some(start) = start {
                let hours = start / 3600;
                let minutes = (start % 3600) / 60;
                let seconds = start % 60;
                let start_str = format!("{:02}:{:02}:{:02}", hours, minutes, seconds);
                ffmpeg_cmd.arg("-ss").arg(&start_str);
            }
            ffmpeg_cmd.arg("-i").arg(full_output.to_str().unwrap());
            // Use -t (duration) if both start and end are set
            if let (Some(start), Some(end)) = (start, end) {
                if end > start {
                    let duration = end - start;
                    let hours = duration / 3600;
                    let minutes = (duration % 3600) / 60;
                    let seconds = duration % 60;
                    let duration_str = format!("{:02}:{:02}:{:02}", hours, minutes, seconds);
                    ffmpeg_cmd.arg("-t").arg(&duration_str);
                } else {
                    return Err(io::Error::new(
                        ErrorKind::InvalidInput,
                        format!("end_time ({}) must be greater than start_time ({})", end, start),
                    ));
                }
            } else if let (None, Some(end)) = (start, end) {
                let hours = end / 3600;
                let minutes = (end % 3600) / 60;
                let seconds = end % 60;
                let end_str = format!("{:02}:{:02}:{:02}", hours, minutes, seconds);
                ffmpeg_cmd.arg("-to").arg(&end_str);
            }
            // Use re-encode for MP4 output to ensure compatibility
            ffmpeg_cmd.arg("-c:v").arg("libx264").arg("-c:a").arg("aac");
            ffmpeg_cmd.arg(&trimmed_output);
            // Print the full ffmpeg command for debugging
            let cmdline = format!(
                "ffmpeg {}",
                ffmpeg_cmd
                    .get_args()
                    .map(|a| a.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            println!("[DEBUG] Running: {}", cmdline);
            let ffmpeg_status = ffmpeg_cmd.status();
            if let Ok(status) = ffmpeg_status {
                if !status.success() {
                    return Err(io::Error::new(
                        ErrorKind::Other,
                        format!("ffmpeg failed with exit code: {}", status.code().unwrap_or(-1)),
                    ));
                }
            } else if let Err(e) = ffmpeg_status {
                return Err(io::Error::new(ErrorKind::Other, format!("ffmpeg error: {}", e)));
            }
            println!("Trimmed video to {}", trimmed_output.display());
        } else {
            println!("Trimmed video already exists: {}", trimmed_output.display());
        }
        let trimmed_output_path = trimmed_output.to_path_buf();
        println!("Video ready at {:?}", trimmed_output_path);
        Ok(trimmed_output_path)
    } else {
        // If no section, just copy the full output to the final output if needed
        if !trimmed_output.exists() {
            fs::copy(&full_output, &trimmed_output)?;
        }
        let trimmed_output_path = trimmed_output.to_path_buf();
        println!("Video ready at {:?}", trimmed_output_path);
        Ok(trimmed_output_path)
    }
}