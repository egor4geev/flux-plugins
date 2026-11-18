//! Pixels for the frames of FXA files: a video's YUV and alpha planes to BGRA, scaling with
//! premultiplied alpha (no dark edges where a picture meets its transparency), and straight alpha
//! at the end, as FXA keeps it.

/// A picture in premultiplied BGRA, a float per channel (0–255).
#[derive(Default)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub data: Vec<f32>,
}

impl Picture {
    fn reset(&mut self, width: usize, height: usize) {
        self.width = width;
        self.height = height;
        self.data.clear();
        self.data.resize(width * height * 4, 0.0);
    }
}

/// A decoded video picture: 8-bit Y, U and V planes, U and V subsampled by the shifts.
pub struct Planes<'a> {
    pub width: usize,
    pub height: usize,
    pub y: (&'a [u8], usize),
    pub u: (&'a [u8], usize),
    pub v: (&'a [u8], usize),
    pub x_shift: u32,
    pub y_shift: u32,
}

/// YUV to RGB: `rgb = m × (Y − y0, U − c0, V − c0)`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    m: [[f32; 3]; 3],
    y0: f32,
    c0: f32,
}

/// The color spaces a video says it is in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ColorSpace {
    Bt601,
    Bt709,
    Bt2020,
    Smpte240,
    /// The planes are G, B, R (VP9's sRGB).
    Rgb,
}

