//! Image edits on 16-bit raw images as composable stages (tone, sharpen, clarity, dust healing,
//! crop, rotate) and TIFF/PNG/JPEG reading and writing. No UI and nothing scanner-specific, so
//! other programs can use it as it is.

pub mod clarity;
pub mod heal;
pub mod sharpen;
pub mod tone;

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("invalid image data: {0}")]
    Data(String),
    #[error("{0}")]
    Write(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Interleaved 16-bit samples; 8-bit data is widened to the full 16-bit range.
#[derive(Debug, Clone, PartialEq)]
pub struct RawImage {
    pub width: u32,
    pub height: u32,
    /// 1 (gray) or 3 (RGB).
    pub channels: u8,
    pub data: Vec<u16>,
}

impl RawImage {
    pub fn new(width: u32, height: u32, channels: u8, data: Vec<u16>) -> Result<Self> {
        if data.len() != width as usize * height as usize * channels as usize {
            return Err(Error::Data("sample count does not match dimensions".into()));
        }
        Ok(Self {
            width,
            height,
            channels,
            data,
        })
    }
}

pub trait Stage {
    fn apply(&self, img: RawImage) -> Result<RawImage>;
}

#[derive(Default)]
pub struct Pipeline(Vec<Box<dyn Stage>>);

impl Pipeline {
    pub fn with(mut self, stage: impl Stage + 'static) -> Self {
        self.0.push(Box::new(stage));
        self
    }

    pub fn run(&self, img: RawImage) -> Result<RawImage> {
        self.0.iter().try_fold(img, |img, s| s.apply(img))
    }
}

/// Inverts every sample (colour/B&W negative to positive; no orange-mask handling yet).
pub struct Invert;

impl Stage for Invert {
    fn apply(&self, mut img: RawImage) -> Result<RawImage> {
        img.data.iter_mut().for_each(|v| *v = u16::MAX - *v);
        Ok(img)
    }
}

/// Rotates clockwise by a multiple of 90 degrees.
pub struct Rotate(pub u16);

impl Stage for Rotate {
    fn apply(&self, img: RawImage) -> Result<RawImage> {
        let turns = match self.0 % 360 {
            0 => return Ok(img),
            90 => 1,
            180 => 2,
            270 => 3,
            d => return Err(Error::Unsupported(format!("rotation by {d} degrees"))),
        };
        let mut img = img;
        for _ in 0..turns {
            img = rotate_cw(&img);
        }
        Ok(img)
    }
}

fn rotate_cw(img: &RawImage) -> RawImage {
    let (w, h, c) = (
        img.width as usize,
        img.height as usize,
        img.channels as usize,
    );
    let mut out = vec![0u16; img.data.len()];
    for y in 0..h {
        for x in 0..w {
            // (x, y) moves to (h - 1 - y, x) in an image of width h.
            let dst = (x * h + (h - 1 - y)) * c;
            let src = (y * w + x) * c;
            out[dst..dst + c].copy_from_slice(&img.data[src..src + c]);
        }
    }
    RawImage {
        width: img.height,
        height: img.width,
        channels: img.channels,
        data: out,
    }
}

/// Crops to a pixel rectangle.
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Crop {
    /// The cropped region as a new image; the source stays untouched.
    pub fn of(&self, img: &RawImage) -> Result<RawImage> {
        if self.x + self.width > img.width || self.y + self.height > img.height {
            return Err(Error::Data("crop exceeds image bounds".into()));
        }
        let c = img.channels as usize;
        let mut data = Vec::with_capacity(self.width as usize * self.height as usize * c);
        for row in self.y..self.y + self.height {
            let start = (row as usize * img.width as usize + self.x as usize) * c;
            data.extend_from_slice(&img.data[start..start + self.width as usize * c]);
        }
        RawImage::new(self.width, self.height, img.channels, data)
    }
}

impl Stage for Crop {
    fn apply(&self, img: RawImage) -> Result<RawImage> {
        self.of(&img)
    }
}

/// File formats scans can be written in. TIFF and PNG keep 16 bits per sample; JPEG is 8 bit only.
/// Quality of JPEG files written.
pub const JPEG_QUALITY: u8 = 95;

/// File formats scans can be written in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OutputFormat {
    #[default]
    Tiff,
    Png,
    Jpeg,
}

impl OutputFormat {
    pub fn ext(self) -> &'static str {
        match self {
            Self::Tiff => "tiff",
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    /// Name shown in the UI (the file extension of JPEG is `jpg`).
    pub fn label(self) -> &'static str {
        match self {
            Self::Tiff => "TIFF",
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
        }
    }

