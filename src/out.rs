//! Messages from a working thread to whoever shows them (the window, or the console in headless mode).
use std::sync::mpsc::Sender;

pub enum Msg {
    Line(String),
    Progress(usize, usize, String),
    Status(String),
    Scan(Box<crate::scan::ScanResult>),
    Toast(String, String),
    Done(i32),
}

#[derive(Clone)]
pub struct Out {
    tx: Sender<Msg>,
    ctx: Option<eframe::egui::Context>,
}

impl Out {
    pub fn new(tx: Sender<Msg>, ctx: Option<eframe::egui::Context>) -> Self {
        Out { tx, ctx }
    }
    pub fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
        if let Some(c) = &self.ctx {
            c.request_repaint();
        }
    }
    pub fn line(&self, s: impl Into<String>) {
        self.send(Msg::Line(s.into()));
    }
    pub fn progress(&self, i: usize, n: usize, label: &str) {
        self.send(Msg::Progress(i, n, label.to_string()));
    }
    pub fn status(&self, s: impl Into<String>) {
        self.send(Msg::Status(s.into()));
    }
}
