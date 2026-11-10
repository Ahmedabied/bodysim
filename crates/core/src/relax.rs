//! THE PRIMITIVE. `x(t) = x∞ + (x0−x∞)·e^{−(t−t0)/τ}` with its exact crossing-time inverse -
//! the model used most in the body. Plus the other closed forms: Linear (bladder, glycogen),
//! Logistic (growth to a ceiling), Bateman (PK), Expm2 (two coupled first-order stores, as in the
//! Gagge two-node thermal model), PiecewiseLinear (timelines). Every one answers "when does it cross x?"
//! analytically or by bisection on a bracket - never by stepping.
//!
//! `Reach::Never` is a first-class answer: the caller must refuse to schedule and log it.
use crate::time::{SimTime, Span};
use serde::{Deserialize, Serialize};

pub trait Trajectory {
    fn at(&self, t: SimTime) -> f64;
    /// First t ≥ t0 at which the value is ≥ `x`. `Already` if it is now; `Never` if it moves away or saturates below.
    fn reach_up(&self, x: f64) -> Reach;
    /// First t ≥ t0 at which the value is ≤ `x`.
    fn reach_down(&self, x: f64) -> Reach;
    /// Where the value is heading (the asymptote / limit), used by the ratchet watch.
    fn limit(&self) -> f64;
    /// Reach `x` in the direction of travel: `reach_up` if the trajectory rises, `reach_down` if it falls.
    fn crossing(&self, x: f64) -> Reach {
        let (x0, lim) = (self.at(SimTime(i64::MIN)), self.limit());
        if lim >= x0 { self.reach_up(x) } else { self.reach_down(x) }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Reach {
    At(SimTime),
    /// The threshold lies beyond the asymptote: refuse to schedule, log `Unreachable`.
    Never { x_inf: f64 },
    /// The value is already at or past the threshold.
    Already,
}
impl Reach {
    pub fn time(self) -> Option<SimTime> { if let Reach::At(t) = self { Some(t) } else { None } }
}

/// Anchored exponential relaxation toward `x_inf` with time constant `tau`.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Relax { pub x0: f64, pub x_inf: f64, pub t0: SimTime, pub tau: Span }

impl Relax {
    pub fn new(x0: f64, x_inf: f64, t0: SimTime, tau: Span) -> Relax { Relax { x0, x_inf, t0, tau } }
    /// A value that does not move.
    pub fn constant(x: f64, t0: SimTime) -> Relax { Relax { x0: x, x_inf: x, t0, tau: Span::secs(1) } }
    /// Continue from the current value with a new target and time constant. Value-continuous at `t`.
    pub fn reanchor(&self, t: SimTime, x_inf: f64, tau: Span) -> Relax { Relax { x0: self.at(t), x_inf, t0: t, tau } }
    /// Same τ, new target.
    pub fn retarget(&self, t: SimTime, x_inf: f64) -> Relax { self.reanchor(t, x_inf, self.tau) }
    /// Jump the current value (e.g. a kick) keeping target and τ.
    pub fn kick(&self, t: SimTime, delta: f64) -> Relax { Relax { x0: self.at(t) + delta, x_inf: self.x_inf, t0: t, tau: self.tau } }
    pub fn settled(&self, t: SimTime, eps: f64) -> bool { (self.at(t) - self.x_inf).abs() <= eps }
}

impl Trajectory for Relax {
    fn at(&self, t: SimTime) -> f64 {
        if t <= self.t0 { return self.x0; }
        if self.tau.0 <= 0 { return self.x_inf; }
        let dt = (t - self.t0).as_secs_f64();
        self.x_inf + (self.x0 - self.x_inf) * (-dt / self.tau.as_secs_f64()).exp()
    }
    fn limit(&self) -> f64 { self.x_inf }
    fn reach_up(&self, x: f64) -> Reach {
        if self.x0 >= x { return Reach::Already; }
        if self.x_inf <= self.x0 || x >= self.x_inf { return Reach::Never { x_inf: self.x_inf }; }
        if self.tau.0 <= 0 { return Reach::At(self.t0); }
        let s = self.tau.as_secs_f64() * ((self.x0 - self.x_inf) / (x - self.x_inf)).ln();
        Reach::At(self.t0 + Span::from_secs_f64(s))
    }
    fn reach_down(&self, x: f64) -> Reach {
        if self.x0 <= x { return Reach::Already; }
        if self.x_inf >= self.x0 || x <= self.x_inf { return Reach::Never { x_inf: self.x_inf }; }
        if self.tau.0 <= 0 { return Reach::At(self.t0); }
        let s = self.tau.as_secs_f64() * ((self.x0 - self.x_inf) / (x - self.x_inf)).ln();
        Reach::At(self.t0 + Span::from_secs_f64(s))
    }
}

/// Constant-rate accumulation: bladder volume, glycogen, colon fill, dust.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Linear { pub x0: f64, /// per second
    pub rate: f64, pub t0: SimTime }

impl Linear {
    pub fn new(x0: f64, rate: f64, t0: SimTime) -> Linear { Linear { x0, rate, t0 } }
    pub fn reanchor(&self, t: SimTime, rate: f64) -> Linear { Linear { x0: self.at(t), rate, t0: t } }
    pub fn reset(&self, t: SimTime, x0: f64) -> Linear { Linear { x0, rate: self.rate, t0: t } }
}
impl Trajectory for Linear {
    fn at(&self, t: SimTime) -> f64 {
        if t <= self.t0 { return self.x0; }
        self.x0 + self.rate * (t - self.t0).as_secs_f64()
    }
    fn limit(&self) -> f64 { if self.rate > 0.0 { f64::INFINITY } else if self.rate < 0.0 { f64::NEG_INFINITY } else { self.x0 } }
    fn reach_up(&self, x: f64) -> Reach {
        if self.x0 >= x { return Reach::Already; }
        if self.rate <= 0.0 { return Reach::Never { x_inf: self.limit() }; }
        Reach::At(self.t0 + Span::from_secs_f64((x - self.x0) / self.rate))
    }
    fn reach_down(&self, x: f64) -> Reach {
        if self.x0 <= x { return Reach::Already; }
        if self.rate >= 0.0 { return Reach::Never { x_inf: self.limit() }; }
        Reach::At(self.t0 + Span::from_secs_f64((x - self.x0) / self.rate))
    }
}

/// Logistic growth/decay toward `cap` (r>0) or 0 (r<0).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Logistic { pub x0: f64, pub cap: f64, /// per second
    pub r: f64, pub t0: SimTime }
