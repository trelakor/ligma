//! Icons Figma's overlay places by name (its `Images`), each drawn into the
//! unit square its command's transform places. Ligma draws its own of each,
//! made to look as Figma's do

use skia_safe::{paint, Canvas, Color4f, Paint, PathBuilder};

/// Draws icon `name`, tinted or as it is, false for one Ligma doesn't have
pub fn draw(canvas: &Canvas, name: &str, tint: Option<Color4f>, opacity: f32) -> bool {
    let color = tint.unwrap_or(Color4f::new(0.0, 0.0, 0.0, 1.0));
    let color = Color4f::new(color.r, color.g, color.b, color.a * opacity);
    match name {
        "AI_SPARKLE_OUTLINE" | "AI_SPARKLE" | "AI_ASSISTANT_FILLED" => {
            // A four pointed star with a small one at its top right
            let star = |path: &mut PathBuilder, cx: f32, cy: f32, r: f32| {
                let k = r * 0.22;
                path.move_to((cx, cy - r));
                path.cubic_to((cx + k * 0.4, cy - k * 1.6), (cx + k * 1.6, cy - k * 0.4), (cx + r, cy));
                path.cubic_to((cx + k * 1.6, cy + k * 0.4), (cx + k * 0.4, cy + k * 1.6), (cx, cy + r));
                path.cubic_to((cx - k * 0.4, cy + k * 1.6), (cx - k * 1.6, cy + k * 0.4), (cx - r, cy));
                path.cubic_to((cx - k * 1.6, cy - k * 0.4), (cx - k * 0.4, cy - k * 1.6), (cx, cy - r));
                path.close();
            };
            let mut path = PathBuilder::new();
            star(&mut path, 0.44, 0.56, 0.34);
            star(&mut path, 0.78, 0.22, 0.14);
            let mut paint = Paint::new(color, None);
            paint.set_anti_alias(true);
            if name == "AI_SPARKLE_OUTLINE" {
                paint.set_style(paint::Style::Stroke);
                paint.set_stroke_width(1.0 / 16.0);
                paint.set_stroke_join(paint::Join::Round);
            }
            canvas.draw_path(&path.detach(), &paint);
            true
        }
        "PLUS_SIGN" | "ICON_CLOSE" => {
            let mut paint = Paint::new(color, None);
            paint.set_anti_alias(true);
            paint.set_style(paint::Style::Stroke);
            paint.set_stroke_width(1.0 / 16.0);
            if name == "ICON_CLOSE" {
                canvas.draw_line((0.3, 0.3), (0.7, 0.7), &paint);
                canvas.draw_line((0.7, 0.3), (0.3, 0.7), &paint);
            } else {
                canvas.draw_line((0.5, 0.25), (0.5, 0.75), &paint);
                canvas.draw_line((0.25, 0.5), (0.75, 0.5), &paint);
            }
            true
        }
        _ => false,
    }
}
