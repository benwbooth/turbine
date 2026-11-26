//! Movable, resizable, zoomable windows.

use crate::frame::{draw_frame, draw_title};
use crate::geom::{Point, Rect};
use crate::surface::Canvas;
use crate::theme::Theme;

/// What part of a window a point is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    None,
    Inside,
    Title,
    Close,
    Zoom,
    Left,
    Right,
    Bottom,
    BottomLeft,
    BottomRight,
    VScroll,
    HScroll,
}

#[derive(Clone, Copy, Debug)]
enum Drag {
    Move { grab: Point },
    Resize { hit: Hit, start: Point, orig: Rect },
}

#[derive(Debug)]
pub struct Window {
    pub id: u32,
    pub title: String,
    pub rect: Rect,
    pub number: Option<u32>,
    pub zoomed: bool,
    saved: Rect,
    drag: Option<Drag>,
    pub min_w: i32,
    pub min_h: i32,
}

impl Window {
    pub fn new(id: u32, title: &str, rect: Rect) -> Self {
        Window { id, title: title.into(), rect, number: None, zoomed: false, saved: rect, drag: None, min_w: 16, min_h: 5 }
    }

    /// The area inside the frame.
    pub fn inner(&self) -> Rect {
        self.rect.inset(1)
    }

    pub fn close_rect(&self) -> Rect {
        Rect::new(self.rect.x + 2, self.rect.y, 3, 1)
    }

    pub fn zoom_rect(&self) -> Rect {
        Rect::new(self.rect.right() - 5, self.rect.y, 3, 1)
    }

    pub fn hit(&self, p: Point) -> Hit {
        let r = self.rect;
        if !r.contains(p) {
            return Hit::None;
        }
        let (l, rt, t, b) = (r.x, r.right() - 1, r.y, r.bottom() - 1);
        if p.y == t {
            if self.close_rect().contains(p) {
                return Hit::Close;
            }
            if self.zoom_rect().contains(p) {
                return Hit::Zoom;
            }
            return Hit::Title;
        }
        if p.y == b && p.x == rt {
            return Hit::BottomRight;
        }
        if p.y == b && p.x == l {
            return Hit::BottomLeft;
        }
        if p.x == rt {
            return if p.y > t + 1 && p.y < b - 1 { Hit::VScroll } else { Hit::Right };
        }
        if p.y == b {
            return if p.x > l + 2 && p.x < rt - 2 { Hit::HScroll } else { Hit::Bottom };
        }
        if p.x == l {
            return Hit::Left;
        }
        Hit::Inside
    }

    /// Start a drag from a mouse press on `hit`.
    pub fn begin_drag(&mut self, hit: Hit, p: Point) -> bool {
        match hit {
            Hit::Title => {
                self.drag = Some(Drag::Move { grab: Point::new(p.x - self.rect.x, p.y - self.rect.y) });
                true
            }
            Hit::Left | Hit::Right | Hit::Bottom | Hit::BottomLeft | Hit::BottomRight => {
                self.drag = Some(Drag::Resize { hit, start: p, orig: self.rect });
                true
            }
            _ => false,
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Continue a drag; `bounds` is the desktop area windows live in.
    pub fn drag_to(&mut self, p: Point, bounds: Rect) {
        let Some(d) = self.drag else { return };
        if self.zoomed {
            self.zoomed = false;
        }
        match d {
            Drag::Move { grab } => {
                let x = (p.x - grab.x).clamp(bounds.x - self.rect.w + 4, bounds.right() - 4);
                let y = (p.y - grab.y).clamp(bounds.y, bounds.bottom() - 1);
                self.rect.x = x;
                self.rect.y = y;
            }
            Drag::Resize { hit, start, orig } => {
                let dx = p.x - start.x;
                let dy = p.y - start.y;
                let mut r = orig;
                if matches!(hit, Hit::Right | Hit::BottomRight) {
                    r.w = (orig.w + dx).max(self.min_w);
                }
                if matches!(hit, Hit::Left | Hit::BottomLeft) {
                    let w = (orig.w - dx).max(self.min_w);
                    r.x = orig.right() - w;
                    r.w = w;
                }
                if matches!(hit, Hit::Bottom | Hit::BottomLeft | Hit::BottomRight) {
                    r.h = (orig.h + dy).max(self.min_h).min(bounds.bottom() - r.y);
                }
                self.rect = r;
            }
        }
    }

    pub fn end_drag(&mut self) {
        self.drag = None;
    }

    pub fn toggle_zoom(&mut self, bounds: Rect) {
        if self.zoomed {
            self.rect = self.saved;
            self.zoomed = false;
        } else {
            self.saved = self.rect;
            self.rect = bounds;
            self.zoomed = true;
        }
    }

    /// Keep the window usable after the screen shrinks or grows.
    pub fn fit(&mut self, bounds: Rect) {
        if self.zoomed {
            self.rect = bounds;
            return;
        }
        self.rect.w = self.rect.w.min(bounds.w).max(self.min_w.min(bounds.w));
        self.rect.h = self.rect.h.min(bounds.h).max(self.min_h.min(bounds.h));
        self.rect.x = self.rect.x.clamp(bounds.x, (bounds.right() - self.rect.w).max(bounds.x));
        self.rect.y = self.rect.y.clamp(bounds.y, (bounds.bottom() - self.rect.h).max(bounds.y));
    }

    pub fn draw_frame(&self, c: &mut Canvas, theme: &Theme, active: bool) {
        let s = &theme.scheme;
        let st = &theme.style;
        let (fc, style) = if active { (st.active_frame, s.frame_active) } else { (st.inactive_frame, s.frame) };
        draw_frame(c, self.rect, fc, style);
        draw_title(c, self.rect, &self.title, style);
        if active {
            if !st.close_icon.is_empty() {
                c.print(self.rect.x + 2, self.rect.y, st.close_icon, s.frame_icons);
            }
            c.print(self.zoom_rect().x, self.rect.y, if self.zoomed { "[↕]" } else { st.zoom_icon }, s.frame_icons);
            c.print(self.rect.right() - 1, self.rect.bottom() - 1, st.resize_grip, s.frame_icons);
        }
        if let Some(n) = self.number {
            let t = n.to_string();
            c.print(self.rect.right() - 7 - t.len() as i32, self.rect.y, &t, style);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_and_resize() {
        let bounds = Rect::new(0, 1, 80, 23);
        let mut w = Window::new(1, "t", Rect::new(10, 5, 30, 10));
        assert_eq!(w.hit(Point::new(20, 5)), Hit::Title);
        assert!(w.begin_drag(Hit::Title, Point::new(20, 5)));
        w.drag_to(Point::new(25, 7), bounds);
        w.end_drag();
        assert_eq!((w.rect.x, w.rect.y), (15, 7));
        let corner = Point::new(w.rect.right() - 1, w.rect.bottom() - 1);
        assert_eq!(w.hit(corner), Hit::BottomRight);
        w.begin_drag(Hit::BottomRight, corner);
        w.drag_to(Point::new(corner.x + 5, corner.y + 2), bounds);
        assert_eq!((w.rect.w, w.rect.h), (35, 12));
    }
}