impl Logistic {
    pub fn limit(&self) -> f64 { if self.r > 0.0 { self.cap } else if self.r < 0.0 { 0.0 } else { self.x0 } }
    pub fn reanchor(&self, t: SimTime, r: f64) -> Logistic { Logistic { x0: self.at(t), cap: self.cap, r, t0: t } }
}
impl Trajectory for Logistic {
    fn at(&self, t: SimTime) -> f64 {
        if t <= self.t0 || self.r == 0.0 { return self.x0; }
        if self.x0 <= 0.0 { return 0.0; }
        if self.x0 >= self.cap { return self.cap; }
        let a = (self.cap - self.x0) / self.x0;
        self.cap / (1.0 + a * (-self.r * (t - self.t0).as_secs_f64()).exp())
    }
    fn limit(&self) -> f64 { Logistic::limit(self) }
    fn reach_up(&self, x: f64) -> Reach {
        if self.x0 >= x { return Reach::Already; }
        let lim = Logistic::limit(self);
        if self.r <= 0.0 || self.x0 <= 0.0 || x >= lim { return Reach::Never { x_inf: lim }; }
        let a = (self.cap - self.x0) / self.x0;
        let s = -((self.cap / x - 1.0) / a).ln() / self.r;
        Reach::At(self.t0 + Span::from_secs_f64(s.max(0.0)))
    }
    fn reach_down(&self, x: f64) -> Reach {
        if self.x0 <= x { return Reach::Already; }
        let lim = Logistic::limit(self);
        if self.r >= 0.0 || self.x0 >= self.cap || x <= lim { return Reach::Never { x_inf: lim }; }
        let a = (self.cap - self.x0) / self.x0;
        let s = -((self.cap / x - 1.0) / a).ln() / self.r;
        Reach::At(self.t0 + Span::from_secs_f64(s.max(0.0)))
    }
}

/// One-compartment first-order absorption/elimination (Bateman). Concentration units = dose/vd.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Bateman { pub dose: f64, /// bioavailability 0..1
    pub f: f64, /// absorption rate, per second
    pub ka: f64, /// elimination rate, per second
    pub ke: f64, pub vd: f64, pub t0: SimTime }
