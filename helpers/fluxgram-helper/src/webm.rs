//! WEBM stickers: VP9 in Matroska, the alpha channel a second VP9 stream in each block's
//! BlockAdditional (BlockAddID 1 — how video stickers carry transparency), decoded by libvpx
//! (native/libvpx, through native/vp9.c): the color decoder's picture, the alpha decoder's luma as
//! its alpha, scaled to the frames' size.
//!
//! The container is read here, as much of it as stickers use: the Segment's Info (TimecodeScale),
//! the first video track (VP9, its pixel size, its color matrix and range), the Clusters' blocks —
//! SimpleBlock, or BlockGroup with Block and BlockAdditions — in their order.

use std::ffi::{CStr, c_char, c_int, c_uint, c_void};
use std::ops::Range;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use crate::fxa;
use crate::pixels::{self, ColorSpace, Matrix, Picture, Planes};
use crate::render::{self, Rendered, Target};

// The elements read (their IDs with the length marker, as Matroska writes them).
const SEGMENT: u32 = 0x1853_8067;
const INFO: u32 = 0x1549_A966;
const TIMECODE_SCALE: u32 = 0x2A_D7B1;
const TRACKS: u32 = 0x1654_AE6B;
const TRACK_ENTRY: u32 = 0xAE;
const TRACK_NUMBER: u32 = 0xD7;
const TRACK_TYPE: u32 = 0x83;
const CODEC_ID: u32 = 0x86;
const CONTENT_ENCODINGS: u32 = 0x6D80;
const VIDEO: u32 = 0xE0;
const PIXEL_WIDTH: u32 = 0xB0;
const PIXEL_HEIGHT: u32 = 0xBA;
const COLOUR: u32 = 0x55B0;
const MATRIX_COEFFICIENTS: u32 = 0x55B1;
const RANGE: u32 = 0x55B9;
const CLUSTER: u32 = 0x1F43_B675;
const TIMECODE: u32 = 0xE7;
const SIMPLE_BLOCK: u32 = 0xA3;
const BLOCK_GROUP: u32 = 0xA0;
const BLOCK: u32 = 0xA1;
const BLOCK_ADDITIONS: u32 = 0x75A1;
const BLOCK_MORE: u32 = 0xA6;
const BLOCK_ADD_ID: u32 = 0xEE;
const BLOCK_ADDITIONAL: u32 = 0xA5;
/// The Segment's children: where a Cluster of unknown size ends.
const TOP_LEVEL: [u32; 8] = [
    CLUSTER,
    INFO,
    TRACKS,
    0x114D_9B74, // SeekHead
    0x1C53_BB6B, // Cues
    0x1254_C367, // Tags
    0x1043_A770, // Chapters
    0x1941_A469, // Attachments
];

/// The most pixels on a side of a sticker's video (Telegram's are 512).
const MAX_SIDE: usize = 4096;

/// What the container says.
#[derive(Debug, Default)]
pub struct Video {
    pub width: usize,
    pub height: usize,
    /// Matroska's MatrixCoefficients and Range, when given.
    pub matrix: Option<u64>,
    pub range: Option<u64>,
    /// The frames in their order.
    pub frames: Vec<Frame>,
}

/// A block of the video track: its time and its frame's bytes, with the alpha frame's.
#[derive(Debug)]
pub struct Frame {
    /// Nanoseconds from the start.
    pub time: i64,
    pub data: Range<usize>,
    pub alpha: Option<Range<usize>>,
}

