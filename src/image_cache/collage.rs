use anyhow::{anyhow, Result};
use image::{
    codecs::jpeg::JpegEncoder, imageops::FilterType, DynamicImage, GenericImageView, Rgba,
    RgbaImage,
};
use std::io::Cursor;

pub const COVER_W: u32 = 960;
pub const COVER_H: u32 = 540;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollageStyle {
    Cards,
    Diagonal,
    Hero,
    Blended,
}

impl CollageStyle {
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "collage_cards" => Some(Self::Cards),
            "collage_diagonal" => Some(Self::Diagonal),
            "collage_hero" => Some(Self::Hero),
            "collage_blended" => Some(Self::Blended),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Cards => "collage_cards",
            Self::Diagonal => "collage_diagonal",
            Self::Hero => "collage_hero",
            Self::Blended => "collage_blended",
        }
    }
}

/// 根据输入的多张海报/剧照字节流，在阻塞线程池安全地合成 960x540 质感封面图（JPEG）
pub fn render_collage(
    style: CollageStyle,
    posters: &[Vec<u8>],
    backdrop: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let decoded_posters: Vec<DynamicImage> = posters
        .iter()
        .filter_map(|b| image::load_from_memory(b).ok())
        .collect();
    let decoded_backdrop = backdrop.and_then(|b| image::load_from_memory(b).ok());

    if decoded_posters.is_empty() && decoded_backdrop.is_none() {
        return Err(anyhow!("没有可用于拼接的图片资源"));
    }

    let mut canvas = match style {
        CollageStyle::Cards => render_cards(&decoded_posters, decoded_backdrop.as_ref()),
        CollageStyle::Diagonal => render_diagonal(&decoded_posters, decoded_backdrop.as_ref()),
        CollageStyle::Hero => render_hero(&decoded_posters, decoded_backdrop.as_ref()),
        CollageStyle::Blended => render_blended(&decoded_posters, decoded_backdrop.as_ref()),
    };

    apply_vignette_and_scrim(&mut canvas);

    let rgb = DynamicImage::ImageRgba8(canvas).to_rgb8();
    let mut out = Cursor::new(Vec::with_capacity(96 * 1024));
    let mut encoder = JpegEncoder::new_with_quality(&mut out, 88);
    encoder.encode_image(&rgb)?;
    Ok(out.into_inner())
}

/// 等比裁剪并缩放图片以铺满目标宽高（类似 CSS object-fit: cover）
fn fit_cover(img: &DynamicImage, target_w: u32, target_h: u32) -> RgbaImage {
    let (sw, sh) = img.dimensions();
    if sw == 0 || sh == 0 {
        return RgbaImage::new(target_w, target_h);
    }
    let scale_w = target_w as f32 / sw as f32;
    let scale_h = target_h as f32 / sh as f32;
    let scale = scale_w.max(scale_h);
    let rw = ((sw as f32 * scale).round() as u32).max(target_w);
    let rh = ((sh as f32 * scale).round() as u32).max(target_h);

    let resized = img.resize_exact(rw, rh, FilterType::Triangle);
    let cx = (rw - target_w) / 2;
    let cy = (rh - target_h) / 2;
    resized.crop_imm(cx, cy, target_w, target_h).to_rgba8()
}

fn pick_img<'a>(
    posters: &'a [DynamicImage],
    backdrop: Option<&'a DynamicImage>,
    idx: usize,
) -> &'a DynamicImage {
    if !posters.is_empty() {
        &posters[idx % posters.len()]
    } else {
        backdrop.unwrap_or_else(|| {
            static FALLBACK: std::sync::OnceLock<DynamicImage> = std::sync::OnceLock::new();
            FALLBACK.get_or_init(|| DynamicImage::new_rgba8(1, 1))
        })
    }
}

