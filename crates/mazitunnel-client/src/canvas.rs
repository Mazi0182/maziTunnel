use ab_glyph::{Font, FontArc, GlyphId, PxScale, ScaleFont};
use std::path::Path;

static GAMMA_LIGHT: std::sync::LazyLock<[f32; 256]> = std::sync::LazyLock::new(|| {
    let mut lut = [0.0f32; 256];
    for i in 0..256 {
        let cov = i as f32 / 255.0;
        lut[i] = cov.powf(0.68).min(1.0);
    }
    lut
});

static GAMMA_DARK: std::sync::LazyLock<[f32; 256]> = std::sync::LazyLock::new(|| {
    let mut lut = [0.0f32; 256];
    for i in 0..256 {
        let cov = i as f32 / 255.0;
        lut[i] = cov.powf(0.78).min(1.0);
    }
    lut
});

// Renk kodlamasi (Little-Endian BGRA: Windows GDI 32-bit uyumlu)
// Format: 0xAARRGGBB
#[inline(always)]
pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> u32 {
    ((a as u32) << 24) | ((r as u32) << 16) | ((g as u32) << 8) | (b as u32)
}

#[inline(always)]
pub fn rgb(r: u8, g: u8, b: u8) -> u32 {
    rgba(r, g, b, 255)
}

#[inline(always)]
pub fn color_r(c: u32) -> u8 {
    ((c >> 16) & 0xFF) as u8
}

#[inline(always)]
pub fn color_g(c: u32) -> u8 {
    ((c >> 8) & 0xFF) as u8
}

#[inline(always)]
pub fn color_b(c: u32) -> u8 {
    (c & 0xFF) as u8
}

#[inline(always)]
pub fn color_a(c: u32) -> u8 {
    ((c >> 24) & 0xFF) as u8
}

#[inline]
pub fn lerp_color(c1: u32, c2: u32, t: f32) -> u32 {
    let t = t.clamp(0.0, 1.0);
    let r1 = color_r(c1) as f32;
    let g1 = color_g(c1) as f32;
    let b1 = color_b(c1) as f32;
    let a1 = color_a(c1) as f32;

    let r2 = color_r(c2) as f32;
    let g2 = color_g(c2) as f32;
    let b2 = color_b(c2) as f32;
    let a2 = color_a(c2) as f32;

    let r = (r1 + (r2 - r1) * t).round() as u8;
    let g = (g1 + (g2 - g1) * t).round() as u8;
    let b = (b1 + (b2 - b1) * t).round() as u8;
    let a = (a1 + (a2 - a1) * t).round() as u8;

    rgba(r, g, b, a)
}

#[inline]
pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let inv = 1.0 - t;
    1.0 - inv * inv * inv
}

#[inline]
pub fn ease_out_back(t: f32, overshoot: f32) -> f32 {
    let t = t.clamp(0.0, 1.0) - 1.0;
    t * t * ((overshoot + 1.0) * t + overshoot) + 1.0
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FontWeight {
    Regular,
    Semibold,
    Bold,
}

pub struct FontManager {
    pub regular: FontArc,
    pub semibold: FontArc,
    pub bold: FontArc,
}

impl FontManager {
    pub fn new() -> Self {
        let regular = Self::load_font(
            "C:\\Windows\\Fonts\\segoeui.ttf",
            include_bytes!("../../../assets/font_regular.ttf"),
        );
        let semibold = Self::load_font_or_fallback(
            "C:\\Windows\\Fonts\\seguisb.ttf",
            &regular,
        );
        let bold = Self::load_font(
            "C:\\Windows\\Fonts\\segoeuib.ttf",
            include_bytes!("../../../assets/font_bold.ttf"),
        );
        Self { regular, semibold, bold }
    }

    fn load_font(system_path: &str, embedded_bytes: &'static [u8]) -> FontArc {
        if Path::new(system_path).exists() {
            if let Ok(data) = std::fs::read(system_path) {
                if let Ok(font) = FontArc::try_from_vec(data) {
                    return font;
                }
            }
        }
        FontArc::try_from_slice(embedded_bytes).expect("Gömülü font yüklenemedi")
    }

    fn load_font_or_fallback(system_path: &str, fallback: &FontArc) -> FontArc {
        if Path::new(system_path).exists() {
            if let Ok(data) = std::fs::read(system_path) {
                if let Ok(font) = FontArc::try_from_vec(data) {
                    return font;
                }
            }
        }
        fallback.clone()
    }
}

pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
    pub fonts: FontManager,
}