/// Renders the WEBM `data` into the FXA file `output`.
pub fn render(data: &[u8], target: &Target, output: &Path) -> Result<Rendered, String> {
    let video = read(data).map_err(|error| format!("A broken WEBM: {error}"))?;
    let count_in = video.frames.len();
    // The frames' rate, from their times (a sticker's is even): the first to the last.
    let interval = match count_in {
        0 => return Err("A broken WEBM: no frames".into()),
        1 => 1e9 / 30.0,
        _ => {
            let span = video.frames[count_in - 1].time - video.frames[0].time;
            if span <= 0 {
                return Err("A broken WEBM: its frames' times".into());
            }
            span as f64 / (count_in - 1) as f64
        }
    };
    let step = render::step(1e9 / interval, target.fps);
    let count = count_in.div_ceil(step);
    if count > render::MAX_FRAMES {
        return Err(format!("A WEBM of {count} frames: too long"));
    }
    let micros = (interval * step as f64 / 1e3).round().max(1.0) as u32;

    let mut color = Decoder::new()?;
    let mut alpha = Decoder::new()?;
    let mut writer: Option<(fxa::Writer, usize, usize)> = None;
    let (mut source, mut scratch, mut scaled) =
        (Picture::default(), Picture::default(), Picture::default());
    let mut bgra = Vec::new();
    let mut size = (0, 0);
    for (index, frame) in video.frames.iter().enumerate() {
        color.decode(&data[frame.data.clone()])?;
        if let Some(range) = &frame.alpha {
            alpha.decode(&data[range.clone()])?;
        }
        if index % step != 0 {
            continue;
        }
        match color.picture()? {
            Some(picture) => {
                let planes = picture.planes();
                let mask = match &frame.alpha {
                    Some(_) => {
                        let mask = alpha
                            .picture()?
                            .ok_or("A broken WEBM: a frame's alpha shows nothing")?;
                        if (mask.width, mask.height) != (picture.width, picture.height) {
                            return Err(
                                "A broken WEBM: its alpha's size isn't its picture's".into()
                            );
                        }
                        Some(mask)
                    }
                    None => None,
                };
                let space = color_space(picture.color_space, video.matrix);
                let full = picture.full_range != 0 || video.range == Some(2);
                let matrix = Matrix::new(space, full);
                let mask = mask.as_ref().map(|mask| mask.planes().y);
                pixels::yuva_to_picture(&planes, mask, &matrix, &mut source);
                if writer.is_none() {
                    // The track's size, else the first picture's.
                    let (width, height) = match (video.width, video.height) {
                        (0, _) | (_, 0) => (source.width, source.height),
                        known => known,
                    };
                    let (frame_width, frame_height) = pixels::fit(width, height, target.size);
                    size = (frame_width, frame_height);
                    let file =
                        fxa::Writer::create(output, frame_width, frame_height, micros, count)
                            .map_err(render::write_error)?;
                    writer = Some((file, frame_width as usize, frame_height as usize));
                }
                let (_, width, height) = writer.as_ref().unwrap();
                if (source.width, source.height) == (*width, *height) {
                    pixels::to_straight(&source, &mut bgra);
                } else {
                    pixels::scale(&source, *width, *height, &mut scratch, &mut scaled);
                    pixels::to_straight(&scaled, &mut bgra);
                }
            }
            // A frame that shows nothing new: the last one again.
            None if !bgra.is_empty() => {}
            None => return Err("A broken WEBM: its first frame shows nothing".into()),
        }
        let (file, _, _) = writer.as_mut().unwrap();
        file.push(&bgra).map_err(render::write_error)?;
    }
    let (file, _, _) = writer.ok_or("A broken WEBM: no pictures")?;
    file.finish().map_err(render::write_error)?;
    Ok(Rendered {
        width: size.0,
        height: size.1,
        frames: count,
    })
}

/// The picture's color space: VP9's own, else the container's, else BT.601.
fn color_space(vp9: c_uint, container: Option<u64>) -> ColorSpace {
    match vp9 {
        1 | 3 => ColorSpace::Bt601,
        2 => ColorSpace::Bt709,
        4 => ColorSpace::Smpte240,
        5 => ColorSpace::Bt2020,
        7 => ColorSpace::Rgb,
        _ => match container {
            Some(1) => ColorSpace::Bt709,
            Some(7) => ColorSpace::Smpte240,
            Some(9) => ColorSpace::Bt2020,
            _ => ColorSpace::Bt601,
        },
    }
}