/// 1. 立体叠放卡片：模糊暗色剧照底图 + 3 张微倾圆角海报 + 柔和投影
fn render_cards(posters: &[DynamicImage], backdrop: Option<&DynamicImage>) -> RgbaImage {
    let bg_src = pick_img(posters, backdrop, 0);
    let small_w = 160;
    let small_h = 90;
    let small_bg = fit_cover(bg_src, small_w, small_h);
    let blurred = image::imageops::blur(&small_bg, 6.0);
    let mut canvas = image::imageops::resize(&blurred, COVER_W, COVER_H, FilterType::Triangle);

    // 底图压暗，突出前景卡片
    for p in canvas.pixels_mut() {
        p[0] = (p[0] as f32 * 0.42) as u8;
        p[1] = (p[1] as f32 * 0.42) as u8;
        p[2] = (p[2] as f32 * 0.46) as u8;
    }

    // 卡片参数：(图片索引, 中心X, 中心Y, 宽, 高, 旋转角度, 亮度系数)
    let specs: [(usize, f32, f32, u32, u32, f32, f32); 3] = [
        (
            1,
            COVER_W as f32 * 0.27,
            COVER_H as f32 * 0.54,
            210,
            315,
            -8.5,
            0.86,
        ),
        (
            2,
            COVER_W as f32 * 0.73,
            COVER_H as f32 * 0.54,
            210,
            315,
            8.5,
            0.86,
        ),
        (
            0,
            COVER_W as f32 * 0.50,
            COVER_H as f32 * 0.50,
            240,
            360,
            0.0,
            1.0,
        ),
    ];

    for (idx, cx, cy, cw, ch, angle_deg, brightness) in specs {
        let img = pick_img(posters, backdrop, idx);
        let card = fit_cover(img, cw, ch);
        draw_rounded_rotated_card(&mut canvas, &card, cx, cy, 16.0, angle_deg, brightness);
    }

    canvas
}

