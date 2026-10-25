//! G10 pop-out windows: any of the main tool windows can live in its own native window
//! (Windows menu). Closing that window docks it back into the main one.
use eframe::egui;

/// (key, title) of the windows that can pop out.
pub const POPOUTS: &[(&str, &str)] = &[
    ("composer", "Composer"),
    ("rules", "Rules"),
    ("packets", "Packets"),
    ("scripts", "Scripts"),
    ("connections", "Connections"),
    ("intercept", "Intercept"),
    ("messages", "Messages"),
];

impl crate::ui::GridApp {
    /// Make the window behind `key` visible (it is drawn only while shown).
    fn popout_show(&mut self, key: &str) {
        match key {
            "composer" => self.show_composer = true,
            "rules" => self.show_rules = true,
            "packets" => self.ext.packets.show = true,
            "scripts" => self.ext.scripts.show = true,
            "connections" => self.ext.conn.show = true,
            "intercept" => self.ext.launch.show = true,
            "messages" => self.ext.streams.open = true,
            _ => {}
        }
    }

    pub fn popout_menu(&mut self, ui: &mut egui::Ui) {
        ui.menu_button("Windows \u{25be}", |ui| {
            ui.label(egui::RichText::new("Show in its own window:").small());
            for (key, title) in POPOUTS {
                let mut on = self.ext.popped.contains(*key);
                if ui.checkbox(&mut on, *title).changed() {
                    if on {
                        self.ext.popped.insert(key.to_string());
                        self.popout_show(key);
                    } else {
                        self.ext.popped.remove(*key);
                    }
                }
            }
        });
    }

    /// Draw a tool window in the main window, or in its own window when popped out.
    pub fn run_window(&mut self, ctx: &egui::Context, key: &str, mut draw: impl FnMut(&mut Self, &egui::Context)) {
        if !self.ext.popped.contains(key) {
            draw(self, ctx);
            return;
        }
        let title = POPOUTS.iter().find(|(k, _)| *k == key).map_or(key, |(_, t)| *t);
        let mut dock = false;
        ctx.show_viewport_immediate(
            egui::ViewportId::from_hash_of(("ferrotap_popout", key)),
            egui::ViewportBuilder::default().with_title(format!("Ferrotap - {title}")).with_inner_size([1000.0, 720.0]),
            |ui, _class| {
                draw(self, ui.ctx());
                if ui.ctx().input(|i| i.viewport().close_requested()) {
                    dock = true;
                }
            },
        );
        if dock {
            self.ext.popped.remove(key);
        }
    }
}