impl Bateman {
    pub fn t_peak(&self) -> f64 {
        if (self.ka - self.ke).abs() < 1e-15 { 1.0 / self.ka } else { (self.ka / self.ke).ln() / (self.ka - self.ke) }
    }
    pub fn c_max(&self) -> f64 { self.at(self.t0 + Span::from_secs_f64(self.t_peak())) }
    /// First time after the peak at which concentration falls to `x`.
    pub fn crossing_down(&self, x: f64) -> Reach {
        let cmax = self.c_max();
        if x >= cmax { return Reach::Already; }
        if x <= 0.0 { return Reach::Never { x_inf: 0.0 }; }
        let tp = self.t_peak();
        let hi = tp + 80.0 / self.ke.max(1e-12);
        let f = |s: f64| self.at(self.t0 + Span::from_secs_f64(s)) - x;
        Reach::At(self.t0 + Span::from_secs_f64(bisect(f, tp, hi, 80)))
    }
}
impl Trajectory for Bateman {
    fn at(&self, t: SimTime) -> f64 {
        if t <= self.t0 { return 0.0; }
        let dt = (t - self.t0).as_secs_f64();
        let scale = self.f * self.dose / self.vd;
        if (self.ka - self.ke).abs() < 1e-15 {
            scale * self.ka * dt * (-self.ka * dt).exp()
        } else {
            scale * self.ka / (self.ka - self.ke) * ((-self.ke * dt).exp() - (-self.ka * dt).exp())
        }
    }
    fn limit(&self) -> f64 { 0.0 }
    /// First (rising) crossing above `x`.
    fn reach_up(&self, x: f64) -> Reach {
        if x <= 0.0 { return Reach::Already; }
        let cmax = self.c_max();
        if x > cmax { return Reach::Never { x_inf: cmax }; }
        let tp = self.t_peak();
        let f = |s: f64| self.at(self.t0 + Span::from_secs_f64(s)) - x;
        Reach::At(self.t0 + Span::from_secs_f64(bisect(f, 0.0, tp, 80)))
    }
    /// Concentration starts at 0, so "≤ x" holds at t0 for any x ≥ 0; use `crossing_down` for the fall after the peak.
    fn reach_down(&self, x: f64) -> Reach { if x >= 0.0 { Reach::Already } else { Reach::Never { x_inf: 0.0 } } }
}

/// Two-state linear system `dx/dt = A x + u`, solved in closed form (2×2 matrix exponential).
/// 2R2C rooms, hand+cup contact, Gagge core/skin.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Expm2 { pub a: [[f64; 2]; 2], pub u: [f64; 2], pub x0: [f64; 2], pub t0: SimTime }