/// Reads the container: the first video track and its blocks.
pub fn read(data: &[u8]) -> Result<Video, String> {
    let mut reader = Reader { data, at: 0 };
    let (id, size) = reader.header()?;
    if id != 0x1A45_DFA3 {
        return Err("not EBML".into());
    }
    reader.skip(size)?;
    let mut segment = None;
    while reader.at < data.len() {
        let (id, size) = reader.header()?;
        if id == SEGMENT {
            segment = Some(reader.at..reader.end(size));
            break;
        }
        reader.skip(size)?;
    }
    let segment = segment.ok_or("no Segment")?;

    let mut scale: u64 = 1_000_000;
    let mut track: Option<u64> = None;
    let mut video = Video::default();
    let mut reader = Reader {
        data: &data[..segment.end],
        at: segment.start,
    };
    while reader.at < reader.data.len() {
        let (id, size) = reader.header()?;
        match id {
            INFO => {
                let end = reader.end(size);
                let mut info = reader.child(end);
                while info.at < end {
                    let (id, size) = info.header()?;
                    let body = info.body(size)?;
                    if id == TIMECODE_SCALE {
                        scale = uint(body)?.max(1);
                    }
                }
                reader.at = end;
            }
            TRACKS if track.is_none() => {
                let end = reader.end(size);
                track = tracks(reader.child(end), end, &mut video)?;
                reader.at = end;
            }
            CLUSTER => {
                let track = track.ok_or("a Cluster before its Tracks")?;
                cluster(&mut reader, size, track, scale, &mut video)?;
            }
            _ => reader.skip(size)?,
        }
    }
    if track.is_none() {
        return Err("no VP9 video track".into());
    }
    Ok(video)
}

/// The first video track: its number (None — not a video's Tracks), its size and colors.
fn tracks(mut reader: Reader, end: usize, video: &mut Video) -> Result<Option<u64>, String> {
    while reader.at < end {
        let (id, size) = reader.header()?;
        if id != TRACK_ENTRY {
            reader.skip(size)?;
            continue;
        }
        let entry_end = reader.end(size);
        let mut entry = reader.child(entry_end);
        let (mut number, mut kind, mut codec, mut encoded) = (None, None, String::new(), false);
        let mut found = Video::default();
        while entry.at < entry_end {
            let (id, size) = entry.header()?;
            match id {
                TRACK_NUMBER => number = Some(uint(entry.body(size)?)?),
                TRACK_TYPE => kind = Some(uint(entry.body(size)?)?),
                CODEC_ID => codec = String::from_utf8_lossy(entry.body(size)?).into_owned(),
                CONTENT_ENCODINGS => {
                    encoded = true;
                    entry.skip(size)?;
                }
                VIDEO => {
                    let video_end = entry.end(size);
                    let mut settings = entry.child(video_end);
                    while settings.at < video_end {
                        let (id, size) = settings.header()?;
                        match id {
                            PIXEL_WIDTH => found.width = uint(settings.body(size)?)? as usize,
                            PIXEL_HEIGHT => found.height = uint(settings.body(size)?)? as usize,
                            COLOUR => {
                                let colour_end = settings.end(size);
                                let mut colour = settings.child(colour_end);
                                while colour.at < colour_end {
                                    let (id, size) = colour.header()?;
                                    let body = colour.body(size)?;
                                    match id {
                                        MATRIX_COEFFICIENTS => found.matrix = Some(uint(body)?),
                                        RANGE => found.range = Some(uint(body)?),
                                        _ => {}
                                    }
                                }
                                settings.at = colour_end;
                            }
                            _ => settings.skip(size)?,
                        }
                    }
                    entry.at = video_end;
                }
                _ => entry.skip(size)?,
            }
        }
        reader.at = entry_end;
        if kind != Some(1) {
            continue;
        }
        if codec != "V_VP9" {
            return Err(format!("{codec} video, not VP9"));
        }
        if encoded {
            return Err("compressed or encrypted frames".into());
        }
        if found.width > MAX_SIDE || found.height > MAX_SIDE {
            return Err(format!("{}×{} pixels", found.width, found.height));
        }
        (video.width, video.height) = (found.width, found.height);
        (video.matrix, video.range) = (found.matrix, found.range);
        return number
            .map(Some)
            .ok_or_else(|| "a track without its number".into());
    }
    Ok(None)
}

