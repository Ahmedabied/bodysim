//! -14: no unseeded draw anywhere; every (system, entity, event) gets its own keyed
//! ChaCha8 stream; a new draw in one handler never shifts another's. Distributions are hand-rolled
//! (uniform, normal, exponential, weibull, bernoulli) so plugins need no `rand`.
use crate::id::{EntityId, EventId, SystemId};
use rand_chacha::rand_core::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

pub fn stream(seed: u64, system: SystemId, entity: EntityId, event: EventId) -> ChaCha8Rng {
    let mut h = blake3::Hasher::new();
    h.update(b"bodysim-stream");
    h.update(&seed.to_le_bytes());
    h.update(&system.0.to_le_bytes());
    h.update(&entity.index.to_le_bytes());
    h.update(&entity.gen.to_le_bytes());
    h.update(&event.0.to_le_bytes());
    ChaCha8Rng::from_seed(*h.finalize().as_bytes())
}

/// A keyed stream for a named purpose (content generation, tests).
pub fn stream_named(seed: u64, purpose: &str, entity: EntityId) -> ChaCha8Rng {
    let mut h = blake3::Hasher::new();
    h.update(b"bodysim-named");
    h.update(&seed.to_le_bytes());
    h.update(purpose.as_bytes());
    h.update(&entity.index.to_le_bytes());
    h.update(&entity.gen.to_le_bytes());
    ChaCha8Rng::from_seed(*h.finalize().as_bytes())
}

/// Uniform in [0, 1).
pub fn uniform(rng: &mut impl Rng) -> f64 { (rng.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64) }
pub fn uniform_range(rng: &mut impl Rng, lo: f64, hi: f64) -> f64 { lo + (hi - lo) * uniform(rng) }
/// Standard normal, Box-Muller.
pub fn normal(rng: &mut impl Rng) -> f64 {
    let u1 = uniform(rng).max(1e-300);
    let u2 = uniform(rng);
    (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
}
pub fn normal_with(rng: &mut impl Rng, mean: f64, sd: f64) -> f64 { mean + sd * normal(rng) }
/// Exponential with the given rate (per unit); returns a waiting time.
pub fn exponential(rng: &mut impl Rng, rate: f64) -> f64 { -uniform(rng).max(1e-300).ln() / rate }
/// Weibull(shape k, scale λ): the failure-time draw (structure/appliance service life).
pub fn weibull(rng: &mut impl Rng, shape: f64, scale: f64) -> f64 { scale * (-uniform(rng).max(1e-300).ln()).powf(1.0 / shape) }
pub fn bernoulli(rng: &mut impl Rng, p: f64) -> bool { uniform(rng) < p }
/// Pick an index with the given weights.
pub fn pick_weighted(rng: &mut impl Rng, weights: &[f64]) -> usize {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 { return 0; }
    let mut u = uniform(rng) * total;
    for (i, w) in weights.iter().enumerate() {
        if u < *w { return i; }
        u -= w;
    }
    weights.len() - 1
}
