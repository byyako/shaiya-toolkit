//! Painted icons (egui's bundled fonts have no glyphs for most symbols). Every icon is drawn in a 20 x 20 design box scaled to the
//! rect it gets, with strokes of 1.5 design units, in one colour: the theme's text colour, or the selection colour when active.

use egui::epaint::StrokeKind;
use egui::{Color32, Painter, Pos2, Rect, Response, Sense, Shape, Stroke, Ui, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)] // World, Compare, Dice, Layout and Scatter are used by the later features
pub enum Icon {
    // tools
    Select,
    Box,
    Npc,
    Portal,
    Paint,
    Measure,
    Scatter,
    Place,
    Terrain,
    // panels and tabs
    Maps,
    Outliner,
    Inspector,
    Problems,
    Changes,
    Find,
    Spawns,
    World,
    Compare,
    // small
    Eye,
    EyeOff,
    Lock,
    Unlock,
    Plus,
    Close,
    Collapse,
    MoveSide,
    Chevron,
    Marker,
    Dice,
    Layout,
    ErrorDot,
    WarnTriangle,
    /// Two links of a chain: a section that is edited together with the client map.
    Link,
}

impl Icon {
    #[cfg(test)]
    pub const ALL: [Icon; 33] = [
        Icon::Select,
        Icon::Box,
        Icon::Npc,
        Icon::Portal,
        Icon::Paint,
        Icon::Measure,
        Icon::Scatter,
        Icon::Place,
        Icon::Terrain,
        Icon::Maps,
        Icon::Outliner,
        Icon::Inspector,
        Icon::Problems,
        Icon::Changes,
        Icon::Find,
        Icon::Spawns,
        Icon::World,
        Icon::Compare,
        Icon::Eye,
        Icon::EyeOff,
        Icon::Lock,
        Icon::Unlock,
        Icon::Plus,
        Icon::Close,
        Icon::Collapse,
        Icon::MoveSide,
        Icon::Chevron,
        Icon::Marker,
        Icon::Dice,
        Icon::Layout,
        Icon::ErrorDot,
        Icon::WarnTriangle,
        Icon::Link,
    ];
}

