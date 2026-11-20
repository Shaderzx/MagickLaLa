use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Camera {
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self { pan_x: 0.0, pan_y: 0.0, zoom: 1.0 }
    }
}

impl Camera {
    pub const MIN_ZOOM: f32 = 0.4;
    pub const MAX_ZOOM: f32 = 2.2;

    pub fn world_to_screen(&self, world: [f32; 2]) -> [f32; 2] {
        [world[0] * self.zoom + self.pan_x, world[1] * self.zoom + self.pan_y]
    }

    pub fn screen_to_world(&self, screen: [f32; 2]) -> [f32; 2] {
        [(screen[0] - self.pan_x) / self.zoom, (screen[1] - self.pan_y) / self.zoom]
    }

    pub fn pan_by(&mut self, dx: f32, dy: f32) {
        self.pan_x += dx;
        self.pan_y += dy;
    }

    pub fn pan(&mut self, dx: f32, dy: f32) {
        self.pan_by(dx, dy);
    }

    pub fn zoom_at(&mut self, pointer: [f32; 2], factor: f32) {
        let anchor = self.screen_to_world(pointer);
        self.zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        self.pan_x = pointer[0] - anchor[0] * self.zoom;
        self.pan_y = pointer[1] - anchor[1] * self.zoom;
    }

    pub fn zoom_by(&mut self, factor: f32, pivot: [f32; 2]) {
        self.zoom_at(pivot, factor);
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn zoom_percent(&self) -> u32 {
        (self.zoom * 100.0).round() as u32
    }
}