/// A Cluster's blocks of the track; a Cluster of unknown size ends at the next of the Segment's
/// elements.
fn cluster(
    reader: &mut Reader,
    size: Option<usize>,
    track: u64,
    scale: u64,
    video: &mut Video,
) -> Result<(), String> {
    let unknown = size.is_none();
    let end = reader.end(size);
    let mut time: u64 = 0;
    while reader.at < end {
        let start = reader.at;
        let (id, size) = reader.header()?;
        if unknown && TOP_LEVEL.contains(&id) {
            // The next of the Segment's elements: this Cluster of unknown size is over.
            reader.at = start;
            return Ok(());
        }
        match id {
            TIMECODE => time = uint(reader.body(size)?)?,
            SIMPLE_BLOCK => {
                let body = reader.body(size)?;
                let body = reader.at - body.len()..reader.at;
                block(reader.data, body, None, track, time, scale, video)?;
            }
            BLOCK_GROUP => {
                let group_end = reader.end(size);
                let mut group = reader.child(group_end);
                let (mut body, mut alpha) = (None, None);
                while group.at < group_end {
                    let (id, size) = group.header()?;
                    match id {
                        BLOCK => {
                            body = Some(group.at..group.end(size));
                            group.skip(size)?;
                        }
                        BLOCK_ADDITIONS => {
                            let additions_end = group.end(size);
                            let mut additions = group.child(additions_end);
                            while additions.at < additions_end {
                                let (id, size) = additions.header()?;
                                if id != BLOCK_MORE {
                                    additions.skip(size)?;
                                    continue;
                                }
                                let more_end = additions.end(size);
                                let mut more = additions.child(more_end);
                                let (mut add_id, mut additional) = (1, None);
                                while more.at < more_end {
                                    let (id, size) = more.header()?;
                                    match id {
                                        BLOCK_ADD_ID => add_id = uint(more.body(size)?)?,
                                        BLOCK_ADDITIONAL => {
                                            additional = Some(more.at..more.end(size));
                                            more.skip(size)?;
                                        }
                                        _ => more.skip(size)?,
                                    }
                                }
                                if add_id == 1 && additional.is_some() {
                                    alpha = additional;
                                }
                                additions.at = more_end;
                            }
                            group.at = additions_end;
                        }
                        _ => group.skip(size)?,
                    }
                }
                reader.at = group_end;
                if let Some(body) = body {
                    block(reader.data, body, alpha, track, time, scale, video)?;
                }
            }
            _ => reader.skip(size)?,
        }
    }
    reader.at = end;
    Ok(())
}

/// A block of the track: its frame (no lacing — a video's blocks have one each).
fn block(
    data: &[u8],
    body: Range<usize>,
    alpha: Option<Range<usize>>,
    track: u64,
    cluster_time: u64,
    scale: u64,
    video: &mut Video,
) -> Result<(), String> {
    let mut reader = Reader {
        data: &data[..body.end],
        at: body.start,
    };
    let number = reader.number()?.ok_or("a block's track")?;
    if number != track {
        return Ok(());
    }
    let head = reader.body(Some(3))?;
    let relative = i16::from_be_bytes([head[0], head[1]]) as i64;
    if head[2] & 0x06 != 0 {
        return Err("laced video blocks".into());
    }
    if reader.at >= body.end {
        return Err("an empty block".into());
    }
    let time = (cluster_time as i64 + relative).saturating_mul(scale as i64);
    video.frames.push(Frame {
        time,
        data: reader.at..body.end,
        alpha: alpha.filter(|alpha| !alpha.is_empty()),
    });
    Ok(())
}