    /// The deepest samples the format can hold.
    pub fn max_depth(self) -> BitDepth {
        match self {
            Self::Jpeg => BitDepth::Eight,
            _ => BitDepth::Sixteen,
        }
    }
}

/// Bits per sample of a saved file. Everything before the write runs on 16-bit data; `Eight`
/// only quantises the finished image.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BitDepth {
    #[default]
    Sixteen,
    Eight,
}

impl BitDepth {
    pub fn bits(self) -> u8 {
        match self {
            Self::Sixteen => 16,
            Self::Eight => 8,
        }
    }
}

/// A 16-bit sample as 8 bit, rounded (0 and 65535 map to 0 and 255).
pub fn to_8bit(v: u16) -> u8 {
    ((v as u32 * 255 + 32767) / 65535) as u8
}

/// Writes a 16-bit TIFF (gray or RGB).
pub fn write_tiff(img: &RawImage, path: &Path) -> Result<()> {
    write_image(img, path)
}

/// Reads a TIFF or PNG (or any format the `image` crate decodes) as 16-bit samples, without the
/// decoder's allocation limit (full-resolution scans are large). 8-bit data is widened to the
/// full 16-bit range (`v * 257`).
pub fn read_image(path: &Path) -> Result<RawImage> {
    let data = |e: &dyn std::fmt::Display| Error::Data(e.to_string());
    let mut reader = image::ImageReader::open(path)?
        .with_guessed_format()
        .map_err(|e| data(&e))?;
    reader.no_limits();
    from_dynamic(reader.decode().map_err(|e| data(&e))?)
}

fn from_dynamic(img: image::DynamicImage) -> Result<RawImage> {
    use image::DynamicImage::*;
    let (w, h) = (img.width(), img.height());
    let widen = |v: Vec<u8>| v.into_iter().map(|s| s as u16 * 257).collect();
    match img {
        ImageLuma16(b) => RawImage::new(w, h, 1, b.into_raw()),
        ImageRgb16(b) => RawImage::new(w, h, 3, b.into_raw()),
        ImageLuma8(b) => RawImage::new(w, h, 1, widen(b.into_raw())),
        ImageRgb8(b) => RawImage::new(w, h, 3, widen(b.into_raw())),
        other => RawImage::new(w, h, 3, other.into_rgb16().into_raw()),
    }
}

/// Bits per sample of the image file at `path`, from its header (nothing is decoded). `None` when
/// the file cannot be opened or is not an image.
pub fn file_depth(path: &Path) -> Option<BitDepth> {
    use image::ImageDecoder;
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    let decoder = reader.into_decoder().ok()?;
    let bits = decoder.original_color_type().bits_per_pixel()
        / decoder.original_color_type().channel_count() as u16;
    Some(if bits <= 8 {
        BitDepth::Eight
    } else {
        BitDepth::Sixteen
    })
}

/// Writes a 16-bit gray or RGB image in the format given by the file extension (tiff, png; jpg
/// is always 8 bit).
pub fn write_image(img: &RawImage, path: &Path) -> Result<()> {
    write_image_depth(img, path, BitDepth::Sixteen)
}

