//! Pictures shown in Turbine: a viewer window fitting the image to it (its
//! shape kept). The Qt app draws it; in a terminal, kitty's graphics
//! protocol does where the terminal has it (else the desktop's app opens
//! it). O opens it in its app; Esc closes the window.

use super::*;

pub(super) struct Viewer {
    pub path: PathBuf,
    /// The picture's size in pixels, once read (off the UI thread).
    pub dims: Option<(u32, u32)>,
    rx: Option<Receiver<Option<(u32, u32)>>>,
}

impl Workbench {
    /// Whether pictures can show here: the app always; a terminal when it
    /// speaks kitty's graphics protocol.
    pub(super) fn can_show_images(&self) -> bool {
        self.images_inline
    }

    pub(super) fn open_image(&mut self, p: &Path) {
        if let Some((&id, _)) = self.viewers.iter().find(|(_, v)| v.path == p) {
            self.activate_window_id(id);
            return;
        }
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        self.show_plain(&name, "");
        let w = self.windows.last_mut().unwrap();
        w.win.title = name;
        w.special = Some("image");
        w.ed.help_look = false;
        let id = w.win.id;
        let (tx, rx) = std::sync::mpsc::channel();
        let path = p.to_path_buf();
        std::thread::spawn(move || {
            let _ = tx.send(image::image_dimensions(&path).ok());
        });
        self.viewers.insert(id, Viewer { path: p.to_path_buf(), dims: None, rx: Some(rx) });
    }

    fn activate_window_id(&mut self, id: u32) {
        if let Some(i) = self.windows.iter().position(|w| w.win.id == id) {
            let w = self.windows.remove(i);
            self.windows.push(w);
            self.renumber();
        }
    }

    pub(super) fn images_tick(&mut self) {
        let ids: HashSet<u32> = self.windows.iter().map(|w| w.win.id).collect();
        self.viewers.retain(|id, _| ids.contains(id));
        for v in self.viewers.values_mut() {
            if let Some(rx) = &v.rx {
                if let Ok(d) = rx.try_recv() {
                    v.dims = d;
                    v.rx = None;
                }
            }
        }
    }

    /// A viewer's keys: O opens the picture in its app, Esc closes.
    pub(super) fn image_key(&mut self, k: &KeyEvent) -> bool {
        let Some(id) = self.windows.last().map(|w| w.win.id).filter(|id| self.viewers.contains_key(id)) else { return false };
        match k.key {
            Key::Char('o' | 'O') if !k.mods.ctrl && !k.mods.alt => {
                let p = self.viewers[&id].path.clone();
                if let Err(e) = crate::filekind::open_outside(&p) {
                    self.say(format!("Can't open {}: {e}", p.display()));
                }
                true
            }
            Key::Esc => {
                self.run_command("window.close");
                true
            }
            _ => false,
        }
    }

    /// Draw a viewer window's inside: black, the picture's place fitted to
    /// it (cells twice as tall as wide), a caption under it. The picture
    /// itself goes on the surface's image list.
    pub(super) fn draw_viewer(&mut self, c: &mut Canvas, id: u32, inner: Rect) {
        let Some(v) = self.viewers.get(&id) else { return };
        let bg = Style::new(vga::LIGHT_GRAY, vga::BLACK);
        c.fill(inner, " ", bg);
        let name = v.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let caption = match v.dims {
            Some((w, h)) => format!(" {name}  {w} × {h}   O opens it in its app   Esc closes "),
            None if v.rx.is_some() => format!(" {name}  … "),
            None => format!(" {name}: not a picture Turbine can read   O opens it in its app "),
        };
        let cap_y = inner.bottom() - 1;
        c.print(inner.x + ((inner.w - str_width(&caption) as i32) / 2).max(0), cap_y, &caption, Style::new(vga::BLACK, vga::LIGHT_GRAY));
        let Some((iw, ih)) = v.dims else { return };
        let area = Rect::new(inner.x, inner.y, inner.w, (inner.h - 1).max(1));
        if !self.can_show_images() {
            c.print(area.x + 2, area.y + 1, "This terminal can't show pictures (it has no kitty graphics): O opens it in its app.", bg);
            return;
        }
        // Fit: a cell is about half as wide as it's tall.
        let (iw, ih) = (iw.max(1) as f64, ih.max(1) as f64);
        let mut cw = area.w as f64;
        let mut ch = cw * ih / iw / 2.0;
        if ch > area.h as f64 {
            ch = area.h as f64;
            cw = ch * 2.0 * iw / ih;
        }
        let (cw, ch) = (cw.round().max(1.0) as i32, ch.round().max(1.0) as i32);
        let r = Rect::new(area.x + (area.w - cw) / 2, area.y + (area.h - ch) / 2, cw, ch);
        self.image_places.push((r, v.path.clone()));
    }
}
