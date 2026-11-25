use anyhow::{Result, anyhow, bail};

pub struct Rgb {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

pub fn decode_png(raw: &[u8]) -> Result<Rgb> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(raw));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info()?;
    let mut buf = vec![
        0;
        reader
            .output_buffer_size()
            .ok_or_else(|| anyhow!("the image is too large"))?
    ];
    let info = reader.next_frame(&mut buf)?;
    buf.truncate(info.buffer_size());
    let pixels = match info.color_type {
        png::ColorType::Rgb => buf,
        png::ColorType::Rgba => buf
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|c| [c[0], c[1], c[2]])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g]).collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|c| [c[0], c[0], c[0]])
            .collect(),
        png::ColorType::Indexed => bail!("unexpected image format"),
    };
    Ok(Rgb {
        width: info.width,
        height: info.height,
        pixels,
    })
}

impl Rgb {
    pub fn shrink(&self, by: u32) -> Rgb {
        if by <= 1 {
            return Rgb {
                width: self.width,
                height: self.height,
                pixels: self.pixels.clone(),
            };
        }
        let (w, h) = (self.width / by, self.height / by);
        let mut pixels = Vec::with_capacity((w * h * 3) as usize);
        let area = by * by;
        for y in 0..h {
            for x in 0..w {
                let mut sum = [0u32; 3];
                for dy in 0..by {
                    let row = ((y * by + dy) * self.width + x * by) as usize * 3;
                    for dx in 0..by as usize {
                        for (c, s) in sum.iter_mut().enumerate() {
                            *s += self.pixels[row + dx * 3 + c] as u32;
                        }
                    }
                }
                pixels.extend(sum.map(|s| (s / area) as u8));
            }
        }
        Rgb {
            width: w,
            height: h,
            pixels,
        }
    }

    pub fn jpeg(&self, quality: u8) -> Result<Vec<u8>> {
        let mut out = Vec::new();
        jpeg_encoder::Encoder::new(&mut out, quality).encode(
            &self.pixels,
            self.width as u16,
            self.height as u16,
            jpeg_encoder::ColorType::Rgb,
        )?;
        Ok(out)
    }
}
