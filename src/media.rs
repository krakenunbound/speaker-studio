use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::process::Command;

pub struct LiveWav {
    file: File,
    samples: u32,
    peaks: Vec<f32>,
    bucket: f32,
    bucket_fill: usize,
}

impl LiveWav {
    pub fn create(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = File::create(path)?;
        file.write_all(&wav_header(0))?;
        Ok(Self { file, samples: 0, peaks: Vec::new(), bucket: 0.0, bucket_fill: 0 })
    }

    pub fn push(&mut self, pcm: &[i16]) -> io::Result<()> {
        if pcm.is_empty() {
            return Ok(());
        }
        let mut bytes = Vec::with_capacity(pcm.len() * 2);
        for sample in pcm {
            bytes.extend_from_slice(&sample.to_le_bytes());
            self.note((*sample as f32 / 32768.0).abs());
        }
        self.file.write_all(&bytes)?;
        self.samples += pcm.len() as u32;
        self.rewrite_header()?;
        self.file.flush()?;
        Ok(())
    }

    pub fn write_silence(&mut self, count: u32) -> io::Result<()> {
        let mut left = count as usize;
        let zeros = [0u8; 8192];
        while left > 0 {
            let samples = left.min(4096);
            self.file.write_all(&zeros[..samples * 2])?;
            for _ in 0..samples {
                self.note(0.0);
            }
            left -= samples;
        }
        self.samples += count;
        self.rewrite_header()?;
        self.file.flush()?;
        Ok(())
    }

    pub fn samples(&self) -> u32 {
        self.samples
    }

    pub fn duration(&self) -> f64 {
        self.samples as f64 / 16000.0
    }

    fn note(&mut self, amp: f32) {
        if amp > self.bucket {
            self.bucket = amp;
        }
        self.bucket_fill += 1;
        if self.bucket_fill >= 1600 {
            self.peaks.push(self.bucket);
            self.bucket = 0.0;
            self.bucket_fill = 0;
        }
    }

    pub fn snapshot(&self) -> Vec<f32> {
        let mut peaks = self.peaks.clone();
        if self.bucket_fill > 0 {
            peaks.push(self.bucket);
        }
        downsample(&peaks, 2000)
    }

    fn rewrite_header(&mut self) -> io::Result<()> {
        let data_bytes = self.samples.saturating_mul(2);
        self.file.seek(SeekFrom::Start(4))?;
        self.file.write_all(&(36u32.saturating_add(data_bytes)).to_le_bytes())?;
        self.file.seek(SeekFrom::Start(40))?;
        self.file.write_all(&data_bytes.to_le_bytes())?;
        self.file.seek(SeekFrom::End(0))?;
        Ok(())
    }
}

pub fn wav_header(samples: u32) -> [u8; 44] {
    let data_bytes = samples.saturating_mul(2);
    let mut header = [0u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36u32.saturating_add(data_bytes)).to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes());
    header[22..24].copy_from_slice(&1u16.to_le_bytes());
    header[24..28].copy_from_slice(&16000u32.to_le_bytes());
    header[28..32].copy_from_slice(&32000u32.to_le_bytes());
    header[32..34].copy_from_slice(&2u16.to_le_bytes());
    header[34..36].copy_from_slice(&16u16.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    header
}

pub fn downsample(peaks: &[f32], points: usize) -> Vec<f32> {
    if peaks.is_empty() || points == 0 {
        return Vec::new();
    }
    if peaks.len() <= points {
        return peaks.to_vec();
    }
    let mut out = vec![0f32; points];
    for (index, value) in peaks.iter().enumerate() {
        let bucket = index * points / peaks.len();
        if *value > out[bucket] {
            out[bucket] = *value;
        }
    }
    out
}

pub fn peaks_from_wav(path: &Path, points: usize) -> io::Result<(f64, Vec<f32>)> {
    let mut file = File::open(path)?;
    let (rate, data_pos, data_len) = find_data(&mut file)?;
    let frames = data_len / 2;
    let duration = if rate == 0 { 0.0 } else { frames as f64 / rate as f64 };
    let points = points.clamp(1, 4000);
    if frames == 0 {
        return Ok((0.0, vec![0.0; points.min(1)]));
    }
    file.seek(SeekFrom::Start(data_pos))?;
    let mut peaks = vec![0f32; points];
    let mut buf = [0u8; 8192];
    let mut remaining = data_len;
    let mut index = 0u64;
    while remaining > 0 {
        let want = buf.len().min(remaining as usize);
        file.read_exact(&mut buf[..want])?;
        remaining -= want as u64;
        for chunk in buf[..want].chunks_exact(2) {
            let sample = i16::from_le_bytes([chunk[0], chunk[1]]);
            let bucket = (index as usize) * points / frames as usize;
            let bucket = bucket.min(points - 1);
            let amp = (sample as f32 / 32768.0).abs();
            if amp > peaks[bucket] {
                peaks[bucket] = amp;
            }
            index += 1;
        }
    }
    Ok((duration, peaks))
}