/// EBML: elements of an ID, a size and a body.
struct Reader<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    /// An element's ID (with its length marker) and its body's size (None — unknown).
    fn header(&mut self) -> Result<(u32, Option<usize>), String> {
        let first = *self.data.get(self.at).ok_or("cut")?;
        let length = first.leading_zeros() as usize + 1;
        if length > 4 {
            return Err("a broken element ID".into());
        }
        let bytes = self.data.get(self.at..self.at + length).ok_or("cut")?;
        let id = bytes.iter().fold(0u32, |id, &byte| id << 8 | byte as u32);
        self.at += length;
        let size = self.number()?;
        let size = match size {
            Some(size) => Some(usize::try_from(size).map_err(|_| "a huge element")?),
            None => None,
        };
        Ok((id, size))
    }

    /// A variable-length number (a size, a track number); all ones — unknown.
    fn number(&mut self) -> Result<Option<u64>, String> {
        let first = *self.data.get(self.at).ok_or("cut")?;
        let length = first.leading_zeros() as usize + 1;
        if length > 8 {
            return Err("a broken number".into());
        }
        let bytes = self.data.get(self.at..self.at + length).ok_or("cut")?;
        self.at += length;
        let mask = if length == 8 { 0 } else { 0xFFu8 >> length };
        let mut value = (first & mask) as u64;
        let mut all_ones = first & mask == mask;
        for &byte in &bytes[1..] {
            value = value << 8 | byte as u64;
            all_ones &= byte == 0xFF;
        }
        Ok((!all_ones).then_some(value))
    }

    /// Where an element of this size ends (unknown — at its parent's end).
    fn end(&self, size: Option<usize>) -> usize {
        match size {
            Some(size) => self.at.saturating_add(size).min(self.data.len()),
            None => self.data.len(),
        }
    }

    /// The element's body, whole.
    fn body(&mut self, size: Option<usize>) -> Result<&'a [u8], String> {
        let size = size.ok_or("a value of unknown size")?;
        let body = self
            .data
            .get(self.at..self.at.saturating_add(size))
            .ok_or("cut")?;
        self.at += size;
        Ok(body)
    }

    fn skip(&mut self, size: Option<usize>) -> Result<(), String> {
        let size = size.ok_or("an element of unknown size")?;
        if self.at.saturating_add(size) > self.data.len() {
            return Err("cut".into());
        }
        self.at += size;
        Ok(())
    }

    /// A reader of the children, up to `end`.
    fn child(&self, end: usize) -> Reader<'a> {
        Reader {
            data: &self.data[..end],
            at: self.at,
        }
    }
}

/// An unsigned integer element (big-endian, up to 8 bytes).
fn uint(body: &[u8]) -> Result<u64, String> {
    if body.len() > 8 {
        return Err("a broken number".into());
    }
    Ok(body
        .iter()
        .fold(0u64, |value, &byte| value << 8 | byte as u64))
}

/// libvpx's decoder, through native/vp9.c.
#[repr(C)]
struct RawPicture {
    planes: [*const u8; 3],
    strides: [c_int; 3],
    width: c_uint,
    height: c_uint,
    x_shift: c_uint,
    y_shift: c_uint,
    color_space: c_uint,
    full_range: c_uint,
}

unsafe extern "C" {
    fn fluxgram_vp9_new() -> *mut c_void;
    fn fluxgram_vp9_decode(decoder: *mut c_void, data: *const u8, size: usize) -> c_int;
    fn fluxgram_vp9_picture_get(decoder: *mut c_void, picture: *mut RawPicture) -> c_int;
    fn fluxgram_vp9_error(decoder: *mut c_void) -> *const c_char;
    fn fluxgram_vp9_free(decoder: *mut c_void);
}

