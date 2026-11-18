//! FXA — Flux's file of an animation's frames, written here as Flux reads it (`flux-media`'s `fxa`
//! module in the Flux tree has the reader and the layout), little-endian: `FXA1`, the width and
//! height (u16 each), the frames' count and a frame's length in microseconds (u32 each), flags
//! (u32, 0), each frame's offset from the file's start then the end of the last (u32 each), then
//! the frames — each an LZ4 block (the block format, no size before it) of width × height × 4
//! bytes: BGRA, straight alpha, rows top to bottom.
//!
//! The frames are written as they come — the offsets' place first, filled in at the end — so one
//! frame is in memory at a time; the file is written to `<output>.tmp` and renamed, so it is whole
//! once it is there.

use std::fs::{self, File};
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const MAGIC: &[u8; 4] = b"FXA1";
/// The header before the offsets.
pub const HEADER: usize = 20;
/// Flux's reader takes no more frames than this.
pub const MAX_FRAMES: usize = 3600;

/// An animation file being written: [`Writer::push`] each frame, then [`Writer::finish`]. Dropped
/// unfinished, it leaves nothing behind.
pub struct Writer {
    /// None once finished.
    file: Option<BufWriter<File>>,
    temp: PathBuf,
    path: PathBuf,
    frame_size: usize,
    count: usize,
    /// The frames' offsets so far.
    offsets: Vec<u32>,
    /// Where the next frame goes.
    end: u64,
    /// A frame's LZ4 block, reused.
    block: Vec<u8>,
}

impl Writer {
    /// A file of `count` frames, `width` × `height` pixels each (up to 65 535), a frame every
    /// `frame_micros` microseconds.
    pub fn create(
        path: &Path,
        width: u32,
        height: u32,
        frame_micros: u32,
        count: usize,
    ) -> io::Result<Writer> {
        let (Ok(width16), Ok(height16)) = (u16::try_from(width), u16::try_from(height)) else {
            return Err(invalid(format!("frames of {width}×{height} pixels")));
        };
        if width == 0 || height == 0 || frame_micros == 0 || !(1..=MAX_FRAMES).contains(&count) {
            return Err(invalid(format!(
                "{count} frames of {width}×{height} pixels, {frame_micros} µs each"
            )));
        }
        let mut name = path.file_name().unwrap_or_default().to_os_string();
        name.push(".tmp");
        let temp = path.with_file_name(name);
        let mut file = BufWriter::with_capacity(1 << 16, File::create(&temp)?);
        let table = 4 * (count + 1);
        let mut header = Vec::with_capacity(HEADER + table);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&width16.to_le_bytes());
        header.extend_from_slice(&height16.to_le_bytes());
        header.extend_from_slice(&(count as u32).to_le_bytes());
        header.extend_from_slice(&frame_micros.to_le_bytes());
        header.extend_from_slice(&0u32.to_le_bytes());
        // The offsets' place: filled in by `finish`.
        header.resize(HEADER + table, 0);
        let written = file.write_all(&header);
        let writer = Writer {
            file: Some(file),
            temp,
            path: path.to_path_buf(),
            frame_size: width as usize * height as usize * 4,
            count,
            offsets: Vec::with_capacity(count + 1),
            end: header.len() as u64,
            block: Vec::new(),
        };
        // Dropped on a failure, the writer takes its file away.
        written.map(|()| writer)
    }

    /// The next frame: width × height × 4 bytes, BGRA, straight alpha.
    pub fn push(&mut self, bgra: &[u8]) -> io::Result<()> {
        if bgra.len() != self.frame_size {
            return Err(invalid(format!(
                "a frame of {} bytes, {} expected",
                bgra.len(),
                self.frame_size
            )));
        }
        if self.offsets.len() == self.count {
            return Err(invalid(format!("more than {} frames", self.count)));
        }
        let offset = u32::try_from(self.end).map_err(|_| invalid("over 4 GB".into()))?;
        self.block
            .resize(lz4_flex::block::get_maximum_output_size(bgra.len()), 0);
        let size =
            lz4_flex::block::compress_into(bgra, &mut self.block).map_err(io::Error::other)?;
        let file = self.file.as_mut().expect("an unfinished writer");
        file.write_all(&self.block[..size])?;
        self.offsets.push(offset);
        self.end += size as u64;
        Ok(())
    }

    /// Writes the offsets and renames the file into place: every frame must have come.
    pub fn finish(mut self) -> io::Result<()> {
        if self.offsets.len() != self.count {
            return Err(invalid(format!(
                "{} frames of {}",
                self.offsets.len(),
                self.count
            )));
        }
        let end = u32::try_from(self.end).map_err(|_| invalid("over 4 GB".into()))?;
        self.offsets.push(end);
        let table: Vec<u8> = self.offsets.iter().flat_map(|o| o.to_le_bytes()).collect();
        let mut file = self.file.take().expect("an unfinished writer");
        file.seek(SeekFrom::Start(HEADER as u64))?;
        file.write_all(&table)?;
        let file = file.into_inner().map_err(io::IntoInnerError::into_error)?;
        file.sync_all()?;
        // Closed before it is renamed (Windows renames no open file).
        drop(file);
        fs::rename(&self.temp, &self.path)
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            drop(file);
            let _ = fs::remove_file(&self.temp);
        }
    }
}