pub fn open_wav_pcm(path: &Path, at: f64) -> io::Result<(File, u32, u64)> {
    let mut file = File::open(path)?;
    let (rate, data_pos, data_len) = find_data(&mut file)?;
    let frames = data_len / 2;
    let skip = ((at.max(0.0) * rate as f64) as u64).min(frames);
    file.seek(SeekFrom::Start(data_pos + skip * 2))?;
    Ok((file, rate, frames.saturating_sub(skip)))
}

fn find_data(file: &mut File) -> io::Result<(u32, u64, u64)> {
    let mut riff = [0u8; 12];
    file.read_exact(&mut riff)?;
    if &riff[0..4] != b"RIFF" || &riff[8..12] != b"WAVE" {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "not a wav file"));
    }
    let mut rate = 16000u32;
    loop {
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk)?;
        let id = [chunk[0], chunk[1], chunk[2], chunk[3]];
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]) as u64;
        let pos = file.stream_position()?;
        if &id == b"fmt " && size >= 16 {
            let mut fmt = vec![0u8; size as usize];
            file.read_exact(&mut fmt)?;
            rate = u32::from_le_bytes([fmt[4], fmt[5], fmt[6], fmt[7]]);
        } else if &id == b"data" {
            return Ok((rate, pos, size));
        } else {
            file.seek(SeekFrom::Current(size as i64))?;
        }
        if size % 2 == 1 {
            file.seek(SeekFrom::Current(1))?;
        }
    }
}

pub fn extract_wav(input: &Path, output: &Path) -> Result<(), String> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-hide_banner", "-loglevel", "error", "-i"])
        .arg(input)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(output);
    hide_window(&mut cmd);
    let out = cmd.output().map_err(|e| format!("ffmpeg is not available ({e})"))?;
    if !out.status.success() {
        let detail = String::from_utf8_lossy(&out.stderr);
        let detail = detail.trim();
        return Err(if detail.is_empty() { "ffmpeg could not read that file".into() } else { detail.into() });
    }
    Ok(())
}

pub fn hide_window(cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
}

pub fn parse_range(header: &str, len: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let rest = header.trim().strip_prefix("bytes=")?;
    let (start, end) = rest.split_once('-')?;
    if start.is_empty() {
        let tail: u64 = end.parse().ok()?;
        let start = len.saturating_sub(tail.max(1));
        Some((start, len - 1))
    } else {
        let start: u64 = start.parse().ok()?;
        let end = if end.is_empty() { len - 1 } else { end.parse::<u64>().ok()?.min(len - 1) };
        if start >= len || start > end {
            None
        } else {
            Some((start, end))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn range_parses_open_end() {
        assert_eq!(parse_range("bytes=0-", 100), Some((0, 99)));
        assert_eq!(parse_range("bytes=10-20", 100), Some((10, 20)));
        assert_eq!(parse_range("bytes=-10", 100), Some((90, 99)));
    }

    #[test]
    fn peaks_read_back_a_written_wav() {
        let path = std::env::temp_dir().join("speaker-studio-peak-test.wav");
        let mut file = File::create(&path).unwrap();
        file.write_all(&wav_header(4)).unwrap();
        for sample in [0i16, 16000, -32000, 1000] {
            file.write_all(&sample.to_le_bytes()).unwrap();
        }
        drop(file);
        let (duration, peaks) = peaks_from_wav(&path, 4).unwrap();
        assert!((duration - 4.0 / 16000.0).abs() < 0.0001);
        assert_eq!(peaks.len(), 4);
        assert!(peaks[2] > 0.9);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn silence_keeps_earlier_audio() {
        let path = std::env::temp_dir().join("speaker-studio-silence-test.wav");
        let mut wav = LiveWav::create(&path).unwrap();
        wav.push(&[1000, -1000]).unwrap();
        wav.write_silence(16000).unwrap();
        assert_eq!(wav.samples(), 16002);
        drop(wav);
        let (duration, peaks) = peaks_from_wav(&path, 8).unwrap();
        assert!((duration - 16002.0 / 16000.0).abs() < 0.001);
        assert!(peaks[0] > 0.0);
        let _ = std::fs::remove_file(path);
    }
}