fn draw_rounded_rotated_card(
    canvas: &mut RgbaImage,
    card: &RgbaImage,
    cx: f32,
    cy: f32,
    radius: f32,
    angle_deg: f32,
    brightness: f32,
) {
    let cw = card.width() as f32;
    let ch = card.height() as f32;
    let hw = cw * 0.5;
    let hh = ch * 0.5;
    let rad = angle_deg.to_radians();
    let cos_a = rad.cos();
    let sin_a = rad.sin();

    // 先绘制柔和投影
    let pad = 34.0;
    let bound = ((cw * cw + ch * ch).sqrt() * 0.5 + pad).ceil() as i32;
    let min_x = (cx as i32 - bound).max(0);
    let max_x = (cx as i32 + bound).min(COVER_W as i32 - 1);
    let min_y = (cy as i32 - bound).max(0);
    let max_y = (cy as i32 + bound).min(COVER_H as i32 - 1);

    for py in min_y..=max_y {
        for px in min_x..=max_x {
            let dx = px as f32 - cx;
            let dy = py as f32 - (cy + 12.0);
            let lx = dx * cos_a + dy * sin_a;
            let ly = -dx * sin_a + dy * cos_a;
            let dist = rounded_rect_sdf(lx, ly, hw, hh, radius);
            if dist < 24.0 {
                let shadow_alpha = if dist <= 0.0 {
                    0.55
                } else {
                    0.55 * (1.0 - dist / 24.0).powi(2)
                };
                blend_pixel(
                    canvas.get_pixel_mut(px as u32, py as u32),
                    Rgba([0, 0, 0, (shadow_alpha * 255.0) as u8]),
                );
            }
        }
    }

    // 绘制卡片本体 + 1px 微弱高光描边
    for py in min_y..=max_y {
        for px in min_x..=max_x {
            let dx = px as f32 - cx;
            let dy = py as f32 - cy;
            let lx = dx * cos_a + dy * sin_a;
            let ly = -dx * sin_a + dy * cos_a;
            let dist = rounded_rect_sdf(lx, ly, hw, hh, radius);
            if dist < 0.8 {
                let sx = (lx + hw).clamp(0.0, cw - 1.001);
                let sy = (ly + hh).clamp(0.0, ch - 1.001);
                let mut sp = sample_bilinear(card, sx, sy);
                sp[0] = (sp[0] as f32 * brightness).min(255.0) as u8;
                sp[1] = (sp[1] as f32 * brightness).min(255.0) as u8;
                sp[2] = (sp[2] as f32 * brightness).min(255.0) as u8;

                // 内边框微高光
                if dist > -1.4 {
                    sp[0] = sp[0].saturating_add(28);
                    sp[1] = sp[1].saturating_add(28);
                    sp[2] = sp[2].saturating_add(28);
                }

                let aa = if dist > -0.8 {
                    ((0.8 - dist) / 1.6).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                sp[3] = (aa * 255.0) as u8;
                blend_pixel(canvas.get_pixel_mut(px as u32, py as u32), sp);
            }
        }
    }
}

/// 2. 斜切电影条带：4 张竖版海报按 ~13° 斜切并排 + 细暗色分隔线
fn render_diagonal(posters: &[DynamicImage], backdrop: Option<&DynamicImage>) -> RgbaImage {
    let mut canvas = RgbaImage::from_pixel(COVER_W, COVER_H, Rgba([10, 12, 16, 255]));
    let tiles: Vec<RgbaImage> = (0..4)
        .map(|i| fit_cover(pick_img(posters, backdrop, i), 360, COVER_H))
        .collect();

    let slope = 0.23_f32; // tan(~13 deg)
    let strip_w = COVER_W as f32 / 4.0;
    let gap = 3.0_f32;

    for y in 0..COVER_H {
        let dy = y as f32 - COVER_H as f32 * 0.5;
        for x in 0..COVER_W {
            let shifted_x = x as f32 + dy * slope;
            let raw_idx = (shifted_x / strip_w).floor() as i32;
            let idx = raw_idx.clamp(0, 3) as usize;
            let local_x_in_strip = shifted_x - raw_idx as f32 * strip_w;

            // 条带边界留出 3px 深色分隔线
            if raw_idx > 0 && local_x_in_strip < gap {
                continue;
            }
            if raw_idx < 3 && (strip_w - local_x_in_strip) < gap {
                continue;
            }

            let tile_x = ((local_x_in_strip / strip_w) * 260.0 + 50.0).clamp(0.0, 359.0) as u32;
            let mut px = *tiles[idx].get_pixel(tile_x, y);
            px[0] = (px[0] as f32 * 0.90) as u8;
            px[1] = (px[1] as f32 * 0.90) as u8;
            px[2] = (px[2] as f32 * 0.92) as u8;
            canvas.put_pixel(x, y, px);
        }
    }
    canvas
}

/// 3. 1 主 3 辅非对称：左侧主剧照渐变过渡 + 右侧 3 张精选缩略图
fn render_hero(posters: &[DynamicImage], backdrop: Option<&DynamicImage>) -> RgbaImage {
    let mut canvas = RgbaImage::from_pixel(COVER_W, COVER_H, Rgba([10, 12, 18, 255]));
    let hero_img = pick_img(posters, backdrop, 0);
    let hero = fit_cover(hero_img, 650, COVER_H);

    // 绘制左侧主视觉（在 x: 540..650 平滑淡出）
    for y in 0..COVER_H {
        for x in 0..650 {
            let mut px = *hero.get_pixel(x, y);
            if x > 530 {
                let fade = 1.0 - (x - 530) as f32 / 120.0;
                px[0] = (px[0] as f32 * fade) as u8;
                px[1] = (px[1] as f32 * fade) as u8;
                px[2] = (px[2] as f32 * fade) as u8;
            }
            canvas.put_pixel(x, y, px);
        }
    }

    // 右侧 3 个竖排缩略图（x: 626..960，留 4px 间隔）
    let right_x = 626_u32;
    let right_w = COVER_W - right_x;
    let gap = 4_u32;
    let cell_h = (COVER_H - gap * 2) / 3;

    for i in 0..3 {
        let img = pick_img(posters, backdrop, i + 1);
        let cell = fit_cover(img, right_w, cell_h);
        let y_off = i as u32 * (cell_h + gap);
        for cy in 0..cell_h {
            for cx in 0..right_w {
                let mut px = *cell.get_pixel(cx, cy);
                // 左侧边缘与主视觉融合过渡
                let edge_fade = if cx < 28 {
                    0.4 + 0.6 * (cx as f32 / 28.0)
                } else {
                    1.0
                };
                px[0] = (px[0] as f32 * 0.88 * edge_fade) as u8;
                px[1] = (px[1] as f32 * 0.88 * edge_fade) as u8;
                px[2] = (px[2] as f32 * 0.90 * edge_fade) as u8;
                canvas.put_pixel(right_x + cx, y_off + cy, px);
            }
        }
    }
    canvas
}

/// 4. 羽化融合 4 连：4 张竖版海报横向平滑羽化过渡 + 统一暗调
fn render_blended(posters: &[DynamicImage], backdrop: Option<&DynamicImage>) -> RgbaImage {
    let mut canvas = RgbaImage::from_pixel(COVER_W, COVER_H, Rgba([10, 12, 16, 255]));
    let tile_w = 276_u32;
    let step = 228_u32; // 每张重叠 48px 做羽化
    let overlap = (tile_w - step) as f32;

    for i in 0..4 {
        let tile = fit_cover(pick_img(posters, backdrop, i), tile_w, COVER_H);
        let start_x = i as u32 * step;
        for y in 0..COVER_H {
            for lx in 0..tile_w {
                let cx = start_x + lx;
                if cx >= COVER_W {
                    continue;
                }
                let alpha = if i > 0 && (lx as f32) < overlap {
                    lx as f32 / overlap
                } else {
                    1.0
                };
                let mut px = *tile.get_pixel(lx, y);
                px[0] = (px[0] as f32 * 0.88) as u8;
                px[1] = (px[1] as f32 * 0.88) as u8;
                px[2] = (px[2] as f32 * 0.90) as u8;
                px[3] = (alpha * 255.0) as u8;
                blend_pixel(canvas.get_pixel_mut(cx, y), px);
            }
        }
    }
    canvas
}

/// 全局四周暗角渐晕 + 底部渐变遮罩，保证客户端白色标题文字清晰可读
fn apply_vignette_and_scrim(canvas: &mut RgbaImage) {
    let hw = COVER_W as f32 * 0.5;
    let hh = COVER_H as f32 * 0.5;
    for y in 0..COVER_H {
        let ny = (y as f32 - hh) / hh;
        let bottom_scrim = if y > COVER_H * 62 / 100 {
            let t = (y as f32 - COVER_H as f32 * 0.62) / (COVER_H as f32 * 0.38);
            1.0 - 0.36 * t
        } else {
            1.0
        };
        for x in 0..COVER_W {
            let nx = (x as f32 - hw) / hw;
            let r2 = (nx * nx + ny * ny).min(1.6);
            let vignette = (1.0 - 0.24 * r2).clamp(0.55, 1.0);
            let factor = vignette * bottom_scrim;
            let p = canvas.get_pixel_mut(x, y);
            p[0] = (p[0] as f32 * factor) as u8;
            p[1] = (p[1] as f32 * factor) as u8;
            p[2] = (p[2] as f32 * factor) as u8;
        }
    }
}

fn rounded_rect_sdf(px: f32, py: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = px.abs() - (hw - r);
    let qy = py.abs() - (hh - r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - r
}

fn sample_bilinear(img: &RgbaImage, x: f32, y: f32) -> Rgba<u8> {
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(img.width() - 1);
    let y1 = (y0 + 1).min(img.height() - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;

    let p00 = img.get_pixel(x0, y0);
    let p10 = img.get_pixel(x1, y0);
    let p01 = img.get_pixel(x0, y1);
    let p11 = img.get_pixel(x1, y1);

    let mut out = [0u8; 4];
    for c in 0..4 {
        let v = (p00[c] as f32 * (1.0 - fx) * (1.0 - fy))
            + (p10[c] as f32 * fx * (1.0 - fy))
            + (p01[c] as f32 * (1.0 - fx) * fy)
            + (p11[c] as f32 * fx * fy);
        out[c] = v.round().clamp(0.0, 255.0) as u8;
    }
    Rgba(out)
}

fn blend_pixel(dst: &mut Rgba<u8>, src: Rgba<u8>) {
    let a = src[3] as f32 / 255.0;
    let inv = 1.0 - a;
    dst[0] = (src[0] as f32 * a + dst[0] as f32 * inv)
        .round()
        .clamp(0.0, 255.0) as u8;
    dst[1] = (src[1] as f32 * a + dst[1] as f32 * inv)
        .round()
        .clamp(0.0, 255.0) as u8;
    dst[2] = (src[2] as f32 * a + dst[2] as f32 * inv)
        .round()
        .clamp(0.0, 255.0) as u8;
    dst[3] = 255;
}
