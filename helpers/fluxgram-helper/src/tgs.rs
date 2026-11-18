//! TGS stickers: a Lottie animation (JSON) in gzip, rendered by rlottie (native/rlottie, its C API)
//! straight at the frames' size — vectors, nothing to scale.

use std::ffi::{CString, c_char, c_void};
use std::io::Read;
use std::path::Path;
use std::sync::{Mutex, PoisonError};

use crate::fxa;
use crate::pixels;
use crate::render::{self, Rendered, Target};

unsafe extern "C" {
    fn lottie_animation_from_data(
        data: *const c_char,
        key: *const c_char,
        resource_path: *const c_char,
    ) -> *mut c_void;
    fn lottie_animation_destroy(animation: *mut c_void);
    fn lottie_animation_get_size(animation: *const c_void, width: *mut usize, height: *mut usize);
    fn lottie_animation_get_totalframe(animation: *const c_void) -> usize;
    fn lottie_animation_get_framerate(animation: *const c_void) -> f64;
    fn lottie_animation_render(
        animation: *mut c_void,
        frame: usize,
        buffer: *mut u32,
        width: usize,
        height: usize,
        bytes_per_line: usize,
    );
}

/// rlottie built without its threads keeps its rasterizer's scratch (the stroker, a dash's and a
/// path's buffers) in globals: one call into it at a time, whichever render it is for.
static RLOTTIE: Mutex<()> = Mutex::new(());

/// The most JSON a TGS may unpack to (Telegram's are up to 64 KB packed).
const MAX_JSON: u64 = 16 << 20;
/// The deepest nesting of the JSON passed to rlottie: its parser goes down a level of its own
/// stack for each (real animations nest some tens deep).
const MAX_DEPTH: usize = 500;

/// Renders the TGS `data` into the FXA file `output`.
pub fn render(data: &[u8], target: &Target, output: &Path) -> Result<Rendered, String> {
    let mut json = Vec::new();
    flate2::read::GzDecoder::new(data)
        .take(MAX_JSON + 1)
        .read_to_end(&mut json)
        .map_err(|error| format!("A broken TGS: {error}"))?;
    if json.len() as u64 > MAX_JSON {
        return Err("A broken TGS: over 16 MB of JSON".into());
    }
    if depth(&json) > MAX_DEPTH {
        return Err("A broken TGS: its JSON nests too deep".into());
    }
    let json = CString::new(json).map_err(|_| "A broken TGS: not JSON".to_string())?;
    let animation = Animation::load(&json).ok_or("A broken TGS: not a Lottie animation")?;

    let (width, height) = animation.size();
    // rlottie counts the out point (`op`) in; a Lottie's frames end before it (it is frame 0
    // again when the animation loops).
    let total = animation.total_frames().saturating_sub(1);
    let rate = animation.frame_rate();
    if width == 0 || height == 0 || total == 0 || !(rate.is_finite() && rate > 0.0) {
        return Err(format!(
            "A broken TGS: {width}×{height}, {total} frames at {rate} a second"
        ));
    }
    let step = render::step(rate, target.fps);
    let count = total.div_ceil(step);
    if count > render::MAX_FRAMES {
        return Err(format!("A TGS of {count} frames: too long"));
    }
    let (frame_width, frame_height) = pixels::fit(width, height, target.size);
    let micros = (1e6 * step as f64 / rate).round() as u32;
    let mut writer = fxa::Writer::create(output, frame_width, frame_height, micros, count)
        .map_err(render::write_error)?;
    // u32s: rlottie's pixels are ARGB words — BGRA bytes in memory.
    let mut pixels = vec![0u32; frame_width as usize * frame_height as usize];
    for index in 0..count {
        pixels.fill(0);
        animation.render(
            index * step,
            &mut pixels,
            frame_width as usize,
            frame_height as usize,
        );
        let bytes = as_bytes(&mut pixels);
        pixels::unpremultiply(bytes);
        writer.push(bytes).map_err(render::write_error)?;
    }
    writer.finish().map_err(render::write_error)?;
    Ok(Rendered {
        width: frame_width,
        height: frame_height,
        frames: count,
    })
}

/// A parsed animation of rlottie's.
struct Animation(*mut c_void);

impl Animation {
    fn load(json: &CString) -> Option<Animation> {
        let _rlottie = RLOTTIE.lock().unwrap_or_else(PoisonError::into_inner);
        // No cache key (rlottie is built without its cache), no folder for a picture's files.
        let animation =
            unsafe { lottie_animation_from_data(json.as_ptr(), c"".as_ptr(), c"".as_ptr()) };
        // Not `then_some`: an `Animation` of null would be made, and dropped under the lock.
        if animation.is_null() {
            None
        } else {
            Some(Animation(animation))
        }
    }

    fn size(&self) -> (usize, usize) {
        let (mut width, mut height) = (0, 0);
        unsafe { lottie_animation_get_size(self.0, &mut width, &mut height) };
        (width, height)
    }

    fn total_frames(&self) -> usize {
        unsafe { lottie_animation_get_totalframe(self.0) }
    }

    fn frame_rate(&self) -> f64 {
        unsafe { lottie_animation_get_framerate(self.0) }
    }

    /// Frame `frame` (from 0), premultiplied, scaled to `width` × `height` (the animation's
    /// proportions).
    fn render(&self, frame: usize, pixels: &mut [u32], width: usize, height: usize) {
        assert_eq!(pixels.len(), width * height);
        let _rlottie = RLOTTIE.lock().unwrap_or_else(PoisonError::into_inner);
        unsafe {
            lottie_animation_render(self.0, frame, pixels.as_mut_ptr(), width, height, width * 4)
        };
    }
}

impl Drop for Animation {
    fn drop(&mut self) {
        let _rlottie = RLOTTIE.lock().unwrap_or_else(PoisonError::into_inner);
        unsafe { lottie_animation_destroy(self.0) };
    }
}

fn as_bytes(words: &mut [u32]) -> &mut [u8] {
    // A u32 is four bytes in their memory order; any byte is a valid u8.
    unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), words.len() * 4) }
}

/// How deep the JSON's arrays and objects nest (brackets in strings aren't counted).
fn depth(json: &[u8]) -> usize {
    let (mut depth, mut deepest) = (0usize, 0usize);
    let (mut in_string, mut escaped) = (false, false);
    for &byte in json {
        if in_string {
            match byte {
                _ if escaped => escaped = false,
                b'\\' => escaped = true,
                b'"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match byte {
            b'"' => in_string = true,
            b'[' | b'{' => {
                depth += 1;
                deepest = deepest.max(depth);
            }
            b']' | b'}' => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    deepest
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nesting() {
        assert_eq!(depth(br#"{"a":[1,{"b":"[[[{"}]}"#), 3);
        assert_eq!(depth(br#"{"a":"\"[","b":[]}"#), 2);
        assert_eq!(depth(b"1"), 0);
    }
}