impl Matrix {
    pub fn new(space: ColorSpace, full_range: bool) -> Matrix {
        let (kr, kb) = match space {
            ColorSpace::Bt601 => (0.299, 0.114),
            ColorSpace::Bt709 => (0.2126, 0.0722),
            ColorSpace::Bt2020 => (0.2627, 0.0593),
            ColorSpace::Smpte240 => (0.212, 0.087),
            ColorSpace::Rgb => {
                return Matrix {
                    m: [[0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                    y0: 0.0,
                    c0: 0.0,
                };
            }
        };
        let kg = 1.0 - kr - kb;
        // Studio range: Y 16–235, U and V 16–240.
        let (ys, cs, y0) = if full_range {
            (1.0, 1.0, 0.0)
        } else {
            (255.0 / 219.0, 255.0 / 224.0, 16.0)
        };
        Matrix {
            m: [
                [ys, 0.0, cs * 2.0 * (1.0 - kr)],
                [
                    ys,
                    -cs * 2.0 * (1.0 - kb) * kb / kg,
                    -cs * 2.0 * (1.0 - kr) * kr / kg,
                ],
                [ys, cs * 2.0 * (1.0 - kb), 0.0],
            ],
            y0,
            c0: 128.0,
        }
    }
}

/// Where a chroma sample is read for each luma row or column: the two neighbors and the second's
/// weight (chroma sited in the middle of the luma samples it covers).
fn chroma_taps(length: usize, shift: u32, chroma_length: usize) -> Vec<(usize, usize, f32)> {
    let last = chroma_length.saturating_sub(1);
    (0..length)
        .map(|at| {
            let position = ((at as f32 + 0.5) / (1u32 << shift) as f32 - 0.5).max(0.0);
            let first = (position as usize).min(last);
            let second = (first + 1).min(last);
            (first, second, position - first as f32)
        })
        .collect()
}

/// YUV planes and an alpha plane (none — opaque) to a premultiplied picture of the same size.
pub fn yuva_to_picture(
    planes: &Planes,
    alpha: Option<(&[u8], usize)>,
    matrix: &Matrix,
    out: &mut Picture,
) {
    let (width, height) = (planes.width, planes.height);
    out.reset(width, height);
    let chroma_width = (width + (1 << planes.x_shift) - 1) >> planes.x_shift;
    let chroma_height = (height + (1 << planes.y_shift) - 1) >> planes.y_shift;
    let columns = chroma_taps(width, planes.x_shift, chroma_width);
    let rows = chroma_taps(height, planes.y_shift, chroma_height);
    let mut u_row = vec![0f32; chroma_width];
    let mut v_row = vec![0f32; chroma_width];
    let Matrix { m, y0, c0 } = *matrix;
    for (y, &(r0, r1, fy)) in rows.iter().enumerate() {
        // The chroma rows around this one, blended.
        for (plane, row) in [(planes.u, &mut u_row), (planes.v, &mut v_row)] {
            let (data, stride) = plane;
            let (top, bottom) = (&data[r0 * stride..], &data[r1 * stride..]);
            for (x, value) in row.iter_mut().enumerate() {
                *value = top[x] as f32 + (bottom[x] as f32 - top[x] as f32) * fy - c0;
            }
        }
        let luma = &planes.y.0[y * planes.y.1..][..width];
        let alpha_row = alpha.map(|(data, stride)| &data[y * stride..][..width]);
        let out_row = &mut out.data[y * width * 4..][..width * 4];
        for (x, (pixel, &(c0x, c1x, fx))) in out_row
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(&columns)
            .enumerate()
        {
            let luma = luma[x] as f32 - y0;
            let u = u_row[c0x] + (u_row[c1x] - u_row[c0x]) * fx;
            let v = v_row[c0x] + (v_row[c1x] - v_row[c0x]) * fx;
            let r = (m[0][0] * luma + m[0][1] * u + m[0][2] * v).clamp(0.0, 255.0);
            let g = (m[1][0] * luma + m[1][1] * u + m[1][2] * v).clamp(0.0, 255.0);
            let b = (m[2][0] * luma + m[2][1] * u + m[2][2] * v).clamp(0.0, 255.0);
            let a = alpha_row.map_or(255.0, |row| row[x] as f32);
            let k = a / 255.0;
            pixel.copy_from_slice(&[b * k, g * k, r * k, a]);
        }
    }
}

/// The source pixels each output pixel takes, with their weights: their area in it when the
/// picture gets smaller (a box filter), the two nearest when it grows (bilinear).
fn taps(source: usize, target: usize) -> Vec<(usize, Vec<f32>)> {
    let ratio = source as f64 / target as f64;
    (0..target)
        .map(|at| {
            if ratio >= 1.0 {
                let (start, end) = (at as f64 * ratio, (at + 1) as f64 * ratio);
                let first = start.floor() as usize;
                let last = (end.ceil() as usize).min(source);
                let weights = (first..last)
                    .map(|pixel| {
                        let covered = end.min(pixel as f64 + 1.0) - start.max(pixel as f64);
                        (covered / ratio) as f32
                    })
                    .collect();
                (first, weights)
            } else {
                let center = ((at as f64 + 0.5) * ratio - 0.5).max(0.0);
                let first = (center.floor() as usize).min(source - 1);
                let t = (center - first as f64) as f32;
                if first + 1 < source && t > 0.0 {
                    (first, vec![1.0 - t, t])
                } else {
                    (first, vec![1.0])
                }
            }
        })
        .collect()
}

/// Scales a premultiplied picture to `width` × `height`: rows, then columns.
pub fn scale(
    source: &Picture,
    width: usize,
    height: usize,
    scratch: &mut Picture,
    out: &mut Picture,
) {
    let columns = taps(source.width, width);
    let rows = taps(source.height, height);
    scratch.reset(width, source.height);
    for y in 0..source.height {
        let from = &source.data[y * source.width * 4..][..source.width * 4];
        let to = &mut scratch.data[y * width * 4..][..width * 4];
        for (pixel, (first, weights)) in to.as_chunks_mut::<4>().0.iter_mut().zip(&columns) {
            let mut sum = [0f32; 4];
            for (offset, weight) in weights.iter().enumerate() {
                let at = (first + offset) * 4;
                for channel in 0..4 {
                    sum[channel] += from[at + channel] * weight;
                }
            }
            pixel.copy_from_slice(&sum);
        }
    }
    out.reset(width, height);
    for (y, (first, weights)) in rows.iter().enumerate() {
        let to = &mut out.data[y * width * 4..][..width * 4];
        for (offset, weight) in weights.iter().enumerate() {
            let from = &scratch.data[(first + offset) * width * 4..][..width * 4];
            for (value, source) in to.iter_mut().zip(from) {
                *value += source * weight;
            }
        }
    }
}

/// A premultiplied picture as FXA keeps it: BGRA bytes, straight alpha (a transparent pixel is all
/// zeros).
pub fn to_straight(picture: &Picture, bgra: &mut Vec<u8>) {
    bgra.clear();
    bgra.reserve(picture.data.len());
    for pixel in picture.data.as_chunks::<4>().0 {
        let alpha = pixel[3].clamp(0.0, 255.0);
        if alpha < 0.5 {
            bgra.extend_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        let k = 255.0 / alpha;
        for &value in &pixel[..3] {
            bgra.push((value * k + 0.5).clamp(0.0, 255.0) as u8);
        }
        bgra.push((alpha + 0.5) as u8);
    }
}

/// Premultiplied BGRA bytes (rlottie's) to straight alpha, in place.
pub fn unpremultiply(bgra: &mut [u8]) {
    for pixel in bgra.as_chunks_mut::<4>().0 {
        let alpha = pixel[3] as u32;
        match alpha {
            255 => {}
            0 => pixel.copy_from_slice(&[0, 0, 0, 0]),
            _ => {
                for value in &mut pixel[..3] {
                    *value = ((*value as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
    }
}

/// The size of the frames: `size` pixels on the longer side, the proportions kept.
pub fn fit(width: usize, height: usize, size: u32) -> (u32, u32) {
    let size = size as f64;
    let (width, height) = (width as f64, height as f64);
    if width >= height {
        (size as u32, ((size * height / width).round() as u32).max(1))
    } else {
        (((size * width / height).round() as u32).max(1), size as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn studio_range_bt601() {
        let matrix = Matrix::new(ColorSpace::Bt601, false);
        let (y, u, v) = ([235u8; 4], [128u8], [128u8]);
        let planes = Planes {
            width: 2,
            height: 2,
            y: (&y, 2),
            u: (&u, 1),
            v: (&v, 1),
            x_shift: 1,
            y_shift: 1,
        };
        let mut picture = Picture::default();
        yuva_to_picture(
            &planes,
            Some((&[255, 0, 128, 255], 2)),
            &matrix,
            &mut picture,
        );
        let mut bgra = Vec::new();
        to_straight(&picture, &mut bgra);
        assert_eq!(&bgra[0..4], &[255, 255, 255, 255], "white");
        assert_eq!(&bgra[4..8], &[0, 0, 0, 0], "transparent");
        assert_eq!(&bgra[8..12], &[255, 255, 255, 128], "half");
        // Pure red in BT.601 studio range: Y 81, U 90, V 240.
        let (y, u, v) = ([81u8], [90u8], [240u8]);
        let planes = Planes {
            width: 1,
            height: 1,
            y: (&y, 1),
            u: (&u, 1),
            v: (&v, 1),
            x_shift: 0,
            y_shift: 0,
        };
        yuva_to_picture(&planes, None, &matrix, &mut picture);
        to_straight(&picture, &mut bgra);
        let red = &bgra[..4];
        assert!(
            red[0] < 3 && red[1] < 3 && red[2] > 252 && red[3] == 255,
            "{red:?}"
        );
    }

    #[test]
    fn scaling_keeps_edges_clean() {
        // Red opaque on the left half, transparent black on the right: no dark fringe.
        let mut source = Picture::default();
        source.reset(8, 2);
        for y in 0..2 {
            for x in 0..4 {
                source.data[(y * 8 + x) * 4..][..4].copy_from_slice(&[0.0, 0.0, 255.0, 255.0]);
            }
        }
        let (mut scratch, mut out) = (Picture::default(), Picture::default());
        scale(&source, 3, 1, &mut scratch, &mut out);
        let mut bgra = Vec::new();
        to_straight(&out, &mut bgra);
        assert_eq!(&bgra[0..4], &[0, 0, 255, 255]);
        assert_eq!(
            &bgra[4..8],
            &[0, 0, 255, 128],
            "the edge: red, half transparent"
        );
        assert_eq!(&bgra[8..12], &[0, 0, 0, 0]);
        // Growing: bilinear between neighbors.
        scale(&source, 16, 4, &mut scratch, &mut out);
        assert_eq!((out.width, out.height), (16, 4));
        assert_eq!(out.data[3], 255.0);
        assert_eq!(out.data[15 * 4 + 3], 0.0);
    }

    #[test]
    fn sizes_and_alpha() {
        assert_eq!(fit(512, 512, 320), (320, 320));
        assert_eq!(fit(512, 256, 320), (320, 160));
        assert_eq!(fit(100, 512, 144), (28, 144));
        assert_eq!(fit(4000, 1, 100), (100, 1));
        let mut pixels = [0, 0, 128, 128, 10, 20, 30, 0, 1, 2, 3, 255];
        unpremultiply(&mut pixels);
        assert_eq!(pixels, [0, 0, 255, 128, 0, 0, 0, 0, 1, 2, 3, 255]);
    }
}