/// libvpx built without its threads sets up its tables, once, unguarded, in a decoder's first
/// decode: the first decodes take turns.
static FIRST_DECODE: Mutex<()> = Mutex::new(());

struct Decoder {
    raw: *mut c_void,
    started: bool,
}

/// A decoded picture, valid until its decoder's next decode.
struct DecodedPicture<'a> {
    raw: RawPicture,
    width: usize,
    height: usize,
    color_space: c_uint,
    full_range: c_uint,
    _decoder: std::marker::PhantomData<&'a Decoder>,
}

impl DecodedPicture<'_> {
    fn planes(&self) -> Planes<'_> {
        let (width, height) = (self.width, self.height);
        let (x_shift, y_shift) = (self.raw.x_shift, self.raw.y_shift);
        let chroma = (
            (width + (1 << x_shift) - 1) >> x_shift,
            (height + (1 << y_shift) - 1) >> y_shift,
        );
        let plane = |index: usize, (width, height): (usize, usize)| {
            let stride = self.raw.strides[index] as usize;
            // libvpx's plane: `height` rows of `stride` bytes, the last row's `width` read.
            let length = stride * (height - 1) + width;
            (
                unsafe { std::slice::from_raw_parts(self.raw.planes[index], length) },
                stride,
            )
        };
        Planes {
            width,
            height,
            y: plane(0, (width, height)),
            u: plane(1, chroma),
            v: plane(2, chroma),
            x_shift,
            y_shift,
        }
    }
}

impl Decoder {
    fn new() -> Result<Decoder, String> {
        let raw = unsafe { fluxgram_vp9_new() };
        if raw.is_null() {
            return Err("VP9's decoder couldn't start".into());
        }
        Ok(Decoder {
            raw,
            started: false,
        })
    }

    fn decode(&mut self, frame: &[u8]) -> Result<(), String> {
        let _first =
            (!self.started).then(|| FIRST_DECODE.lock().unwrap_or_else(PoisonError::into_inner));
        self.started = true;
        if unsafe { fluxgram_vp9_decode(self.raw, frame.as_ptr(), frame.len()) } != 0 {
            let reason = unsafe { CStr::from_ptr(fluxgram_vp9_error(self.raw)) };
            return Err(format!("A broken WEBM: {}", reason.to_string_lossy()));
        }
        Ok(())
    }

    /// The picture the last decode showed, if it showed one.
    fn picture(&mut self) -> Result<Option<DecodedPicture<'_>>, String> {
        let mut raw = RawPicture {
            planes: [std::ptr::null(); 3],
            strides: [0; 3],
            width: 0,
            height: 0,
            x_shift: 0,
            y_shift: 0,
            color_space: 0,
            full_range: 0,
        };
        match unsafe { fluxgram_vp9_picture_get(self.raw, &mut raw) } {
            0 => Ok(None),
            1 => {
                let (width, height) = (raw.width as usize, raw.height as usize);
                let ok = (1..=MAX_SIDE).contains(&width)
                    && (1..=MAX_SIDE).contains(&height)
                    && raw.x_shift <= 1
                    && raw.y_shift <= 1
                    && raw.strides.iter().all(|&stride| stride > 0)
                    && raw.planes.iter().all(|plane| !plane.is_null());
                if !ok {
                    return Err(format!("A WEBM of {width}×{height} pixels"));
                }
                Ok(Some(DecodedPicture {
                    width,
                    height,
                    color_space: raw.color_space,
                    full_range: raw.full_range,
                    raw,
                    _decoder: std::marker::PhantomData,
                }))
            }
            _ => Err("A WEBM of more than 8 bits a color".into()),
        }
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { fluxgram_vp9_free(self.raw) };
    }
}