/// Like [`write_image`], with the sample depth chosen. A `.jpg`/`.jpeg` path is written 8 bit at
/// [`JPEG_QUALITY`] whatever `depth` says.
pub fn write_image_depth(img: &RawImage, path: &Path, depth: BitDepth) -> Result<()> {
    use image::{ImageBuffer, Luma, Rgb};
    let bad = || Error::Data("buffer size mismatch".into());
    let io = |e: image::ImageError| Error::Write(format!("writing {}: {e}", path.display()));
    let jpeg = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("jpg") || e.eq_ignore_ascii_case("jpeg"));
    if depth == BitDepth::Eight || jpeg {
        let data: Vec<u8> = img.data.iter().map(|&v| to_8bit(v)).collect();
        if jpeg {
            let color = match img.channels {
                1 => image::ExtendedColorType::L8,
                3 => image::ExtendedColorType::Rgb8,
                n => return Err(Error::Unsupported(format!("{n} channels"))),
            };
            let file = std::fs::File::create(path)?;
            let mut out = std::io::BufWriter::new(file);
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY)
                .encode(&data, img.width, img.height, color)
                .map_err(io)?;
            std::io::Write::flush(&mut out)?;
            return Ok(());
        }
        return match img.channels {
            1 => ImageBuffer::<Luma<u8>, _>::from_raw(img.width, img.height, data)
                .ok_or_else(bad)?
                .save(path)
                .map_err(io),
            3 => ImageBuffer::<Rgb<u8>, _>::from_raw(img.width, img.height, data)
                .ok_or_else(bad)?
                .save(path)
                .map_err(io),
            n => Err(Error::Unsupported(format!("{n} channels"))),
        };
    }
    match img.channels {
        1 => ImageBuffer::<Luma<u16>, _>::from_raw(img.width, img.height, img.data.clone())
            .ok_or_else(bad)?
            .save(path)
            .map_err(io),
        3 => ImageBuffer::<Rgb<u16>, _>::from_raw(img.width, img.height, img.data.clone())
            .ok_or_else(bad)?
            .save(path)
            .map_err(io),
        n => Err(Error::Unsupported(format!("{n} channels"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gray(w: u32, h: u32, data: Vec<u16>) -> RawImage {
        RawImage::new(w, h, 1, data).unwrap()
    }

    #[test]
    fn jpeg_is_always_8_bit_and_reads_back_close() {
        assert_eq!(OutputFormat::Jpeg.ext(), "jpg");
        assert_eq!(
            [
                OutputFormat::Tiff.label(),
                OutputFormat::Png.label(),
                OutputFormat::Jpeg.label()
            ],
            ["TIFF", "PNG", "JPEG"]
        );
        assert_eq!(OutputFormat::Jpeg.max_depth(), BitDepth::Eight);
        assert_eq!(OutputFormat::Png.max_depth(), BitDepth::Sixteen);
        let dir = std::env::temp_dir().join(format!("scan-jpeg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for channels in [1u8, 3] {
            let n = 32 * 32 * channels as usize;
            let data: Vec<u16> = (0..n).map(|i| (i % 32 * 2000) as u16).collect();
            let img = RawImage::new(32, 32, channels, data).unwrap();
            for ext in ["jpg", "jpeg"] {
                let p = dir.join(format!("a{channels}.{ext}"));
                // The depth asked for is ignored.
                write_image_depth(&img, &p, BitDepth::Sixteen).unwrap();
                assert_eq!(file_depth(&p), Some(BitDepth::Eight));
                let back = read_image(&p).unwrap();
                assert_eq!((back.width, back.height, back.channels), (32, 32, channels));
                let err: u64 = img
                    .data
                    .iter()
                    .zip(&back.data)
                    .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as u64)
                    .sum();
                assert!(err / (n as u64) < 2500, "mean error {}", err / n as u64);
            }
        }
    }

    #[test]
    fn eight_bit_output_is_rounded_half_the_size_and_reads_back_widened() {
        let dir = std::env::temp_dir().join(format!("scan-depth-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(
            (to_8bit(0), to_8bit(65535), to_8bit(257 * 100)),
            (0, 255, 100)
        );
        let data: Vec<u16> = (0..64 * 64u32).map(|i| (i * 16) as u16).collect();
        for (channels, ext) in [(1u8, "tiff"), (3, "png"), (1, "png"), (3, "tiff")] {
            let n = 64 * 64 * channels as usize;
            let img = RawImage::new(
                64,
                64,
                channels,
                data.iter().cycle().take(n).copied().collect(),
            )
            .unwrap();
            let (p16, p8) = (
                dir.join(format!("a16.{ext}")),
                dir.join(format!("a8.{ext}")),
            );
            write_image(&img, &p16).unwrap();
            write_image_depth(&img, &p8, BitDepth::Eight).unwrap();
            let back = read_image(&p8).unwrap();
            assert_eq!((back.width, back.channels), (64, channels));
            for (a, b) in img.data.iter().zip(&back.data) {
                assert!((*a as i32 - *b as i32).abs() <= 129, "{a} vs {b}");
            }
            assert_eq!(
                image::open(&p8).unwrap().color().bits_per_pixel() / channels as u16,
                8
            );
            assert_eq!(
                image::open(&p16).unwrap().color().bits_per_pixel() / channels as u16,
                16
            );
            if ext == "tiff" {
                let (s16, s8) = (
                    std::fs::metadata(&p16).unwrap().len(),
                    std::fs::metadata(&p8).unwrap().len(),
                );
                assert!(s8 < s16 * 6 / 10, "{s8} vs {s16}");
            }
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rotate_90_moves_pixels_clockwise() {
        // 1 2 3      4 1
        // 4 5 6  ->  5 2
        //            6 3
        let r = Rotate(90)
            .apply(gray(3, 2, vec![1, 2, 3, 4, 5, 6]))
            .unwrap();
        assert_eq!((r.width, r.height), (2, 3));
        assert_eq!(r.data, vec![4, 1, 5, 2, 6, 3]);
    }

    #[test]
    fn rotate_360_is_identity_and_odd_angles_fail() {
        let i = gray(2, 1, vec![1, 2]);
        assert_eq!(Rotate(360).apply(i.clone()).unwrap(), i);
        assert!(Rotate(45).apply(i).is_err());
    }

    #[test]
    fn crop_and_invert_compose() {
        let p = Pipeline::default()
            .with(Crop {
                x: 1,
                y: 0,
                width: 2,
                height: 1,
            })
            .with(Invert);
        let r = p.run(gray(3, 1, vec![0, 10, 20])).unwrap();
        assert_eq!(r.data, vec![u16::MAX - 10, u16::MAX - 20]);
    }

    #[test]
    fn tiff_roundtrip_keeps_16_bit() {
        let dir = std::env::temp_dir().join(format!("latent-image-edit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.tiff");
        let img = RawImage::new(2, 1, 3, vec![0, 1000, 65535, 256, 257, 258]).unwrap();
        write_tiff(&img, &path).unwrap();
        let back = image::open(&path).unwrap().into_rgb16();
        assert_eq!(back.into_raw(), img.data);
        std::fs::remove_dir_all(dir).ok();
    }
}

#[cfg(test)]
mod format_tests {
    use super::*;

    #[test]
    fn extensions_match_the_format() {
        assert_eq!(OutputFormat::default().ext(), "tiff");
        assert_eq!(OutputFormat::Png.ext(), "png");
    }

    #[test]
    fn png_keeps_16_bit_gray_and_rgb() {
        let dir = std::env::temp_dir().join(format!("scan-png-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let gray = RawImage::new(2, 1, 1, vec![257, 65535]).unwrap();
        let rgb = RawImage::new(1, 1, 3, vec![1, 40000, 65535]).unwrap();

        write_image(&gray, &dir.join("g.png")).unwrap();
        write_image(&rgb, &dir.join("c.png")).unwrap();
        assert_eq!(
            image::open(dir.join("g.png"))
                .unwrap()
                .into_luma16()
                .into_raw(),
            gray.data
        );
        assert_eq!(
            image::open(dir.join("c.png"))
                .unwrap()
                .into_rgb16()
                .into_raw(),
            rgb.data
        );
        let kind = image::open(dir.join("c.png")).unwrap().color();
        assert_eq!(kind, image::ColorType::Rgb16);
        std::fs::remove_dir_all(dir).ok();
    }
}

#[cfg(test)]
mod read_tests {
    use super::*;

    #[test]
    fn written_images_read_back_exactly_in_both_formats() {
        let dir = std::env::temp_dir().join(format!("scan-read-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let rgb = RawImage::new(2, 1, 3, vec![0, 257, 40000, 65535, 1, 2]).unwrap();
        let gray = RawImage::new(3, 1, 1, vec![0, 1234, 65535]).unwrap();
        for ext in ["tiff", "png"] {
            write_image(&rgb, &dir.join(format!("c.{ext}"))).unwrap();
            write_image(&gray, &dir.join(format!("g.{ext}"))).unwrap();
            assert_eq!(
                read_image(&dir.join(format!("c.{ext}"))).unwrap(),
                rgb,
                "{ext}"
            );
            assert_eq!(
                read_image(&dir.join(format!("g.{ext}"))).unwrap(),
                gray,
                "{ext}"
            );
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn eight_bit_files_are_widened_to_the_16_bit_range() {
        let img = image::DynamicImage::ImageLuma8(
            image::GrayImage::from_raw(2, 1, vec![0, 255]).unwrap(),
        );
        assert_eq!(from_dynamic(img).unwrap().data, vec![0, 65535]);
    }

    #[test]
    fn unreadable_files_are_errors() {
        assert!(read_image(Path::new("/definitely/not/here.png")).is_err());
    }
}

#[cfg(test)]
mod raw_image_tests {
    use super::*;

    #[test]
    fn raw_image_checks_length() {
        assert!(RawImage::new(2, 2, 3, vec![0; 12]).is_ok());
        assert!(RawImage::new(2, 2, 3, vec![0; 11]).is_err());
    }
}