impl Expm2 {
    /// e^{A s} for a real 2×2 matrix, closed form via the characteristic decomposition.
    pub fn expm(a: [[f64; 2]; 2], s: f64) -> [[f64; 2]; 2] {
        let tr = a[0][0] + a[1][1];
        let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
        let alpha = tr / 2.0;
        let delta = alpha * alpha - det;
        let id = [[1.0, 0.0], [0.0, 1.0]];
        let sub = |m: [[f64; 2]; 2], l: f64| [[m[0][0] - l, m[0][1]], [m[1][0], m[1][1] - l]];
        let scale = |m: [[f64; 2]; 2], k: f64| [[m[0][0] * k, m[0][1] * k], [m[1][0] * k, m[1][1] * k]];
        let add = |p: [[f64; 2]; 2], q: [[f64; 2]; 2]| [[p[0][0] + q[0][0], p[0][1] + q[0][1]], [p[1][0] + q[1][0], p[1][1] + q[1][1]]];
        if delta > 1e-18 {
            let w = delta.sqrt();
            let (l1, l2) = (alpha + w, alpha - w);
            let (e1, e2) = ((l1 * s).exp(), (l2 * s).exp());
            scale(add(scale(sub(a, l2), e1), scale(sub(a, l1), -e2)), 1.0 / (l1 - l2))
        } else if delta < -1e-18 {
            let w = (-delta).sqrt();
            let e = (alpha * s).exp();
            let (c, sn) = ((w * s).cos(), (w * s).sin() / w);
            scale(add(scale(id, c), scale(sub(a, alpha), sn)), e)
        } else {
            let e = (alpha * s).exp();
            scale(add(id, scale(sub(a, alpha), s)), e)
        }
    }
    fn mul(m: [[f64; 2]; 2], v: [f64; 2]) -> [f64; 2] { [m[0][0] * v[0] + m[0][1] * v[1], m[1][0] * v[0] + m[1][1] * v[1]] }
    pub fn state_at(&self, t: SimTime) -> [f64; 2] {
        if t <= self.t0 { return self.x0; }
        let s = (t - self.t0).as_secs_f64();
        let m = Self::expm(self.a, s);
        let hom = Self::mul(m, self.x0);
        let part = match Self::det_if_invertible(self.a) {
            // A⁻¹ (e^{As} − I) u
            Some(det) => {
                let inv = [[self.a[1][1] / det, -self.a[0][1] / det], [-self.a[1][0] / det, self.a[0][0] / det]];
                let mi = [[m[0][0] - 1.0, m[0][1]], [m[1][0], m[1][1] - 1.0]];
                Self::mul(inv, Self::mul(mi, self.u))
            }
            // No inverse to lean on (two bodies that only exchange with each other lose nothing, so A is singular):
            // (∫₀ˢ e^{Aτ} dτ) u, which exists for every A.
            None => Self::mul(Self::integral(self.a, s), self.u),
        };
        [hom[0] + part[0], hom[1] + part[1]]
    }
    /// det A when A can be inverted without losing the answer to rounding: the determinant must not be a
    /// difference of two products that nearly cancel.
    fn det_if_invertible(a: [[f64; 2]; 2]) -> Option<f64> {
        let (p, q) = (a[0][0] * a[1][1], a[0][1] * a[1][0]);
        let det = p - q;
        if det.abs() > 1e-9 * (p.abs() + q.abs()) && det.abs() > 1e-300 { Some(det) } else { None }
    }
    /// ∫₀ˢ e^{Aτ} dτ in closed form, by the same decomposition as `expm`. Needs no inverse, so it is exact for a
    /// singular A too (an eigenvalue of 0 contributes `s`, not `(e^{0·s} − 1) / 0`).
    pub fn integral(a: [[f64; 2]; 2], s: f64) -> [[f64; 2]; 2] {
        let tr = a[0][0] + a[1][1];
        let det = a[0][0] * a[1][1] - a[0][1] * a[1][0];
        let alpha = tr / 2.0;
        let delta = alpha * alpha - det;
        let sub = |m: [[f64; 2]; 2], l: f64| [[m[0][0] - l, m[0][1]], [m[1][0], m[1][1] - l]];
        let scale = |m: [[f64; 2]; 2], k: f64| [[m[0][0] * k, m[0][1] * k], [m[1][0] * k, m[1][1] * k]];
        let add = |p: [[f64; 2]; 2], q: [[f64; 2]; 2]| [[p[0][0] + q[0][0], p[0][1] + q[0][1]], [p[1][0] + q[1][0], p[1][1] + q[1][1]]];
        // ∫₀ˢ e^{lτ} dτ = (e^{ls} − 1) / l, and s as l goes to 0
        let phi = |l: f64| if (l * s).abs() < 1e-8 { s * (1.0 + 0.5 * l * s) } else { (l * s).exp_m1() / l };
        if delta > 1e-18 {
            let w = delta.sqrt();
            let (l1, l2) = (alpha + w, alpha - w);
            scale(add(scale(sub(a, l2), phi(l1)), scale(sub(a, l1), -phi(l2))), 1.0 / (l1 - l2))
        } else if delta < -1e-18 {
            // ∫ e^{ατ} cos(wτ) and ∫ e^{ατ} sin(wτ) / w; here α² + w² = det > 0
            let w = (-delta).sqrt();
            let d = alpha * alpha + w * w;
            let e = (alpha * s).exp();
            let (c, sn) = ((w * s).cos(), (w * s).sin());
            let (ci, si) = if d * s * s < 1e-12 { (s, 0.5 * s * s) } else { ((e * (alpha * c + w * sn) - alpha) / d, (e * (alpha * sn - w * c) + w) / (d * w)) };
            let id = [[ci, 0.0], [0.0, ci]];
            add(id, scale(sub(a, alpha), si))
        } else {
            // repeated eigenvalue α: ∫ e^{ατ} and ∫ τ e^{ατ}
            let x = alpha * s;
            let e1 = if x.abs() < 1e-4 { 0.5 * s * s * (1.0 + 2.0 * x / 3.0) } else { (x.exp() * (x - 1.0) + 1.0) / (alpha * alpha) };
            let e0 = phi(alpha);
            add([[e0, 0.0], [0.0, e0]], scale(sub(a, alpha), e1))
        }
    }
    pub fn at_i(&self, i: usize, t: SimTime) -> f64 { self.state_at(t)[i] }
    /// Equilibrium `−A⁻¹ u`, if A is invertible.
    pub fn equilibrium(&self) -> Option<[f64; 2]> {
        let det = Self::det_if_invertible(self.a)?;
        let inv = [[self.a[1][1] / det, -self.a[0][1] / det], [-self.a[1][0] / det, self.a[0][0] / det]];
        let v = Self::mul(inv, self.u);
        Some([-v[0], -v[1]])
    }
    pub fn reanchor(&self, t: SimTime, a: [[f64; 2]; 2], u: [f64; 2]) -> Expm2 { Expm2 { a, u, x0: self.state_at(t), t0: t } }
    /// First time component `i` is ≥ `x` (`up`) or ≤ `x` (`!up`) within `horizon` (scan + bisect).
    pub fn reach_of(&self, i: usize, x: f64, up: bool, horizon: Span) -> Reach {
        let sign = if up { 1.0 } else { -1.0 };
        let f = |s: f64| sign * (self.at_i(i, self.t0 + Span::from_secs_f64(s)) - x);
        if f(0.0) >= 0.0 { return Reach::Already; }
        let eq = self.equilibrium().map(|e| e[i]);
        match first_crossing_scan(f, horizon.as_secs_f64(), 256) {
            Some(s) => Reach::At(self.t0 + Span::from_secs_f64(s)),
            None => Reach::Never { x_inf: eq.unwrap_or(f64::NAN) },
        }
    }
    /// The soonest time, after `t0` and before `t0 + horizon`, at which any of `asks` is reached; each ask is
    /// (component, threshold, up) as in `reach_of`. The answer is the one `reach_of` gives when asked one by one,
    /// to the nanosecond, but the trajectory is sampled once for all the asks instead of once per ask, and only
    /// the asks that cross in the earliest interval are bisected: nothing later can come sooner.
    pub fn first_reach(&self, asks: &[(usize, f64, bool)], horizon: Span) -> Option<SimTime> {
        const N: u32 = 256; // the scan `reach_of` does
        let step = horizon.as_secs_f64() / N as f64;
        let samples: Vec<[f64; 2]> = (0..=N).map(|k| self.state_at(self.t0 + Span::from_secs_f64(step * k as f64))).collect();
        // for each ask that is not already met: the first interval (k−1, k] in which it changes sign
        let mut crossings: Vec<(u32, usize)> = Vec::new();
        for (ai, &(i, x, up)) in asks.iter().enumerate() {
            let sign = if up { 1.0 } else { -1.0 };
            let f = |k: u32| sign * (samples[k as usize][i] - x);
            let mut fprev = f(0);
            if fprev >= 0.0 { continue; }
            for k in 1..=N {
                let fs = f(k);
                if (fs < 0.0) != (fprev < 0.0) || fs == 0.0 { crossings.push((k, ai)); break; }
                fprev = fs;
            }
        }
        crossings.sort_unstable();
        let end = self.t0 + horizon;
        let mut best: Option<SimTime> = None;
        let mut interval = 0;
        for (k, ai) in crossings {
            if best.is_some() && k != interval { break; }
            interval = k;
            let (i, x, up) = asks[ai];
            let sign = if up { 1.0 } else { -1.0 };
            let f = |s: f64| sign * (self.at_i(i, self.t0 + Span::from_secs_f64(s)) - x);
            let t = self.t0 + Span::from_secs_f64(bisect(f, step * (k - 1) as f64, step * k as f64, 80));
            if t > self.t0 && t < end && best.is_none_or(|b| t < b) { best = Some(t); }
        }
        best
    }
}