/// Draws `icon` inside `rect` (the design box is centred and scaled to fit).
pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let side = rect.width().min(rect.height());
    let k = side / 20.0;
    let origin = rect.center() - Vec2::splat(side / 2.0);
    let pt = |x: f32, y: f32| -> Pos2 { origin + Vec2::new(x, y) * k };
    let st = Stroke::new(1.5 * k, color);
    let line = |a: (f32, f32), b: (f32, f32)| p.line_segment([pt(a.0, a.1), pt(b.0, b.1)], st);
    let poly = |pts: &[(f32, f32)]| p.add(Shape::closed_line(pts.iter().map(|(x, y)| pt(*x, *y)).collect(), st));
    let open = |pts: &[(f32, f32)]| p.add(Shape::line(pts.iter().map(|(x, y)| pt(*x, *y)).collect(), st));
    let boxr = |x0: f32, y0: f32, x1: f32, y1: f32| Rect::from_min_max(pt(x0, y0), pt(x1, y1));
    let dashed = |x0: f32, y0: f32, x1: f32, y1: f32| {
        let pts = [pt(x0, y0), pt(x1, y0), pt(x1, y1), pt(x0, y1), pt(x0, y0)];
        p.extend(Shape::dashed_line(&pts, st, 2.5 * k, 2.0 * k));
    };
    let dot = |x: f32, y: f32, r: f32| p.circle_filled(pt(x, y), r * k, color);
    match icon {
        Icon::Select => {
            p.add(Shape::convex_polygon(vec![pt(5.0, 3.0), pt(5.0, 16.0), pt(8.5, 12.5), pt(11.5, 17.0), pt(13.5, 16.0), pt(10.5, 11.5), pt(15.0, 11.0)], color.gamma_multiply(0.25), st));
        }
        Icon::Box => {
            dashed(4.0, 4.0, 16.0, 16.0);
            for (x, y) in [(4.0, 4.0), (16.0, 4.0), (4.0, 16.0), (16.0, 16.0)] {
                p.rect_filled(boxr(x - 1.8, y - 1.8, x + 1.8, y + 1.8), 0.0, color);
            }
        }
        Icon::Npc => {
            p.circle_stroke(pt(10.0, 7.5), 4.5 * k, st);
            open(&[(5.8, 9.5), (10.0, 18.0), (14.2, 9.5)]);
            dot(10.0, 7.5, 1.5);
        }
        Icon::Portal => {
            p.circle_stroke(pt(10.0, 10.0), 7.0 * k, st);
            dot(10.0, 10.0, 2.0);
        }
        Icon::Paint => {
            for (x, y, fill) in [(3.0, 3.0, true), (10.5, 3.0, false), (3.0, 10.5, false), (10.5, 10.5, true)] {
                let r = boxr(x, y, x + 6.5, y + 6.5);
                if fill {
                    p.rect_filled(r, 0.0, color);
                } else {
                    p.rect_stroke(r, 0.0, st, StrokeKind::Inside);
                }
            }
        }
        Icon::Measure => {
            poly(&[(2.5, 12.0), (17.5, 12.0), (17.5, 16.5), (2.5, 16.5)]);
            for x in [6.0, 9.5, 13.0] {
                line((x, 12.0), (x, 14.5));
            }
            line((2.5, 7.5), (17.5, 7.5));
            line((2.5, 5.5), (2.5, 9.5));
            line((17.5, 5.5), (17.5, 9.5));
        }
        Icon::Scatter => {
            dashed(3.0, 3.0, 17.0, 17.0);
            for (x, y) in [(7.0, 7.0), (13.0, 8.0), (9.0, 13.5), (14.0, 14.0)] {
                dot(x, y, 1.4);
            }
        }
        Icon::Place => {
            poly(&[(3.0, 16.5), (3.0, 8.5), (10.0, 3.5), (17.0, 8.5), (17.0, 16.5)]);
            line((8.0, 16.5), (8.0, 11.5));
            line((8.0, 11.5), (12.0, 11.5));
            line((12.0, 11.5), (12.0, 16.5));
        }
        Icon::Terrain => {
            // a hill profile over a ground line
            line((3.0, 16.5), (17.0, 16.5));
            open(&[(3.0, 14.0), (7.0, 8.0), (10.0, 11.0), (13.5, 4.5), (17.0, 12.0)]);
        }
        Icon::Maps => {
            for (x, y) in [(3.0, 3.0), (11.0, 3.0), (3.0, 11.0), (11.0, 11.0)] {
                p.rect_stroke(boxr(x, y, x + 6.0, y + 6.0), 0.0, st, StrokeKind::Inside);
            }
        }
        Icon::Outliner => {
            for y in [5.0, 10.0, 15.0] {
                dot(4.5, y, 1.2);
                line((8.0, y), (16.5, y));
            }
        }
        Icon::Inspector => {
            for (y, x) in [(5.0, 7.0), (10.0, 13.0), (15.0, 9.0)] {
                line((3.5, y), (16.5, y));
                p.rect_filled(boxr(x - 1.5, y - 2.2, x + 1.5, y + 2.2), 0.0, color);
            }
        }
        Icon::Problems => {
            poly(&[(10.0, 3.5), (17.5, 16.5), (2.5, 16.5)]);
            line((10.0, 8.0), (10.0, 12.0));
            dot(10.0, 14.4, 1.0);
        }
        Icon::Changes => {
            p.circle_stroke(pt(10.0, 10.0), 7.0 * k, st);
            open(&[(10.0, 5.5), (10.0, 10.0), (13.0, 12.0)]);
        }
        Icon::Find => {
            p.circle_stroke(pt(8.5, 8.5), 5.0 * k, st);
            line((12.2, 12.2), (17.0, 17.0));
        }
        Icon::Spawns => {
            line((3.0, 17.0), (17.0, 17.0));
            for (x, top) in [(5.0, 11.0), (9.0, 6.0), (13.0, 9.0)] {
                p.rect_filled(boxr(x - 1.5, top, x + 1.5, 17.0), 0.0, color);
            }
        }
        Icon::World => {
            p.circle_stroke(pt(10.0, 10.0), 7.0 * k, st);
            line((3.0, 10.0), (17.0, 10.0));
            p.add(Shape::line(vec![pt(10.0, 3.0), pt(6.5, 6.5), pt(6.5, 13.5), pt(10.0, 17.0)], st));
            p.add(Shape::line(vec![pt(10.0, 3.0), pt(13.5, 6.5), pt(13.5, 13.5), pt(10.0, 17.0)], st));
        }
        Icon::Compare => {
            p.rect_stroke(boxr(3.0, 3.0, 11.0, 15.0), 0.0, st, StrokeKind::Inside);
            p.rect_stroke(boxr(9.0, 5.0, 17.0, 17.0), 0.0, st, StrokeKind::Inside);
        }
        Icon::Eye => {
            open(&[(2.5, 10.0), (6.0, 6.0), (10.0, 4.8), (14.0, 6.0), (17.5, 10.0), (14.0, 14.0), (10.0, 15.2), (6.0, 14.0), (2.5, 10.0)]);
            dot(10.0, 10.0, 2.2);
        }
        Icon::EyeOff => {
            open(&[(2.5, 10.0), (6.0, 6.0), (10.0, 4.8), (14.0, 6.0), (17.5, 10.0), (14.0, 14.0), (10.0, 15.2), (6.0, 14.0), (2.5, 10.0)]);
            line((3.5, 16.5), (16.5, 3.5));
        }
        Icon::Lock => {
            p.rect_stroke(boxr(4.5, 9.0, 15.5, 17.0), 1.0 * k, st, StrokeKind::Inside);
            open(&[(7.0, 9.0), (7.0, 6.0), (8.5, 3.8), (11.5, 3.8), (13.0, 6.0), (13.0, 9.0)]);
        }
        Icon::Unlock => {
            p.rect_stroke(boxr(4.5, 9.0, 15.5, 17.0), 1.0 * k, st, StrokeKind::Inside);
            open(&[(7.0, 9.0), (7.0, 6.0), (8.5, 3.8), (11.5, 3.8), (13.0, 5.5)]);
        }
        Icon::Plus => {
            line((10.0, 4.0), (10.0, 16.0));
            line((4.0, 10.0), (16.0, 10.0));
        }
        Icon::Close => {
            line((4.5, 4.5), (15.5, 15.5));
            line((15.5, 4.5), (4.5, 15.5));
        }
        Icon::Collapse => {
            line((4.0, 3.5), (4.0, 16.5));
            open(&[(14.5, 4.5), (8.0, 10.0), (14.5, 15.5)]);
        }
        Icon::MoveSide => {
            line((3.5, 7.0), (16.5, 7.0));
            open(&[(13.0, 3.5), (16.5, 7.0), (13.0, 10.5)]);
            line((16.5, 13.0), (3.5, 13.0));
            open(&[(7.0, 9.5), (3.5, 13.0), (7.0, 16.5)]);
        }
        Icon::Chevron => {
            open(&[(5.0, 7.5), (10.0, 12.5), (15.0, 7.5)]);
        }
        Icon::Marker => {
            p.add(Shape::convex_polygon(vec![pt(4.0, 5.0), pt(16.0, 10.0), pt(4.0, 15.0)], color, Stroke::NONE));
        }
        Icon::Dice => {
            p.rect_stroke(boxr(3.5, 3.5, 16.5, 16.5), 2.0 * k, st, StrokeKind::Inside);
            for (x, y) in [(7.0, 7.0), (13.0, 7.0), (10.0, 10.0), (7.0, 13.0), (13.0, 13.0)] {
                dot(x, y, 1.1);
            }
        }
        Icon::Layout => {
            p.rect_stroke(boxr(3.0, 3.5, 17.0, 16.5), 1.0 * k, st, StrokeKind::Inside);
            line((8.0, 3.5), (8.0, 16.5));
            line((8.0, 11.0), (17.0, 11.0));
        }
        Icon::ErrorDot => {
            p.circle_filled(pt(10.0, 10.0), 5.5 * k, color);
        }
        Icon::Link => {
            p.rect_stroke(boxr(2.0, 6.5, 11.5, 13.5), 3.5 * k, st, StrokeKind::Inside);
            p.rect_stroke(boxr(8.5, 6.5, 18.0, 13.5), 3.5 * k, st, StrokeKind::Inside);
            line((7.0, 10.0), (13.0, 10.0));
        }
        Icon::WarnTriangle => {
            p.add(Shape::convex_polygon(vec![pt(10.0, 3.5), pt(17.5, 16.5), pt(2.5, 16.5)], color, Stroke::NONE));
        }
    }
}

