//! Orbit camera (Y-up) with perspective / orthographic projection.

use glam::{DVec3, Mat4, Vec3};

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    pub target: DVec3,
    /// Rotation about +Y, radians.
    pub yaw: f64,
    /// Elevation, radians, clamped to [-pi/2, pi/2].
    pub pitch: f64,
    pub distance: f64,
    pub perspective: bool,
    pub fov_y: f64,
}

impl Default for Camera {
    fn default() -> Self {
        let mut c = Camera {
            target: DVec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            distance: 250.0,
            perspective: true,
            fov_y: 40f64.to_radians(),
        };
        c.set_home();
        c
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdView {
    Home,
    Front,
    Back,
    Top,
    Bottom,
    Right,
    Left,
}

impl Camera {
    pub fn set_home(&mut self) {
        self.yaw = 35f64.to_radians();
        self.pitch = 28f64.to_radians();
    }

    pub fn set_view(&mut self, v: StdView) {
        use std::f64::consts::FRAC_PI_2 as H;
        let (yaw, pitch) = match v {
            StdView::Home => (35f64.to_radians(), 28f64.to_radians()),
            StdView::Front => (0.0, 0.0),
            StdView::Back => (2.0 * H, 0.0),
            StdView::Top => (0.0, H),
            StdView::Bottom => (0.0, -H),
            StdView::Right => (H, 0.0),
            StdView::Left => (-H, 0.0),
        };
        self.yaw = yaw;
        self.pitch = pitch;
    }

    /// Unit vector from target toward the eye.
    pub fn dir(&self) -> DVec3 {
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        DVec3::new(cp * sy, sp, cp * cy)
    }

    pub fn right(&self) -> DVec3 {
        DVec3::new(self.yaw.cos(), 0.0, -self.yaw.sin())
    }

    pub fn up(&self) -> DVec3 {
        self.dir().cross(self.right()).normalize()
    }

    pub fn eye(&self) -> DVec3 {
        self.target + self.dir() * self.distance
    }

    pub fn view(&self) -> Mat4 {
        let eye = self.eye().as_vec3();
        Mat4::look_to_rh(eye, -self.dir().as_vec3(), self.up().as_vec3())
    }

    pub fn proj(&self, aspect: f32) -> Mat4 {
        let d = self.distance as f32;
        if self.perspective {
            Mat4::perspective_rh(self.fov_y as f32, aspect, d * 0.005, d * 200.0)
        } else {
            let h = d * (self.fov_y as f32 * 0.5).tan();
            let w = h * aspect;
            Mat4::orthographic_rh(-w, w, -h, h, -d * 100.0, d * 100.0)
        }
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        self.proj(aspect) * self.view()
    }

    /// World-space ray through a normalised device coordinate (x,y in [-1,1], y up).
    pub fn ray(&self, ndc: [f64; 2], aspect: f32) -> (DVec3, DVec3) {
        let inv = self.view_proj(aspect).as_dmat4().inverse();
        let p0 = inv.project_point3(DVec3::new(ndc[0], ndc[1], 0.0));
        let p1 = inv.project_point3(DVec3::new(ndc[0], ndc[1], 1.0));
        (p0, (p1 - p0).normalize())
    }

    pub fn orbit(&mut self, dx: f64, dy: f64) {
        use std::f64::consts::FRAC_PI_2 as H;
        self.yaw -= dx * 0.008;
        self.pitch = (self.pitch + dy * 0.008).clamp(-H, H);
    }

    /// Pan by a screen delta in pixels for a viewport `height_px` tall.
    pub fn pan(&mut self, dx: f64, dy: f64, height_px: f64) {
        let world_per_px = 2.0 * self.distance * (self.fov_y * 0.5).tan() / height_px.max(1.0);
        self.target += (-self.right() * dx + self.up() * dy) * world_per_px;
    }

    /// Zoom by `factor` (<1 zooms in) keeping `anchor` fixed on screen.
    pub fn zoom_at(&mut self, factor: f64, anchor: Option<DVec3>) {
        let factor = factor.clamp(0.2, 5.0);
        let new_dist = (self.distance * factor).clamp(0.01, 1e7);
        let f = new_dist / self.distance;
        if let Some(a) = anchor {
            self.target = a + (self.target - a) * f;
        }
        self.distance = new_dist;
    }

    /// Frame a bounding box.
    pub fn fit(&mut self, lo: DVec3, hi: DVec3) {
        let c = (lo + hi) * 0.5;
        let r = ((hi - lo).length() * 0.5).max(1.0);
        self.target = c;
        self.distance = r / (self.fov_y * 0.5).sin() * 1.15;
    }

    pub fn light_dir(&self) -> Vec3 {
        (self.dir() + self.up() * 0.5 + self.right() * 0.3).normalize().as_vec3()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ray_hits_target_from_center() {
        let c = Camera::default();
        let (o, d) = c.ray([0.0, 0.0], 1.5);
        let t = (c.target - o).dot(d);
        assert!((o + d * t - c.target).length() < 1e-3);
    }
}