pub type SoftwareCanvas = Canvas;

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        let size = (width * height) as usize;
        Self {
            width,
            height,
            pixels: vec![0xFF000000; size],
            fonts: FontManager::new(),
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width != width || self.height != height {
            self.width = width;
            self.height = height;
            let size = (width * height) as usize;
            self.pixels.resize(size, 0xFF000000);
        }
    }

    #[inline(always)]
    pub fn clear(&mut self, color: u32) {
        self.pixels.fill(color);
    }

    #[inline(always)]
    pub fn blend_pixel(&mut self, x: i32, y: i32, src_color: u32, coverage: f32) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let alpha_f = (color_a(src_color) as f32 / 255.0) * coverage;
        if alpha_f <= 0.001 {
            return;
        }

        let idx = (y as u32 * self.width + x as u32) as usize;
        if alpha_f >= 0.999 {
            self.pixels[idx] = src_color | 0xFF000000;
            return;
        }

        let dst = self.pixels[idx];
        let dr = color_r(dst) as f32;
        let dg = color_g(dst) as f32;
        let db = color_b(dst) as f32;

        let sr = color_r(src_color) as f32;
        let sg = color_g(src_color) as f32;
        let sb = color_b(src_color) as f32;

        let inv = 1.0 - alpha_f;
        let out_r = (sr * alpha_f + dr * inv).round() as u8;
        let out_g = (sg * alpha_f + dg * inv).round() as u8;
        let out_b = (sb * alpha_f + db * inv).round() as u8;

        self.pixels[idx] = rgba(out_r, out_g, out_b, 255);
    }

    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: u32) {
        let x0 = (x.floor() as i32).max(0).min(self.width as i32);
        let y0 = (y.floor() as i32).max(0).min(self.height as i32);
        let x1 = ((x + w).ceil() as i32).max(0).min(self.width as i32);
        let y1 = ((y + h).ceil() as i32).max(0).min(self.height as i32);

        let alpha = color_a(color);
        if alpha == 255 {
            for py in y0..y1 {
                let row_start = (py as u32 * self.width + x0 as u32) as usize;
                let row_end = (py as u32 * self.width + x1 as u32) as usize;
                self.pixels[row_start..row_end].fill(color);
            }
        } else {
            for py in y0..y1 {
                for px in x0..x1 {
                    self.blend_pixel(px, py, color, 1.0);
                }
            }
        }
    }

    // SDF Anti-Aliased Yuvarlatilmis Dikdortgen Dolgusu
    pub fn fill_rounded_rect(&mut self, x: f32, y: f32, w: f32, h: f32, radius: f32, color: u32) {
        if w <= 0.0 || h <= 0.0 || color_a(color) == 0 {
            return;
        }
        let r = radius.min(w * 0.5).min(h * 0.5).max(0.0);
        if r < 0.5 {
            self.fill_rect(x, y, w, h, color);
            return;
        }

        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let half_w = w * 0.5;
        let half_h = h * 0.5;
        let bx = half_w - r;
        let by = half_h - r;

        let min_x = (x.floor() as i32).max(0).min(self.width as i32);
        let min_y = (y.floor() as i32).max(0).min(self.height as i32);
        let max_x = ((x + w).ceil() as i32).max(0).min(self.width as i32);
        let max_y = ((y + h).ceil() as i32).max(0).min(self.height as i32);

        let alpha = color_a(color);
        let packed = color | 0xFF000000;

        for py in min_y..max_y {
            let py_f = py as f32 + 0.5;
            let dy_corner = (py_f - cy).abs() - by;

            if dy_corner <= 0.0 {
                // Dikey duz alan: kosedeki egrilik bu satira etki etmez
                let inner_start = ((x + 1.0).ceil() as i32).max(min_x).min(max_x);
                let inner_end = ((x + w - 1.0).floor() as i32).max(min_x).min(max_x);

                for px in min_x..inner_start {
                    let px_f = px as f32 + 0.5;
                    let dist = (cx - px_f) - half_w;
                    let cov = (0.5 - dist).clamp(0.0, 1.0);
                    if cov > 0.0 {
                        self.blend_pixel(px, py, color, cov);
                    }
                }

                if inner_start < inner_end {
                    let row_offset = py as usize * self.width as usize;
                    if alpha == 255 {
                        self.pixels[row_offset + inner_start as usize..row_offset + inner_end as usize].fill(packed);
                    } else {
                        let row_slice = &mut self.pixels[row_offset + inner_start as usize..row_offset + inner_end as usize];
                        let alpha_f = alpha as f32 / 255.0;
                        let sr = color_r(color) as f32;
                        let sg = color_g(color) as f32;
                        let sb = color_b(color) as f32;
                        let inv = 1.0 - alpha_f;
                        for dst in row_slice.iter_mut() {
                            let dr = color_r(*dst) as f32;
                            let dg = color_g(*dst) as f32;
                            let db = color_b(*dst) as f32;
                            let out_r = (sr * alpha_f + dr * inv).round() as u8;
                            let out_g = (sg * alpha_f + dg * inv).round() as u8;
                            let out_b = (sb * alpha_f + db * inv).round() as u8;
                            *dst = rgba(out_r, out_g, out_b, 255);
                        }
                    }
                }

                for px in inner_end..max_x {
                    let px_f = px as f32 + 0.5;
                    let dist = (px_f - cx) - half_w;
                    let cov = (0.5 - dist).clamp(0.0, 1.0);
                    if cov > 0.0 {
                        self.blend_pixel(px, py, color, cov);
                    }
                }
            } else {
                // Ust veya alt yuvarlatilmis kose satiri: Tam Euclidean SDF ile puruzsuz tarama
                let qy = dy_corner;
                for px in min_x..max_x {
                    let px_f = px as f32 + 0.5;
                    let qx = (px_f - cx).abs() - bx;
                    let dist = (qx.max(0.0).hypot(qy.max(0.0))) + qx.min(0.0).max(qy.min(0.0)) - r;
                    let cov = (0.5 - dist).clamp(0.0, 1.0);
                    if cov >= 1.0 {
                        if alpha == 255 {
                            let idx = (py as u32 * self.width + px as u32) as usize;
                            self.pixels[idx] = packed;
                        } else {
                            self.blend_pixel(px, py, color, 1.0);
                        }
                    } else if cov > 0.0 {
                        self.blend_pixel(px, py, color, cov);
                    }
                }
            }
        }
    }

    // SDF Anti-Aliased Yuvarlatilmis Dikdortgen Kenarligi
    pub fn stroke_rounded_rect(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        radius: f32,
        stroke_width: f32,
        color: u32,
    ) {
        if w <= 0.0 || h <= 0.0 || stroke_width <= 0.0 || color_a(color) == 0 {
            return;
        }
        let half_stroke = stroke_width * 0.5;
        let r = radius.min(w * 0.5).min(h * 0.5).max(0.0);

        let cx = x + w * 0.5;
        let cy = y + h * 0.5;
        let half_w = w * 0.5;
        let half_h = h * 0.5;
        let bx = half_w - r;
        let by = half_h - r;

        let pad = half_stroke + 1.0;
        let min_x = ((x - pad).floor() as i32).max(0).min(self.width as i32);
        let min_y = ((y - pad).floor() as i32).max(0).min(self.height as i32);
        let max_x = ((x + w + pad).ceil() as i32).max(0).min(self.width as i32);
        let max_y = ((y + h + pad).ceil() as i32).max(0).min(self.height as i32);

        let hole_y_thresh = -half_stroke - 1.5;
        let hole_start_x = ((cx - half_w + half_stroke + 1.5).ceil() as i32).max(min_x).min(max_x);
        let hole_end_x = ((cx + half_w - half_stroke - 1.5).floor() as i32).max(min_x).min(max_x);

        for py in min_y..max_y {
            let py_f = py as f32 + 0.5;
            let dy_corner = (py_f - cy).abs() - by;
            let qy = dy_corner;

            // in_hollow_band yalnizca kose yayinin bittigi duz dikey alanda aktif olmali
            let in_hollow_band = dy_corner < hole_y_thresh && hole_start_x < hole_end_x;

            if in_hollow_band {
                for px in min_x..hole_start_x {
                    let px_f = px as f32 + 0.5;
                    let qx = (px_f - cx).abs() - bx;
                    let outer_dist = (qx.max(0.0).hypot(qy.max(0.0))) + qx.min(0.0).max(qy.min(0.0)) - r;
                    let dist = outer_dist.abs() - half_stroke;
                    let cov = (0.5 - dist).clamp(0.0, 1.0);
                    if cov > 0.0 {
                        self.blend_pixel(px, py, color, cov);
                    }
                }

                // Ortadaki bos alan atlanir

                for px in hole_end_x..max_x {
                    let px_f = px as f32 + 0.5;
                    let qx = (px_f - cx).abs() - bx;
                    let outer_dist = (qx.max(0.0).hypot(qy.max(0.0))) + qx.min(0.0).max(qy.min(0.0)) - r;
                    let dist = outer_dist.abs() - half_stroke;
                    let cov = (0.5 - dist).clamp(0.0, 1.0);
                    if cov > 0.0 {
                        self.blend_pixel(px, py, color, cov);
                    }
                }
            } else {
                for px in min_x..max_x {
                    let px_f = px as f32 + 0.5;
                    let qx = (px_f - cx).abs() - bx;
                    let outer_dist = (qx.max(0.0).hypot(qy.max(0.0))) + qx.min(0.0).max(qy.min(0.0)) - r;
                    let dist = outer_dist.abs() - half_stroke;
                    let cov = (0.5 - dist).clamp(0.0, 1.0);
                    if cov > 0.0 {
                        self.blend_pixel(px, py, color, cov);
                    }
                }
            }
        }
    }

    // SDF Anti-Aliased Daire Dolgusu
    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, color: u32) {
        if radius <= 0.0 {
            return;
        }
        let pad = radius + 1.0;
        let min_x = ((cx - pad).floor() as i32).max(0).min(self.width as i32);
        let min_y = ((cy - pad).floor() as i32).max(0).min(self.height as i32);
        let max_x = ((cx + pad).ceil() as i32).max(0).min(self.width as i32);
        let max_y = ((cy + pad).ceil() as i32).max(0).min(self.height as i32);

        for py in min_y..max_y {
            let py_f = py as f32 + 0.5;
            let dy = py_f - cy;
            for px in min_x..max_x {
                let px_f = px as f32 + 0.5;
                let dx = px_f - cx;
                let dist = (dx * dx + dy * dy).sqrt() - radius;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_pixel(px, py, color, cov);
                }
            }
        }
    }

    // SDF Anti-Aliased Halka / Cember Kenarligi
    pub fn stroke_circle(&mut self, cx: f32, cy: f32, radius: f32, stroke_width: f32, color: u32) {
        if radius <= 0.0 || stroke_width <= 0.0 {
            return;
        }
        let half_stroke = stroke_width * 0.5;
        let pad = radius + half_stroke + 1.0;
        let min_x = ((cx - pad).floor() as i32).max(0).min(self.width as i32);
        let min_y = ((cy - pad).floor() as i32).max(0).min(self.height as i32);
        let max_x = ((cx + pad).ceil() as i32).max(0).min(self.width as i32);
        let max_y = ((cy + pad).ceil() as i32).max(0).min(self.height as i32);

        for py in min_y..max_y {
            let py_f = py as f32 + 0.5;
            let dy = py_f - cy;
            for px in min_x..max_x {
                let px_f = px as f32 + 0.5;
                let dx = px_f - cx;
                let r_dist = (dx * dx + dy * dy).sqrt() - radius;
                let dist = r_dist.abs() - half_stroke;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_pixel(px, py, color, cov);
                }
            }
        }
    }

    // SDF Anti-Aliased Yay (Arc) Cizimi (Aci radyan cinsinden)
    pub fn stroke_arc(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        start_angle_rad: f32,
        sweep_angle_rad: f32,
        stroke_width: f32,
        color: u32,
    ) {
        if radius <= 0.0 || stroke_width <= 0.0 || sweep_angle_rad.abs() < 0.01 {
            return;
        }
        let half_stroke = stroke_width * 0.5;
        let pad = radius + half_stroke + 1.0;
        let min_x = ((cx - pad).floor() as i32).max(0).min(self.width as i32);
        let min_y = ((cy - pad).floor() as i32).max(0).min(self.height as i32);
        let max_x = ((cx + pad).ceil() as i32).max(0).min(self.width as i32);
        let max_y = ((cy + pad).ceil() as i32).max(0).min(self.height as i32);

        let pi2 = std::f32::consts::PI * 2.0;
        let mut norm_start = start_angle_rad % pi2;
        if norm_start < 0.0 {
            norm_start += pi2;
        }

        for py in min_y..max_y {
            let py_f = py as f32 + 0.5;
            let dy = py_f - cy;
            for px in min_x..max_x {
                let px_f = px as f32 + 0.5;
                let dx = px_f - cx;
                let r = (dx * dx + dy * dy).sqrt();
                let r_dist = r - radius;
                let dist = r_dist.abs() - half_stroke;
                let cov_r = (0.5 - dist).clamp(0.0, 1.0);

                if cov_r > 0.0 {
                    let mut angle = dy.atan2(dx);
                    if angle < 0.0 {
                        angle += pi2;
                    }
                    let mut diff = angle - norm_start;
                    while diff < 0.0 {
                        diff += pi2;
                    }
                    while diff >= pi2 {
                        diff -= pi2;
                    }

                    if diff <= sweep_angle_rad {
                        self.blend_pixel(px, py, color, cov_r);
                    }
                }
            }
        }
    }

    // SDF Anti-Aliased Yuvarlak Uclu Cizgi Segmenti (Capsule)
    pub fn draw_line(&mut self, x0: f32, y0: f32, x1: f32, y1: f32, stroke_width: f32, color: u32) {
        if stroke_width <= 0.0 {
            return;
        }
        let half_w = stroke_width * 0.5;
        let pad = half_w + 1.0;

        let min_x = ((x0.min(x1) - pad).floor() as i32).max(0).min(self.width as i32);
        let min_y = ((y0.min(y1) - pad).floor() as i32).max(0).min(self.height as i32);
        let max_x = ((x0.max(x1) + pad).ceil() as i32).max(0).min(self.width as i32);
        let max_y = ((y0.max(y1) + pad).ceil() as i32).max(0).min(self.height as i32);

        let dx = x1 - x0;
        let dy = y1 - y0;
        let len_sq = dx * dx + dy * dy;

        for py in min_y..max_y {
            let py_f = py as f32 + 0.5;
            for px in min_x..max_x {
                let px_f = px as f32 + 0.5;

                let t = if len_sq < 0.0001 {
                    0.0
                } else {
                    let dot = (px_f - x0) * dx + (py_f - y0) * dy;
                    (dot / len_sq).clamp(0.0, 1.0)
                };

                let qx = x0 + t * dx;
                let qy = y0 + t * dy;
                let dist = ((px_f - qx).powi(2) + (py_f - qy).powi(2)).sqrt() - half_w;
                let cov = (0.5 - dist).clamp(0.0, 1.0);
                if cov > 0.0 {
                    self.blend_pixel(px, py, color, cov);
                }
            }
        }
    }

    // --- RGBA BLITTING VE GECIS / ANİMASYON FONKSIYONLARI ---

    // Raw RGBA ikon blit
    pub fn draw_rgba_icon(&mut self, x: i32, y: i32, w: usize, h: usize, rgba_data: &[u8], opacity: f32) {
        let op = opacity.clamp(0.0, 1.0);
        if op <= 0.001 {
            return;
        }
        for py in 0..h {
            let dy = y + py as i32;
            if dy < 0 || dy >= self.height as i32 {
                continue;
            }
            for px in 0..w {
                let dx = x + px as i32;
                if dx < 0 || dx >= self.width as i32 {
                    continue;
                }
                let idx = (py * w + px) * 4;
                if idx + 3 < rgba_data.len() {
                    let r = rgba_data[idx];
                    let g = rgba_data[idx + 1];
                    let b = rgba_data[idx + 2];
                    let a = rgba_data[idx + 3];
                    if a > 0 {
                        self.blend_pixel(dx, dy, rgba(r, g, b, a), op);
                    }
                }
            }
        }
    }

    // Tam pencere tema crossfade'i icin baska bir piksel tamponunu alpha ile karistir
    pub fn blend_from_buffer(&mut self, other: &[u32], alpha: f32) {
        let a = alpha.clamp(0.0, 1.0);
        if a <= 0.001 {
            return;
        }
        let a_u32 = (a * 256.0).round() as u32;
        let inv_u32 = 256 - a_u32;
        let count = self.pixels.len().min(other.len());
        for i in 0..count {
            if a_u32 >= 256 {
                self.pixels[i] = other[i];
            } else {
                let src = other[i];
                let dst = self.pixels[i];

                let r = ((color_r(src) as u32 * a_u32 + color_r(dst) as u32 * inv_u32) >> 8) as u8;
                let g = ((color_g(src) as u32 * a_u32 + color_g(dst) as u32 * inv_u32) >> 8) as u8;
                let b = ((color_b(src) as u32 * a_u32 + color_b(dst) as u32 * inv_u32) >> 8) as u8;

                self.pixels[i] = rgba(r, g, b, 255);
            }
        }
    }

    // Subpixel-interpolated 42px slide + smooth crossfade
    pub fn composite_page_slide(&mut self, from_buf: &[u32], to_buf: &[u32], progress: f32, dir: f32) {
        if to_buf.is_empty() || from_buf.is_empty() {
            if !to_buf.is_empty() {
                self.pixels.copy_from_slice(to_buf);
            }
            return;
        }

        let p = ease_out_cubic(progress.clamp(0.0, 1.0));
        let w = self.width as i32;
        let h = self.height as i32;

        let from_shift = dir * 42.0 * p;
        let to_shift = -dir * 42.0 * (1.0 - p);

        let mut x_lut: Vec<(usize, usize, f32, usize, usize, f32)> = Vec::with_capacity(w as usize);
        for x in 0..w {
            let fx_from = (x as f32 + from_shift).clamp(0.0, (w - 1) as f32);
            let x0_from = fx_from.floor() as usize;
            let x1_from = (x0_from + 1).min((w - 1) as usize);
            let frac_from = fx_from - x0_from as f32;

            let fx_to = (x as f32 + to_shift).clamp(0.0, (w - 1) as f32);
            let x0_to = fx_to.floor() as usize;
            let x1_to = (x0_to + 1).min((w - 1) as usize);
            let frac_to = fx_to - x0_to as f32;

            x_lut.push((x0_from, x1_from, frac_from, x0_to, x1_to, frac_to));
        }

        for y in 0..h {
            let row = y as usize * self.width as usize;
            for x in 0..w as usize {
                let idx = row + x;
                if idx >= self.pixels.len() {
                    continue;
                }
                let (x0_f, x1_f, frac_f, x0_t, x1_t, frac_t) = x_lut[x];

                let from_c0 = from_buf[row + x0_f];
                let from_col = if frac_f <= 0.001 {
                    from_c0
                } else {
                    lerp_color(from_c0, from_buf[row + x1_f], frac_f)
                };

                let to_c0 = to_buf[row + x0_t];
                let to_col = if frac_t <= 0.001 {
                    to_c0
                } else {
                    lerp_color(to_c0, to_buf[row + x1_t], frac_t)
                };

                self.pixels[idx] = lerp_color(from_col, to_col, p);
            }
        }
    }

    // TunnelSwitch basparmak guc simgesi
    pub fn draw_power_icon(&mut self, cx: f32, cy: f32, r: f32, color: u32, stroke: f32) {
        let pi = std::f32::consts::PI;
        let cy_opt = cy + 0.5; // optik dikey merkezleme
        let start_rad = 308.0 * (pi / 180.0);
        let sweep_rad = 284.0 * (pi / 180.0);
        self.stroke_arc(cx, cy_opt, r, start_rad, sweep_rad, stroke, color);
        self.draw_line(cx, cy_opt - r - 0.5, cx, cy_opt - 1.0, stroke, color);
    }

    // ThemeToggleButton ay / gunes morfoloji ikonu
    pub fn draw_sun_moon_morph(&mut self, cx: f32, cy: f32, morph_t: f32, fg_color: u32, bg_color: u32) {
        let t = morph_t.clamp(0.0, 1.0);
        // Taban diski
        self.fill_circle(cx, cy, 6.8, fg_color);

        // Hilal maskesi (bg_color ile oyulur)
        let cutout_r = 5.8 * (1.0 - t);
        if cutout_r > 0.3 {
            let mask_x = cx + 3.2 + t * 6.0;
            let mask_y = cy - 2.6 - t * 6.0;
            self.fill_circle(mask_x, mask_y, cutout_r, bg_color);
        }

        // Acik modda (t > 0.15) gunes isinlari
        if t > 0.15 {
            let pi = std::f32::consts::PI;
            let rot_rad = t * 90.0 * (pi / 180.0);
            let r_in = 6.2;
            let r_out = 6.2 + 3.0 * t;
            let ray_alpha = ((t - 0.15) / 0.85).clamp(0.0, 1.0);
            let ray_col = rgba(
                color_r(fg_color),
                color_g(fg_color),
                color_b(fg_color),
                (color_a(fg_color) as f32 * ray_alpha).round() as u8,
            );
            for i in 0..8 {
                let ang = rot_rad + (i as f32) * (pi / 4.0);
                let x0 = cx + ang.cos() * r_in;
                let y0 = cy + ang.sin() * r_in;
                let x1 = cx + ang.cos() * r_out;
                let y1 = cy + ang.sin() * r_out;
                self.draw_line(x0, y0, x1, y1, 1.8, ray_col);
            }
        }
    }

    // Asagi/Yukari acilan sevron (v -> ^)
    pub fn draw_chevron(&mut self, cx: f32, cy: f32, size: f32, open_t: f32, color: u32, stroke: f32) {
        let t = open_t.clamp(0.0, 1.0);
        let dy = size * 0.55 * (1.0 - 2.0 * t);
        self.draw_line(cx - size, cy - dy, cx, cy + dy, stroke, color);
        self.draw_line(cx, cy + dy, cx + size, cy - dy, stroke, color);
    }

    // Otomatik satir saran metin cizimi
    pub fn draw_text_wrapped(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        max_w: f32,
        line_h: f32,
        size: f32,
        color: u32,
        bold: bool,
    ) -> f32 {
        if text.is_empty() {
            return y;
        }

        let mut lines = Vec::new();
        for paragraph in text.split('\n') {
            if paragraph.is_empty() {
                lines.push(String::new());
                continue;
            }

            let words: Vec<&str> = paragraph.split_whitespace().collect();
            if words.is_empty() {
                lines.push(String::new());
                continue;
            }

            let mut current_line = String::new();
            for word in words {
                let candidate = if current_line.is_empty() {
                    word.to_string()
                } else {
                    format!("{} {}", current_line, word)
                };

                let (w, _) = self.measure_text(bold, size, &candidate);
                if w <= max_w || current_line.is_empty() {
                    current_line = candidate;
                } else {
                    lines.push(current_line);
                    current_line = word.to_string();
                }
            }
            if !current_line.is_empty() {
                lines.push(current_line);
            }
        }

        for (idx, line) in lines.iter().enumerate() {
            let ly = y + (idx as f32) * line_h;
            self.draw_text(bold, size, x, ly, line, color);
        }

        y + (lines.len() as f32) * line_h
    }

    // --- VEKTOR IKON CIZIMLERI ---

    // Pencere Ikonu
    pub fn draw_icon_window(&mut self, cx: f32, cy: f32, color: u32) {
        self.stroke_rounded_rect(cx - 6.5, cy - 5.5, 13.0, 11.0, 2.5, 1.6, color);
        self.draw_line(cx - 6.0, cy - 2.0, cx + 6.0, cy - 2.0, 1.6, color);
    }

    // Guc / Power Ikonu
    pub fn draw_icon_power(&mut self, cx: f32, cy: f32, color: u32) {
        let pi = std::f32::consts::PI;
        self.stroke_arc(cx, cy + 0.5, 6.0, pi * 0.72, pi * 1.56, 1.65, color);
        self.draw_line(cx, cy - 6.5, cx, cy - 1.0, 1.65, color);
    }

    // Kopyala / Copy Ikonu
    pub fn draw_icon_copy(&mut self, cx: f32, cy: f32, color: u32) {
        self.stroke_rounded_rect(cx - 5.5, cy - 6.0, 8.5, 8.5, 1.8, 1.4, color);
        self.stroke_rounded_rect(cx - 2.5, cy - 3.0, 8.5, 8.5, 1.8, 1.4, color);
    }

    // Cikis / Carpi Ikonu
    pub fn draw_icon_cross(&mut self, cx: f32, cy: f32, size: f32, stroke_width: f32, color: u32) {
        let half = size * 0.5;
        self.draw_line(cx - half, cy - half, cx + half, cy + half, stroke_width, color);
        self.draw_line(cx + half, cy - half, cx - half, cy + half, stroke_width, color);
    }

    pub fn draw_icon_close(&mut self, cx: f32, cy: f32, size: f32, stroke_width: f32, color: u32) {
        self.draw_icon_cross(cx, cy, size, stroke_width, color);
    }

    // Onay Tiki Ikonu
    pub fn draw_icon_check(&mut self, cx: f32, cy: f32, size: f32, stroke_width: f32, color: u32) {
        self.draw_line(cx - size * 0.45, cy, cx - size * 0.1, cy + size * 0.35, stroke_width, color);
        self.draw_line(cx - size * 0.1, cy + size * 0.35, cx + size * 0.45, cy - size * 0.35, stroke_width, color);
    }

    // Mimari doku: 16px nokta matrisi ve ust ayirici cizgi
    pub fn draw_architectural_backdrop(&mut self, is_dark: bool) {
        let w = self.width as f32;
        let h = self.height as f32;
        let center_x = w * 0.5;
        let center_y = 160.0f32;
        let max_r = (center_x * center_x + (h - center_y) * (h - center_y)).sqrt().max(1.0);

        let max_alpha = if is_dark { 18.0f32 } else { 14.0f32 };
        let dot_rgb = if is_dark { (255u8, 255u8, 255u8) } else { (0u8, 0u8, 0u8) };

        let step = 16usize;
        let w_u = self.width as usize;
        let h_u = self.height as usize;

        for y in (8..h_u).step_by(step) {
            let y_f = y as f32;
            let dy = y_f - center_y;
            let row = y * w_u;

            for x in (8..w_u).step_by(step) {
                let x_f = x as f32;
                let dx = x_f - center_x;
                let dist = (dx * dx + dy * dy).sqrt();
                let falloff = (1.0 - (dist / max_r)).clamp(0.0, 1.0);
                let alpha = (max_alpha * falloff * falloff).round() as u8;
                if alpha > 0 {
                    let idx = row + x;
                    if idx < self.pixels.len() {
                        let bg = self.pixels[idx];
                        let a_u = alpha as u32;
                        let inv_a = 255 - a_u;
                        let r = ((color_r(bg) as u32 * inv_a + dot_rgb.0 as u32 * a_u) / 255) as u8;
                        let g = ((color_g(bg) as u32 * inv_a + dot_rgb.1 as u32 * a_u) / 255) as u8;
                        let b = ((color_b(bg) as u32 * inv_a + dot_rgb.2 as u32 * a_u) / 255) as u8;
                        self.pixels[idx] = rgba(r, g, b, 255);
                    }
                }
            }
        }

        // Ust baslik ayirici teknik cizgi (y = 51.0)
        let line_alpha = if is_dark { 14 } else { 10 };
        let line_col = rgba(dot_rgb.0, dot_rgb.1, dot_rgb.2, line_alpha);
        self.draw_line(20.0, 51.0, w - 20.0, 51.0, 1.0, line_col);
    }

    // Goz (Goster/Gizle) Animasyonlu Ikonu
    pub fn draw_icon_eye_animated(&mut self, cx: f32, cy: f32, open_t: f32, hover_t: f32, color: u32, bg_color: u32) {
        let t = open_t.clamp(0.0, 1.0);

        // 1) Hover vurgusu
        if hover_t > 0.01 {
            let r = color_r(color);
            let g = color_g(color);
            let b = color_b(color);
            let a = (hover_t * 24.0).clamp(0.0, 255.0) as u8;
            self.fill_rounded_rect(cx - 14.0, cy - 13.0, 28.0, 26.0, 6.0, rgba(r, g, b, a));
        }

        // 2) Gercek Badem Goz (Almond Eye) dis hatlari (vh morfu: 2.5 + 2.2 * t)
        let hw = 8.5f32;
        let vh = 2.5 + 2.2 * t;
        for i in 0..20 {
            let u0 = -1.0 + (i as f32 / 10.0);
            let u1 = -1.0 + ((i + 1) as f32 / 10.0);
            let x0 = cx + u0 * hw;
            let x1 = cx + u1 * hw;
            let y0_top = cy - vh * (1.0 - u0 * u0);
            let y1_top = cy - vh * (1.0 - u1 * u1);
            let y0_bot = cy + vh * (1.0 - u0 * u0);
            let y1_bot = cy + vh * (1.0 - u1 * u1);
            self.draw_line(x0, y0_top, x1, y1_top, 1.45, color);
            self.draw_line(x0, y0_bot, x1, y1_bot, 1.45, color);
        }

        // 3) Goz Bebegi (Iris): pupil_r = 1.3 + 1.2 * t
        let pupil_r = 1.3 + 1.2 * t;
        self.stroke_circle(cx, cy, pupil_r, 1.35, color);
        if t > 0.15 {
            let fill_r = (pupil_r - 0.7).max(0.4);
            self.fill_circle(cx, cy, fill_r, color);
        }

        // 4) Gizli Durum Capraz Cizgisi (Slash) ve Animasyonu (t < 0.99 iken)
        if t < 0.99 {
            let slash_t = ease_out_cubic((1.0 - t).clamp(0.0, 1.0));
            if slash_t > 0.02 {
                let sx0 = cx - 6.8;
                let sy0 = cy - 5.8;
                let sx1 = sx0 + 13.6 * slash_t;
                let sy1 = sy0 + 11.6 * slash_t;
                self.draw_line(sx0, sy0, sx1, sy1, 3.8, bg_color);
                self.draw_line(sx0, sy0, sx1, sy1, 1.5, color);
            }
        }
    }

    // Goz (Goster/Gizle) Ikonu (geriye donuk uyumluluk)
    pub fn draw_icon_eye(&mut self, cx: f32, cy: f32, is_open: bool, color: u32) {
        self.draw_icon_eye_animated(cx, cy, if is_open { 1.0 } else { 0.0 }, 0.0, color, 0xFF18181C);
    }

    // Eski ikon uyumlulugu
    pub fn draw_icon_theme(&mut self, cx: f32, cy: f32, morph_t: f32, color: u32) {
        self.draw_sun_moon_morph(cx, cy, morph_t, color, 0xFF141417);
    }

    pub fn draw_icon_arrow_down(&mut self, cx: f32, cy: f32, size: f32, color: u32) {
        self.draw_chevron(cx, cy, size, 0.0, color, 1.5);
    }

    // Altigen Tunel Dugum Ikonu (Brand Logo)
    pub fn draw_icon_hexagon_node(&mut self, cx: f32, cy: f32, radius: f32, color: u32) {
        let pi = std::f32::consts::PI;
        for i in 0..6 {
            let a0 = (i as f32) * (pi / 3.0) - pi / 6.0;
            let a1 = ((i + 1) as f32) * (pi / 3.0) - pi / 6.0;
            let x0 = cx + a0.cos() * radius;
            let y0 = cy + a0.sin() * radius;
            let x1 = cx + a1.cos() * radius;
            let y1 = cy + a1.sin() * radius;
            self.draw_line(x0, y0, x1, y1, 1.6, color);
        }
        self.fill_circle(cx, cy, radius * 0.35, color);
    }

    // Vektor Sunucu Rafi (ServerNodeIcon)
    pub fn draw_server_rack_node(&mut self, cx: f32, cy: f32, connected_t: f32, is_dark: bool) {
        let x = cx - 18.0;
        let y = cy - 18.0;

        let bg = if is_dark {
            lerp_color(0xFF232329, 0xFF27272F, connected_t)
        } else {
            lerp_color(0xFFE4E4E7, 0xFFE2E2E7, connected_t)
        };
        let border = if is_dark {
            lerp_color(0xFF32323A, 0xFF454552, connected_t)
        } else {
            lerp_color(0xFFD4D4D8, 0xFFC4C4C8, connected_t)
        };

        self.fill_rounded_rect(x, y, 36.0, 36.0, 9.0, bg);
        self.stroke_rounded_rect(x, y, 36.0, 36.0, 9.0, 1.0, border);

        let blade_stroke = if is_dark {
            lerp_color(0xFFD4D4DC, 0xFFFFFFFF, connected_t)
        } else {
            lerp_color(0xFF52525B, 0xFF141417, connected_t)
        };
        let blade_fill = if is_dark {
            lerp_color(0xFF18181C, 0xFF1F1F26, connected_t)
        } else {
            lerp_color(0xFFF4F4F6, 0xFFFFFFFF, connected_t)
        };
        let led_col = if is_dark {
            lerp_color(0xFF71717A, 0xFFF4F4F6, connected_t)
        } else {
            lerp_color(0xFFA1A1AA, 0xFF141417, connected_t)
        };
        let vent_col = if is_dark {
            lerp_color(0xFF52525B, 0xFFA1A1AA, connected_t)
        } else {
            lerp_color(0xFFA1A1AA, 0xFF71717A, connected_t)
        };

        for row_y in [y + 9.5, y + 19.0] {
            self.fill_rounded_rect(x + 7.5, row_y, 21.0, 7.5, 2.4, blade_fill);
            self.stroke_rounded_rect(x + 7.5, row_y, 21.0, 7.5, 2.4, 1.25, blade_stroke);

            // LED noktasi
            self.fill_circle(x + 11.3, row_y + 3.75, 1.45, led_col);

            // Havalandirma izgarasi
            self.draw_line(x + 16.5, row_y + 3.75, x + 24.5, row_y + 3.75, 1.3, vent_col);
        }
    }

    // --- METIN YAZMA VE OLCME MOTORU (ab_glyph) ---

    #[inline]
    pub fn get_font(&self, weight: FontWeight) -> &FontArc {
        match weight {
            FontWeight::Regular => &self.fonts.regular,
            FontWeight::Semibold => &self.fonts.semibold,
            FontWeight::Bold => &self.fonts.bold,
        }
    }

    pub fn measure_text_weight(&self, weight: FontWeight, font_size: f32, text: &str) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, font_size);
        }
        let font = self.get_font(weight);
        let eff_size = font_size.round().max(10.0);
        let scale = PxScale::from(eff_size);
        let scaled = font.as_scaled(scale);

        let mut width = 0.0f32;
        let mut prev: Option<GlyphId> = None;

        for ch in text.chars() {
            let glyph_id = font.glyph_id(ch);
            if let Some(prev_id) = prev {
                width += scaled.kern(prev_id, glyph_id);
            }
            width += scaled.h_advance(glyph_id);
            prev = Some(glyph_id);
        }

        let height = scaled.ascent() - scaled.descent();
        (width, height)
    }

    pub fn measure_text(&self, is_bold: bool, font_size: f32, text: &str) -> (f32, f32) {
        let weight = if is_bold { FontWeight::Bold } else { FontWeight::Regular };
        self.measure_text_weight(weight, font_size, text)
    }

    pub fn measure_text_semibold(&self, font_size: f32, text: &str) -> (f32, f32) {
        self.measure_text_weight(FontWeight::Semibold, font_size, text)
    }

    pub fn draw_text_weight(
        &mut self,
        weight: FontWeight,
        font_size: f32,
        x: f32,
        y: f32,
        text: &str,
        color: u32,
    ) {
        if text.is_empty() || color_a(color) == 0 {
            return;
        }
        let font = self.get_font(weight).clone();
        let eff_size = font_size.round().max(10.0);
        let scale = PxScale::from(eff_size);
        let scaled = font.as_scaled(scale);

        let mut x_pos = x.round();
        let baseline_y = (y + scaled.ascent()).round();
        let mut prev: Option<GlyphId> = None;

        let alpha_base = color_a(color) as f32 / 255.0;
        let sr = color_r(color) as f32;
        let sg = color_g(color) as f32;
        let sb = color_b(color) as f32;
        let luma = sr * 0.299 + sg * 0.587 + sb * 0.114;
        let gamma_lut = if luma > 128.0 { &*GAMMA_LIGHT } else { &*GAMMA_DARK };

        for ch in text.chars() {
            let glyph_id = font.glyph_id(ch);
            if let Some(prev_id) = prev {
                x_pos += scaled.kern(prev_id, glyph_id);
            }

            let glyph = glyph_id.with_scale_and_position(scale, ab_glyph::point(x_pos, baseline_y));
            if let Some(outlined) = font.outline_glyph(glyph) {
                let bounds = outlined.px_bounds();
                let width = self.width;
                let height = self.height;
                let pixels = &mut self.pixels;

                outlined.draw(|gx, gy, cov| {
                    if cov <= 0.005 {
                        return;
                    }
                    let px = bounds.min.x as i32 + gx as i32;
                    let py = bounds.min.y as i32 + gy as i32;

                    if px >= 0 && py >= 0 && px < width as i32 && py < height as i32 {
                        let idx = (py as u32 * width + px as u32) as usize;
                        let cov_idx = (cov * 255.0 + 0.5).min(255.0) as usize;
                        let cov_adj = gamma_lut[cov_idx];
                        let alpha_f = alpha_base * cov_adj;
                        if alpha_f >= 0.999 {
                            pixels[idx] = color | 0xFF000000;
                        } else {
                            let dst = pixels[idx];
                            let dr = color_r(dst) as f32;
                            let dg = color_g(dst) as f32;
                            let db = color_b(dst) as f32;
                            let inv = 1.0 - alpha_f;

                            let out_r = (sr * alpha_f + dr * inv).round() as u8;
                            let out_g = (sg * alpha_f + dg * inv).round() as u8;
                            let out_b = (sb * alpha_f + db * inv).round() as u8;
                            pixels[idx] = rgba(out_r, out_g, out_b, 255);
                        }
                    }
                });
            }

            x_pos += scaled.h_advance(glyph_id);
            prev = Some(glyph_id);
        }
    }

    pub fn draw_text(&mut self, is_bold: bool, font_size: f32, x: f32, y: f32, text: &str, color: u32) {
        let weight = if is_bold { FontWeight::Bold } else { FontWeight::Regular };
        self.draw_text_weight(weight, font_size, x, y, text, color);
    }

    pub fn draw_text_semibold(&mut self, font_size: f32, x: f32, y: f32, text: &str, color: u32) {
        self.draw_text_weight(FontWeight::Semibold, font_size, x, y, text, color);
    }

    pub fn draw_text_centered_weight(
        &mut self,
        weight: FontWeight,
        font_size: f32,
        cx: f32,
        cy: f32,
        text: &str,
        color: u32,
    ) {
        let (w, h) = self.measure_text_weight(weight, font_size, text);
        let x = cx - w * 0.5;
        let y = cy - h * 0.5;
        self.draw_text_weight(weight, font_size, x, y, text, color);
    }

    pub fn draw_text_centered(&mut self, is_bold: bool, font_size: f32, cx: f32, cy: f32, text: &str, color: u32) {
        let weight = if is_bold { FontWeight::Bold } else { FontWeight::Regular };
        self.draw_text_centered_weight(weight, font_size, cx, cy, text, color);
    }

    pub fn draw_text_centered_semibold(&mut self, font_size: f32, cx: f32, cy: f32, text: &str, color: u32) {
        self.draw_text_centered_weight(FontWeight::Semibold, font_size, cx, cy, text, color);
    }
}