/// A square icon button of this size with a tooltip. `selected` draws it filled with the selection colour; a disabled button is
/// drawn weak and does not click (its tooltip says why).
pub fn icon_button_sized(ui: &mut Ui, icon: Icon, selected: bool, tip: &str, size: f32, enabled: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), if enabled { Sense::click() } else { Sense::hover() });
    if ui.is_rect_visible(rect) {
        let pal = crate::theme::pal_ui(ui);
        if selected {
            ui.painter().rect_filled(rect, 4.0, pal.selection);
        } else if resp.hovered() && enabled {
            ui.painter().rect_filled(rect, 4.0, pal.hover);
        }
        let color = if !enabled {
            pal.weak.gamma_multiply(0.55)
        } else if selected {
            ui.visuals().selection.stroke.color
        } else {
            pal.text
        };
        paint(ui.painter(), rect.shrink(size * 0.14), icon, color);
    }
    if tip.is_empty() {
        resp
    } else if enabled {
        resp.on_hover_text(tip)
    } else {
        resp.on_disabled_hover_text(tip)
    }
}
/// A small mark that cannot be clicked (the chain of a paired section), with a tooltip.
pub fn mark(ui: &mut Ui, icon: Icon, color: Color32, size: f32, tip: &str) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    if ui.is_rect_visible(rect) {
        paint(ui.painter(), rect, icon, color);
    }
    resp.on_hover_text(tip)
}

