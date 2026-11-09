//! Rules bind to component SIGNATURES and dispatch only on touched fields.
//! Dynamics are code (systems). Constraints are data (`DeclRule`, RON) + typed `Rule` impls.
use crate::component::{AnyComponent, ComponentKind, Signature};
use crate::id::{EntityId, FieldId, SystemId};
use crate::value::Value;
use crate::world::{Kinds, World};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq)]
pub struct FieldChange { pub field: FieldId, pub old: Value, pub new: Value }

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum RuleOp { Set, Insert, Remove, Spawn, Despawn }

/// What a rule sees: the proposed change, before it is applied.
pub struct RuleView<'a> {
    pub entity: EntityId,
    pub op: RuleOp,
    pub component: Option<ComponentKind>,
    pub changed: &'a [FieldChange],
    pub new: Option<&'a dyn AnyComponent>,
    pub sig_before: Signature,
    pub sig_after: Signature,
    pub by: SystemId,
}

pub trait Rule: Send + Sync {
    fn name(&self) -> &str;
    /// bound to a COMPONENT SIGNATURE, never a type..
    fn signature(&self) -> Signature;
    /// dispatch only on touched fields.. Empty ⇒ structural rule (runs on insert/remove/spawn/despawn).
    fn touches(&self) -> &[(ComponentKind, FieldId)];
    /// Err(reason) rejects the proposal; the engine fills the rest of the `Violation`.
    fn check(&self, w: &World, v: &RuleView) -> Result<(), String>;
    /// rules carry a version; the log records the set.
    fn version(&self) -> u16 { 1 }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir { Up, Down }

/// Declarative rules - data, loadable from `content/rules/*.ron`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum DeclRule {
    Range { kind: String, field: String, min: f64, max: f64 },
    /// the field may only move in `dir` (a ratchet by design - must be bounded elsewhere)
    Monotone { kind: String, field: String, dir: Dir },
    /// an entity with `kind` must also have `other`
    Requires { kind: String, other: String },
    /// an entity never has both
    Excludes { kind: String, other: String },
}

impl DeclRule {
    pub fn name(&self) -> String {
        match self {
            DeclRule::Range { kind, field, min, max } => format!("range:{kind}.{field}∈[{min},{max}]"),
            DeclRule::Monotone { kind, field, dir } => format!("monotone:{kind}.{field}:{dir:?}"),
            DeclRule::Requires { kind, other } => format!("requires:{kind}→{other}"),
            DeclRule::Excludes { kind, other } => format!("excludes:{kind}×{other}"),
        }
    }
    pub fn kind(&self) -> &str {
        match self { DeclRule::Range { kind, .. } | DeclRule::Monotone { kind, .. } | DeclRule::Requires { kind, .. } | DeclRule::Excludes { kind, .. } => kind }
    }
    pub fn resolve(&self, kinds: &Kinds) -> Result<ResolvedDecl, String> {
        let kind = kinds.component(self.kind()).ok_or_else(|| format!("rule {}: unknown component {}", self.name(), self.kind()))?;
        let (field, other) = match self {
            DeclRule::Range { field, .. } | DeclRule::Monotone { field, .. } => {
                (Some(kinds.field(kind, field).ok_or_else(|| format!("rule {}: unknown field {}.{}", self.name(), self.kind(), field))?), None)
            }
            DeclRule::Requires { other, .. } | DeclRule::Excludes { other, .. } => {
                (None, Some(kinds.component(other).ok_or_else(|| format!("rule {}: unknown component {other}", self.name()))?))
            }
        };
        let sig = Signature::from_kinds([kind]);
        let touches = field.map(|f| vec![(kind, f)]).unwrap_or_default();
        Ok(ResolvedDecl { name: self.name(), decl: self.clone(), sig, touches, kind, field, other })
    }
}

pub struct ResolvedDecl {
    name: String,
    decl: DeclRule,
    sig: Signature,
    touches: Vec<(ComponentKind, FieldId)>,
    kind: ComponentKind,
    field: Option<FieldId>,
    other: Option<ComponentKind>,
}

impl Rule for ResolvedDecl {
    fn name(&self) -> &str { &self.name }
    fn signature(&self) -> Signature { self.sig }
    fn touches(&self) -> &[(ComponentKind, FieldId)] { &self.touches }
    fn check(&self, w: &World, v: &RuleView) -> Result<(), String> {
        match &self.decl {
            DeclRule::Range { min, max, .. } => {
                if v.component != Some(self.kind) { return Ok(()); }
                for ch in v.changed.iter().filter(|c| Some(c.field) == self.field) {
                    if let Some(x) = ch.new.scalar_at(w.now) {
                        if x < *min || x > *max {
                            return Err(format!("{x:.6} outside [{min}, {max}]"));
                        }
                    }
                }
                Ok(())
            }
            DeclRule::Monotone { dir, .. } => {
                if v.component != Some(self.kind) || v.op != RuleOp::Set { return Ok(()); }
                for ch in v.changed.iter().filter(|c| Some(c.field) == self.field) {
                    let (Some(o), Some(n)) = (ch.old.scalar_at(w.now), ch.new.scalar_at(w.now)) else { continue };
                    let asym = ch.new.asymptote().unwrap_or(n);
                    let bad = match dir { Dir::Up => n < o || asym < n, Dir::Down => n > o || asym > n };
                    if bad { return Err(format!("must move {dir:?}: {o:.6} → {n:.6} (→{asym:.6})")); }
                }
                Ok(())
            }
            DeclRule::Requires { other, .. } => {
                let o = self.other.expect("resolved");
                if v.sig_after.contains(self.kind) && !v.sig_after.contains(o) {
                    return Err(format!("{} requires {}", w.kinds.component_name(self.kind), other));
                }
                Ok(())
            }
            DeclRule::Excludes { other, .. } => {
                let o = self.other.expect("resolved");
                if v.sig_after.contains(self.kind) && v.sig_after.contains(o) {
                    return Err(format!("{} excludes {}", w.kinds.component_name(self.kind), other));
                }
                Ok(())
            }
        }
    }
}
