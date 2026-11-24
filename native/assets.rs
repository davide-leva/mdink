use crate::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::read::GzDecoder;
use krilla::{image::Image, text::Font};
use std::{
    collections::HashMap,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

pub struct Fonts {
    pub fonts: Vec<Font>,
    pub data: Vec<Vec<u8>>,
}

impl Fonts {
    pub fn new() -> Result<Self> {
        let compressed: &[&[u8]] = &[
            include_bytes!("fonts/NotoSans-Regular.ttf.gz"),
            include_bytes!("fonts/NotoSans-Bold.ttf.gz"),
            include_bytes!("fonts/NotoSans-Italic.ttf.gz"),
            include_bytes!("fonts/NotoSans-BoldItalic.ttf.gz"),
            include_bytes!("fonts/NotoSansMono-Regular.ttf.gz"),
            include_bytes!("fonts/NotoSansSymbols-Regular.ttf.gz"),
            include_bytes!("fonts/NotoSansSymbols2-Regular.ttf.gz"),
        ];
        let mut data = Vec::new();
        let mut fonts = Vec::new();
        for bytes in compressed {
            let mut raw = Vec::new();
            GzDecoder::new(*bytes).read_to_end(&mut raw)?;
            fonts.push(Font::new(raw.clone().into(), 0).ok_or("Invalid embedded font")?);
            data.push(raw);
        }
        Ok(Self { fonts, data })
    }

    pub fn runs(&self, text: &str, preferred: usize) -> Result<Vec<(String, usize)>> {
        let faces: Vec<_> = self
            .data
            .iter()
            .map(|data| rustybuzz::Face::from_slice(data, 0).unwrap())
            .collect();
        let mut runs: Vec<(String, usize)> = Vec::new();
        for ch in text.chars() {
            let font = if ch.is_control() || faces[preferred].glyph_index(ch).is_some() {
                preferred
            } else {
                (0..faces.len())
                    .find(|&i| faces[i].glyph_index(ch).is_some())
                    .ok_or_else(|| {
                        format!(
                            "Glyph not available in the embedded fonts: {ch} (U+{:04X})",
                            ch as u32
                        )
                    })?
            };
            if let Some((text, _)) = runs.last_mut().filter(|(_, current)| *current == font) {
                text.push(ch);
            } else {
                runs.push((ch.to_string(), font));
            }
        }
        Ok(runs)
    }

    pub fn width(&self, text: &str, font: usize, size: f32) -> f32 {
        let face = rustybuzz::Face::from_slice(&self.data[font], 0).unwrap();
        let mut buffer = rustybuzz::UnicodeBuffer::new();
        buffer.push_str(text);
        let glyphs = rustybuzz::shape(&face, &[], buffer);
        glyphs
            .glyph_positions()
            .iter()
            .map(|p| p.x_advance as f32)
            .sum::<f32>()
            * size
            / face.units_per_em() as f32
    }
}

pub enum Asset {
    Raster(Image),
    Svg(Box<usvg::Tree>),
}

impl Asset {
    pub fn size(&self) -> (f32, f32) {
        match self {
            Self::Raster(image) => {
                let (w, h) = image.size();
                (w as f32, h as f32)
            }
            Self::Svg(tree) => (tree.size().width(), tree.size().height()),
        }
    }
}

pub struct Assets {
    base: PathBuf,
    cache: HashMap<String, usize>,
    pub items: Vec<Asset>,
    fontdb: Arc<usvg::fontdb::Database>,
    agent: ureq::Agent,
}

fn decode(text: &str) -> Result<String> {
    Ok(percent_encoding::percent_decode_str(text)
        .decode_utf8()?
        .into_owned())
}

impl Assets {
    pub fn new(base: &Path, fonts: &Fonts) -> Self {
        let mut fontdb = usvg::fontdb::Database::new();
        for font in &fonts.data {
            fontdb.load_font_data(font.clone());
        }
        fontdb.set_sans_serif_family("Noto Sans");
        fontdb.set_monospace_family("Noto Sans Mono");
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(30)))
            .build()
            .new_agent();
        Self {
            base: base.into(),
            cache: HashMap::new(),
            items: Vec::new(),
            fontdb: Arc::new(fontdb),
            agent,
        }
    }

    pub fn load(&mut self, src: &str) -> Result<usize> {
        if let Some(&id) = self.cache.get(src) {
            return Ok(id);
        }
        let image = self
            .read(src)
            .map_err(|e| format!("Unable to load image \"{src}\": {e}"))?;
        let id = self.items.len();
        self.items.push(image);
        self.cache.insert(src.into(), id);
        Ok(id)
    }

    fn read(&self, src: &str) -> Result<Asset> {
        let lower = src.to_ascii_lowercase();
        let bytes = if lower.starts_with("http://") || lower.starts_with("https://") {
            self.agent
                .get(src)
                .call()?
                .body_mut()
                .with_config()
                .limit(64 * 1024 * 1024)
                .read_to_vec()?
        } else if lower.starts_with("data:") {
            let (header, content) = src.split_once(',').ok_or("Invalid data URI")?;
            if header.ends_with(";base64") {
                STANDARD.decode(content)?
            } else {
                percent_encoding::percent_decode_str(content).collect()
            }
        } else {
            let path = if lower.starts_with("file://") {
                let path = &src[7..];
                let path = path
                    .strip_prefix("localhost/")
                    .map(|p| format!("/{p}"))
                    .unwrap_or_else(|| path.into());
                if !path.starts_with('/') {
                    return Err("file: URLs must refer to an absolute local path".into());
                }
                #[cfg(windows)]
                let path = path.strip_prefix('/').unwrap_or(&path).to_string();
                PathBuf::from(decode(&path)?)
            } else {
                if lower.contains("://") {
                    return Err("Unsupported image URL scheme".into());
                }
                self.base.join(decode(src)?)
            };
            std::fs::read(path)?
        };
        let data = bytes.clone().into();
        let raster = if bytes.starts_with(b"\x89PNG") {
            Some(Image::from_png(data, true))
        } else if bytes.starts_with(b"\xff\xd8") {
            Some(Image::from_jpeg(data, true))
        } else if bytes.starts_with(b"GIF8") {
            Some(Image::from_gif(data, true))
        } else if bytes.starts_with(b"RIFF") {
            Some(Image::from_webp(data, true))
        } else {
            None
        };
        if let Some(image) = raster {
            return Ok(Asset::Raster(
                image.map_err(|e| format!("Invalid image: {e}"))?,
            ));
        }
        if bytes.starts_with(b"BM") {
            let image =
                image::load_from_memory_with_format(&bytes, image::ImageFormat::Bmp)?.to_rgba8();
            let (w, h) = image.dimensions();
            return Ok(Asset::Raster(Image::from_rgba8(image.into_raw(), w, h)));
        }
        let opts = usvg::Options {
            fontdb: self.fontdb.clone(),
            font_family: "Noto Sans".into(),
            ..Default::default()
        };
        Ok(Asset::Svg(Box::new(
            usvg::Tree::from_data(&bytes, &opts).map_err(
                |_| "Unsupported image format or corrupt file (PNG, JPEG, GIF, WebP, BMP, SVG)",
            )?,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn embedded_fonts_cover_accents_currency_arrows_and_math() {
        let fonts = Fonts::new().unwrap();
        for font in 0..5 {
            let runs = fonts.runs("città perché € → − α Ж", font).unwrap();
            assert_eq!(
                runs.iter().map(|(s, _)| s.as_str()).collect::<String>(),
                "città perché € → − α Ж"
            );
            assert!(runs.iter().all(|(s, f)| fonts.width(s, *f, 10.5) > 0.0));
        }
    }
}