/// A small text button with a chevron at its right edge (a button that opens a menu). Open the menu with `egui::Popup::menu(&resp)`.
pub fn text_chevron_button(ui: &mut Ui, text: &str) -> Response {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, Color32::PLACEHOLDER);
    let pad = ui.spacing().button_padding;
    let size = Vec2::new(galley.size().x + 14.0 + pad.x * 2.0 + 4.0, galley.size().y + pad.y * 2.0);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let v = *ui.style().interact(&resp);
        ui.painter().rect(rect, v.corner_radius, v.weak_bg_fill, v.bg_stroke, StrokeKind::Inside);
        let color = v.text_color();
        let text_pos = Pos2::new(rect.left() + pad.x, rect.center().y - galley.size().y / 2.0);
        ui.painter().galley_with_override_text_color(text_pos, galley, color);
        paint(ui.painter(), Rect::from_center_size(Pos2::new(rect.right() - pad.x - 7.0, rect.center().y), Vec2::splat(12.0)), Icon::Chevron, color);
    }
    resp
}
#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon paints at least one shape and every shape stays inside the rect (plus half a stroke), in both themes.
    #[test]
    fn icons_paint_inside_rect() {
        for dark in [true, false] {
            for icon in Icon::ALL {
                let ctx = egui::Context::default();
                ctx.set_visuals(if dark { egui::Visuals::dark() } else { egui::Visuals::light() });
                let rect = Rect::from_min_size(Pos2::new(40.0, 30.0), Vec2::splat(28.0));
                let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 100.0))), ..Default::default() };
                let mut out = ctx.run_ui(input, |ui| {
                    let color = crate::theme::pal_ui(ui).text;
                    paint(ui.painter(), rect, icon, color);
                });
                out.textures_delta.clear();
                let mut n = 0;
                fn walk(s: &Shape, rect: Rect, n: &mut usize, icon: Icon) {
                    match s {
                        Shape::Vec(v) => v.iter().for_each(|s| walk(s, rect, n, icon)),
                        Shape::Noop => {}
                        other => {
                            *n += 1;
                            let b = other.visual_bounding_rect();
                            assert!(rect.expand(0.6).contains_rect(b), "{icon:?}: shape {b:?} leaves {rect:?}");
                        }
                    }
                }
                for c in &out.shapes {
                    walk(&c.shape, rect, &mut n, icon);
                }
                assert!(n >= 1, "{icon:?} paints nothing");
            }
        }
    }
}