fn invalid(message: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("FXA: {message}"))
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// A file read back: width, height, a frame's length in microseconds, the frames.
    pub type Read = (u32, u32, u32, Vec<Vec<u8>>);

    /// Flux's reader (`flux-media`'s `fxa::Animation`), its checks as they are: the header, the
    /// offsets against the file, every frame's LZ4 block whole.
    pub fn read(path: &Path) -> Result<Read, String> {
        let bytes = fs::read(path).map_err(|error| error.to_string())?;
        if bytes.len() < HEADER || &bytes[0..4] != MAGIC {
            return Err("not an FXA file".into());
        }
        let u16_at = |at: usize| u16::from_le_bytes([bytes[at], bytes[at + 1]]) as u32;
        let u32_at = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        let (width, height, count, micros) = (u16_at(4), u16_at(6), u32_at(8), u32_at(12));
        if width == 0 || height == 0 || count == 0 || count > MAX_FRAMES as u32 || micros == 0 {
            return Err(format!("{width}×{height}, {count} frames of {micros} µs"));
        }
        if u32_at(16) != 0 {
            return Err("flags".into());
        }
        let table = HEADER + 4 * (count as usize + 1);
        if bytes.len() < table {
            return Err("the offsets are cut".into());
        }
        let offsets: Vec<u32> = (0..=count as usize)
            .map(|index| u32_at(HEADER + 4 * index))
            .collect();
        let ordered = offsets.windows(2).all(|pair| pair[0] < pair[1]);
        if offsets[0] as usize != table
            || !ordered
            || *offsets.last().unwrap() as usize > bytes.len()
        {
            return Err("the offsets are broken".into());
        }
        let size = width as usize * height as usize * 4;
        let mut frames = Vec::new();
        for pair in offsets.windows(2) {
            let mut pixels = vec![0u8; size];
            let block = &bytes[pair[0] as usize..pair[1] as usize];
            let written = lz4_flex::block::decompress_into(block, &mut pixels)
                .map_err(|error| error.to_string())?;
            if written != size {
                return Err("a frame is cut".into());
            }
            frames.push(pixels);
        }
        Ok((width, height, micros, frames))
    }

    #[test]
    fn writes_what_flux_reads() {
        let dir = std::env::temp_dir().join(format!("fluxgram-fxa-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.fxa");
        let frames: Vec<Vec<u8>> = (0..4u8)
            .map(|frame| (0..24).map(|byte| byte * frame).collect())
            .collect();
        let mut writer = Writer::create(&path, 3, 2, 33_333, 4).unwrap();
        for frame in &frames {
            writer.push(frame).unwrap();
        }
        assert!(writer.push(&frames[0]).is_err(), "a fifth frame of four");
        assert!(writer.push(&[0; 5]).is_err());
        writer.finish().unwrap();
        assert!(!dir.join("a.fxa.tmp").exists());

        let (width, height, micros, back) = read(&path).unwrap();
        assert_eq!((width, height, micros), (3, 2, 33_333));
        assert_eq!(back, frames);

        // Unfinished: nothing is left, the old file stays.
        let mut writer = Writer::create(&path, 3, 2, 33_333, 2).unwrap();
        writer.push(&frames[1]).unwrap();
        assert!(writer.finish().is_err());
        assert!(!dir.join("a.fxa.tmp").exists());
        assert_eq!(read(&path).unwrap().3, frames);

        assert!(Writer::create(&path, 70_000, 2, 33_333, 2).is_err());
        assert!(Writer::create(&path, 3, 2, 33_333, 0).is_err());
        fs::remove_dir_all(&dir).ok();
    }
}