/// A timeline: value interpolated between (offset seconds, value) points; flat outside.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PiecewiseLinear { pub t0: SimTime, pub pts: Vec<(f64, f64)> }
impl PiecewiseLinear {
    fn reach(&self, x: f64, up: bool) -> Reach {
        if self.pts.is_empty() { return Reach::Never { x_inf: 0.0 }; }
        let ok = |v: f64| if up { v >= x } else { v <= x };
        if ok(self.pts[0].1) { return Reach::Already; }
        for w in self.pts.windows(2) {
            let ((s0, a), (s1, b)) = (w[0], w[1]);
            if ok(b) {
                let s = if a == b { s1 } else { s0 + (x - a) / (b - a) * (s1 - s0) };
                return Reach::At(self.t0 + Span::from_secs_f64(s.max(0.0)));
            }
        }
        Reach::Never { x_inf: self.pts[self.pts.len() - 1].1 }
    }
}
impl Trajectory for PiecewiseLinear {
    fn at(&self, t: SimTime) -> f64 {
        if self.pts.is_empty() { return 0.0; }
        // saturating: `crossing` asks for the value at the beginning of time, which is before any t0
        let s = Span(t.0.saturating_sub(self.t0.0)).as_secs_f64();
        if s <= self.pts[0].0 { return self.pts[0].1; }
        for w in self.pts.windows(2) {
            let ((s0, v0), (s1, v1)) = (w[0], w[1]);
            if s <= s1 { return if s1 == s0 { v1 } else { v0 + (v1 - v0) * (s - s0) / (s1 - s0) }; }
        }
        self.pts[self.pts.len() - 1].1
    }
    fn limit(&self) -> f64 { self.pts.last().map(|p| p.1).unwrap_or(0.0) }
    fn reach_up(&self, x: f64) -> Reach { self.reach(x, true) }
    fn reach_down(&self, x: f64) -> Reach { self.reach(x, false) }
}

