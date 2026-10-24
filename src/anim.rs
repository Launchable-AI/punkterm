//! Motion primitives. Everything that moves is a critically damped spring or
//! an eased tween, stepped by real frame time so it reads the same at any Hz.

#[derive(Clone, Copy, Debug)]
pub struct Spring {
    pub value: f32,
    pub target: f32,
    vel: f32,
    /// Angular frequency; higher settles faster. ~30 settles in ~150ms.
    omega: f32,
}

impl Spring {
    pub fn new(v: f32, omega: f32) -> Self {
        Spring { value: v, target: v, vel: 0.0, omega }
    }

    pub fn set(&mut self, t: f32) {
        self.target = t;
    }

    pub fn snap(&mut self, v: f32) {
        self.value = v;
        self.target = v;
        self.vel = 0.0;
    }

    /// Advance by `dt` seconds; returns true while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        // Exact critically-damped solution, stable for any dt.
        let x0 = self.value - self.target;
        let w = self.omega;
        let e = (-w * dt).exp();
        let c = self.vel + w * x0;
        let x = (x0 + c * dt) * e;
        self.vel = (c - w * (x0 + c * dt)) * e;
        self.value = self.target + x;
        if x.abs() < 0.01 && self.vel.abs() < 0.05 {
            self.value = self.target;
            self.vel = 0.0;
            return false;
        }
        true
    }
}

pub fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}