/// Bisection on a bracket [lo, hi] with f(lo)·f(hi) ≤ 0. 80 iterations resolve any span to the ns.
pub fn bisect(f: impl Fn(f64) -> f64, mut lo: f64, mut hi: f64, iters: u32) -> f64 {
    let mut flo = f(lo);
    if flo == 0.0 { return lo; }
    for _ in 0..iters {
        let mid = 0.5 * (lo + hi);
        let fm = f(mid);
        if fm == 0.0 { return mid; }
        if (fm < 0.0) == (flo < 0.0) { lo = mid; flo = fm; } else { hi = mid; }
        if hi - lo < 1e-9 { break; }
    }
    0.5 * (lo + hi)
}

/// Scan `f` on [0, horizon] in `n` intervals for the first sign change, then bisect.
/// Returns the crossing offset in seconds, `Some(0.0)` if exactly at zero, `None` if none found.
/// Callers defining a directional threshold must handle an already-passed threshold themselves.
pub fn first_crossing_scan(f: impl Fn(f64) -> f64, horizon: f64, n: u32) -> Option<f64> {
    let f0 = f(0.0);
    if f0 == 0.0 { return Some(0.0); }
    let step = horizon / n as f64;
    let mut prev = 0.0;
    let mut fprev = f0;
    for k in 1..=n {
        let s = step * k as f64;
        let fs = f(s);
        if (fs < 0.0) != (fprev < 0.0) || fs == 0.0 {
            return Some(bisect(&f, prev, s, 80));
        }
        prev = s;
        fprev = fs;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    fn t(s: f64) -> SimTime { SimTime::ZERO + Span::from_secs_f64(s) }
    #[test]
    fn relax_crossing_is_exact_inverse() {
        let r = Relax::new(100.0, 25.0, SimTime::ZERO, Span::mins(10));
        match r.reach_down(60.0) {
            Reach::At(tc) => assert!((r.at(tc) - 60.0).abs() < 1e-9, "{}", r.at(tc)),
            other => panic!("{other:?}"),
        }
        assert_eq!(r.crossing(60.0), r.reach_down(60.0));
        assert_eq!(r.reach_down(20.0), Reach::Never { x_inf: 25.0 });
        assert_eq!(r.reach_down(25.0), Reach::Never { x_inf: 25.0 });
        assert_eq!(r.reach_down(100.0), Reach::Already);
        assert_eq!(r.reach_down(120.0), Reach::Already);
        assert_eq!(r.reach_up(120.0), Reach::Never { x_inf: 25.0 }, "falling: never reaches above");
        assert_eq!(r.reach_up(90.0), Reach::Already, "falling from 100: already ≥ 90");
        let up = Relax::new(0.0, 1.0, SimTime::ZERO, Span::hours(1));
        assert!(matches!(up.reach_up(0.5), Reach::At(_)));
        assert!(matches!(up.reach_up(1.5), Reach::Never { .. }));
        assert_eq!(up.reach_up(0.0), Reach::Already);
        let re = r.reanchor(t(300.0), 90.0, Span::mins(5));
        assert!((re.x0 - r.at(t(300.0))).abs() < 1e-12);
    }
    #[test]
    fn linear_and_logistic() {
        let l = Linear::new(50.0, 1.0 / 60.0, SimTime::ZERO);
        assert_eq!(l.reach_up(110.0), Reach::At(t(3600.0)));
        assert_eq!(l.reach_up(10.0), Reach::Already);
        assert_eq!(l.reach_down(10.0), Reach::Never { x_inf: f64::INFINITY });
        assert_eq!(Linear::new(1.0, 0.0, SimTime::ZERO).reach_up(2.0), Reach::Never { x_inf: 1.0 });
        let g = Logistic { x0: 1.0, cap: 100.0, r: 1e-4, t0: SimTime::ZERO };
        if let Reach::At(tc) = g.reach_up(50.0) { assert!((g.at(tc) - 50.0).abs() < 1e-6) } else { panic!() }
        assert!(matches!(g.reach_up(100.0), Reach::Never { .. }));
        assert_eq!(g.crossing(50.0), g.reach_up(50.0));
    }
    #[test]
    fn bateman_peak_and_crossings() {
        let b = Bateman { dose: 95.0, f: 1.0, ka: 2.0 / 3600.0, ke: 0.14 / 3600.0, vd: 40.0, t0: SimTime::ZERO };
        let cmax = b.c_max();
        assert!(cmax > 1.5 && cmax < 2.5, "{cmax}");
        if let Reach::At(tc) = b.reach_up(1.0) { assert!((b.at(tc) - 1.0).abs() < 1e-6) } else { panic!() }
        if let Reach::At(tc) = b.crossing_down(1.0) { assert!((b.at(tc) - 1.0).abs() < 1e-6); assert!(tc > t(b.t_peak())) } else { panic!() }
    }
    #[test]
    fn expm2_matches_scalar_relax() {
        // decoupled: two independent relaxations
        let e = Expm2 { a: [[-1.0 / 600.0, 0.0], [0.0, -1.0 / 60.0]], u: [25.0 / 600.0, 25.0 / 60.0], x0: [100.0, 100.0], t0: SimTime::ZERO };
        let r = Relax::new(100.0, 25.0, SimTime::ZERO, Span::secs(600));
        let s = e.state_at(t(900.0));
        assert!((s[0] - r.at(t(900.0))).abs() < 1e-9);
        // coupled cup+hand
        let c = Expm2 { a: [[-0.02, 0.02], [0.05, -0.05]], u: [0.0, 0.0], x0: [80.0, 30.0], t0: SimTime::ZERO };
        let eq = c.equilibrium();
        assert!(eq.is_none() || eq.unwrap()[0].is_finite());
        let late = c.state_at(t(10_000.0));
        assert!((late[0] - late[1]).abs() < 1e-3);
        if let Reach::At(tc) = c.reach_of(0, 70.0, false, Span::hours(1)) { assert!((c.at_i(0, tc) - 70.0).abs() < 1e-6) } else { panic!() }
        assert!(matches!(c.reach_of(0, 60.0, false, Span::hours(1)), Reach::Never { .. }), "80→65.7 never reaches 60");
        assert_eq!(c.reach_of(1, 25.0, true, Span::hours(1)), Reach::Already);
    }

    /// dx/dt = A x + u stepped with RK4: slow, and the yardstick for the closed forms.
    fn rk4(a: [[f64; 2]; 2], u: [f64; 2], x0: [f64; 2], s: f64) -> [f64; 2] {
        let n = 200_000;
        let h = s / n as f64;
        let f = |x: [f64; 2]| [a[0][0] * x[0] + a[0][1] * x[1] + u[0], a[1][0] * x[0] + a[1][1] * x[1] + u[1]];
        let step = |x: [f64; 2], k: [f64; 2], c: f64| [x[0] + c * k[0], x[1] + c * k[1]];
        let mut x = x0;
        for _ in 0..n {
            let k1 = f(x);
            let k2 = f(step(x, k1, h / 2.0));
            let k3 = f(step(x, k2, h / 2.0));
            let k4 = f(step(x, k3, h));
            x = [x[0] + h / 6.0 * (k1[0] + 2.0 * k2[0] + 2.0 * k3[0] + k4[0]), x[1] + h / 6.0 * (k1[1] + 2.0 * k2[1] + 2.0 * k3[1] + k4[1])];
        }
        x
    }

    #[test]
    fn expm2_is_right_when_the_matrix_cannot_be_inverted() {
        // A cup and a hand that only exchange heat with each other (nothing leaves, so det A = 0), and a heater on
        // the cup. The forced part used to be a two-term series, right only while s is tiny.
        type Case = ([[f64; 2]; 2], [f64; 2], [f64; 2], f64); // A, u, x0, seconds
        let cases: [Case; 6] = [
            ([[-0.02, 0.02], [0.05, -0.05]], [0.1, 0.0], [80.0, 30.0], 1000.0),  // singular, forced
            ([[-0.02, 0.02], [0.05, -0.05]], [0.0, -0.03], [80.0, 30.0], 40.0),
            ([[0.0, 0.0], [0.0, 0.0]], [0.5, -0.25], [1.0, 2.0], 300.0),           // nothing but the forcing
            ([[0.0, 1.0], [0.0, 0.0]], [0.0, 0.001], [0.0, 0.0], 100.0),           // nilpotent: a double integrator
            ([[-0.01, 0.0], [0.0, 0.0]], [0.2, 0.3], [5.0, 5.0], 500.0),           // one store leaks, one only fills
            ([[-0.02, 0.02], [0.05, -0.05 - 1e-13]], [0.1, 0.0], [80.0, 30.0], 1000.0), // an inverse exists, and is useless
        ];
        for (a, u, x0, s) in cases {
            let got = Expm2 { a, u, x0, t0: SimTime::ZERO }.state_at(t(s));
            let want = rk4(a, u, x0, s);
            for i in 0..2 {
                assert!((got[i] - want[i]).abs() <= 1e-6 * want[i].abs().max(1.0), "A = {a:?}, u = {u:?}, s = {s}: got {got:?}, want {want:?}");
            }
        }
        assert_eq!(Expm2 { a: [[-0.02, 0.02], [0.05, -0.05 - 1e-13]], u: [0.1, 0.0], x0: [0.0, 0.0], t0: SimTime::ZERO }.equilibrium(), None,
                   "no equilibrium is better than one made of rounding error");
    }

    #[test]
    fn the_integral_agrees_with_the_inverse_where_both_work() {
        // real and distinct, complex, and repeated eigenvalues
        for a in [[[-0.004, 0.003], [0.0005, -0.0005]], [[-0.01, 0.05], [-0.05, -0.01]], [[-0.02, 0.0], [0.0, -0.02]], [[-0.02, 0.01], [0.0, -0.02]]] {
            for s in [0.001, 1.0, 60.0, 3600.0] {
                let (u, zero) = ([0.7, -0.2], [0.0, 0.0]);
                let by_inverse = Expm2 { a, u, x0: zero, t0: SimTime::ZERO }.state_at(t(s));
                let by_integral = Expm2::mul(Expm2::integral(a, s), u);
                for i in 0..2 {
                    assert!((by_inverse[i] - by_integral[i]).abs() <= 1e-9 * by_inverse[i].abs().max(1e-6), "A = {a:?}, s = {s}: {by_inverse:?} vs {by_integral:?}");
                }
            }
        }
    }

    #[test]
    fn first_reach_is_reach_of_asked_one_by_one() {
        // a small generator, so the test needs no dependency and draws the same cases every run
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut draw = |lo: f64, hi: f64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            lo + (hi - lo) * ((seed >> 11) as f64 / (1u64 << 53) as f64)
        };
        let horizon = Span::mins(30);
        let (mut found, mut none) = (0, 0);
        for case in 0..400 {
            // two stores that exchange and leak, pushed from outside: the shape of the thermal model
            let (k, leak, c0, c1) = (draw(0.0, 0.01), draw(0.0, 0.01), draw(0.2, 5.0), draw(0.2, 5.0));
            let a = [[-(k + leak) * c0, k * c0], [k * c1, -k * c1]];
            let seg = Expm2 { a, u: [draw(-0.02, 0.02), draw(-0.02, 0.02)], x0: [draw(30.0, 36.0), draw(36.0, 38.0)], t0: SimTime(draw(0.0, 1e15) as i64) };
            // every fifth case: thresholds too far away to reach inside the horizon
            let far = if case % 5 == 0 { 1000.0 } else { 0.0 };
            let asks: Vec<(usize, f64, bool)> = (0..16).map(|n| { let up = n % 4 < 2; (n % 2, seg.x0[n % 2] + draw(-0.5, 0.5) + if up { far } else { -far }, up) }).collect();
            let mut want: Option<SimTime> = None;
            for &(i, x, up) in &asks {
                if let Reach::At(tc) = seg.reach_of(i, x, up, horizon) {
                    if tc > seg.t0 && tc < seg.t0 + horizon && want.is_none_or(|w| tc < w) { want = Some(tc); }
                }
            }
            assert_eq!(seg.first_reach(&asks, horizon), want, "{seg:?} {asks:?}");
            if want.is_some() { found += 1 } else { none += 1 }
        }
        assert!(found > 50 && none > 5, "the cases must cover both answers: {found} with a crossing, {none} without");
    }

    #[test]
    fn a_timeline_knows_which_way_it_is_going() {
        // `crossing` reads the start value at the beginning of time; with t0 after zero that subtraction overflowed
        let line = PiecewiseLinear { t0: t(3600.0), pts: vec![(0.0, 10.0), (60.0, 4.0)] };
        assert_eq!(line.crossing(7.0), Reach::At(t(3630.0)));
        assert_eq!(line.at(SimTime(i64::MIN)), 10.0);
    }
}
