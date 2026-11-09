//! `Value`: the typed carrier for a component field. Used by field diffs, rules-as-data,
//! predicates, violations and the log. Trajectories are values too (LAW: state is an anchor).
use crate::id::EntityId;
use crate::relax::{Linear, Relax, Trajectory};
use crate::time::{SimTime, Span};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Unit,
    F64(f64),
    I64(i64),
    Bool(bool),
    Time(SimTime),
    Span(Span),
    Relax(Relax),
    Linear(Linear),
    Enum(u16),
    Id(EntityId),
    OptId(Option<EntityId>),
    OptTime(Option<SimTime>),
    OptF64(Option<f64>),
    Text(String),
    /// postcard bytes of an opaque field (vectors, maps, nested structs)
    Bytes(Vec<u8>),
}

impl Value {
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Unit => "unit", Value::F64(_) => "f64", Value::I64(_) => "i64", Value::Bool(_) => "bool",
            Value::Time(_) => "time", Value::Span(_) => "span", Value::Relax(_) => "relax", Value::Linear(_) => "linear",
            Value::Enum(_) => "enum", Value::Id(_) => "id", Value::OptId(_) => "opt_id", Value::OptTime(_) => "opt_time",
            Value::OptF64(_) => "opt_f64", Value::Text(_) => "text", Value::Bytes(_) => "bytes",
        }
    }
    /// The scalar reading of this value at `now` (trajectories are evaluated).
    pub fn scalar_at(&self, now: SimTime) -> Option<f64> {
        match self {
            Value::F64(v) => Some(*v),
            Value::I64(v) => Some(*v as f64),
            Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            Value::Relax(r) => Some(r.at(now)),
            Value::Linear(l) => Some(l.at(now)),
            Value::OptF64(Some(v)) => Some(*v),
            Value::Enum(e) => Some(*e as f64),
            _ => None,
        }
    }
    /// Where this value is heading (its asymptote), if it is a trajectory; the value itself otherwise.
    pub fn asymptote(&self) -> Option<f64> {
        match self {
            Value::Relax(r) => Some(r.limit()),
            Value::Linear(l) => Some(l.limit()),
            other => other.scalar_at(SimTime::ZERO),
        }
    }
    pub fn is_finite(&self) -> bool {
        match self {
            Value::F64(v) => v.is_finite(),
            Value::OptF64(Some(v)) => v.is_finite(),
            Value::Relax(r) => r.x0.is_finite() && r.x_inf.is_finite(),
            Value::Linear(l) => l.x0.is_finite() && l.rate.is_finite(),
            _ => true,
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Unit => write!(f, "∅"),
            Value::F64(v) => write!(f, "{v:.4}"),
            Value::I64(v) => write!(f, "{v}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Time(t) => write!(f, "{t}"),
            Value::Span(s) => write!(f, "{s}"),
            Value::Relax(r) => write!(f, "relax({:.4}→{:.4} τ={} from {})", r.x0, r.x_inf, r.tau, r.t0),
            Value::Linear(l) => write!(f, "linear({:.4} {:+.6}/s from {})", l.x0, l.rate, l.t0),
            Value::Enum(e) => write!(f, "#{e}"),
            Value::Id(e) => write!(f, "{e}"),
            Value::OptId(e) => match e { Some(e) => write!(f, "{e}"), None => write!(f, "none") },
            Value::OptTime(t) => match t { Some(t) => write!(f, "{t}"), None => write!(f, "none") },
            Value::OptF64(v) => match v { Some(v) => write!(f, "{v:.4}"), None => write!(f, "none") },
            Value::Text(s) => write!(f, "{s:?}"),
            Value::Bytes(b) => write!(f, "<{} bytes>", b.len()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValueError { pub expected: &'static str, pub got: &'static str }
impl fmt::Display for ValueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "expected {}, got {}", self.expected, self.got) }
}
impl std::error::Error for ValueError {}

/// A field type that can be carried as a `Value`.
pub trait FieldValue: Sized {
    fn to_value(&self) -> Value;
    fn from_value(v: Value) -> Result<Self, ValueError>;
}

macro_rules! simple_fv {
    ($t:ty, $var:ident, $name:literal) => {
        impl FieldValue for $t {
            fn to_value(&self) -> Value { Value::$var(self.clone()) }
            fn from_value(v: Value) -> Result<Self, ValueError> {
                match v { Value::$var(x) => Ok(x), other => Err(ValueError { expected: $name, got: other.kind_name() }) }
            }
        }
    };
}
simple_fv!(f64, F64, "f64");
simple_fv!(i64, I64, "i64");
simple_fv!(bool, Bool, "bool");
simple_fv!(SimTime, Time, "time");
simple_fv!(Span, Span, "span");
simple_fv!(Relax, Relax, "relax");
simple_fv!(Linear, Linear, "linear");
simple_fv!(EntityId, Id, "id");
simple_fv!(Option<EntityId>, OptId, "opt_id");
simple_fv!(Option<SimTime>, OptTime, "opt_time");
simple_fv!(Option<f64>, OptF64, "opt_f64");
simple_fv!(String, Text, "text");

macro_rules! int_fv {
    ($t:ty) => {
        impl FieldValue for $t {
            fn to_value(&self) -> Value { Value::I64(*self as i64) }
            fn from_value(v: Value) -> Result<Self, ValueError> {
                match v {
                    Value::I64(x) => <$t>::try_from(x).map_err(|_| ValueError { expected: stringify!($t), got: "i64 out of range" }),
                    other => Err(ValueError { expected: stringify!($t), got: other.kind_name() }),
                }
            }
        }
    };
}
int_fv!(u8);
int_fv!(u16);
int_fv!(u32);
int_fv!(i32);
int_fv!(u64);

/// Trajectories without a single scalar reading travel as opaque bytes; plugins that hold one
/// write their own rule over it (rules see the typed component).
macro_rules! opaque_fv {
    ($t:ty, $name:literal) => {
        impl FieldValue for $t {
            fn to_value(&self) -> Value { Value::Bytes(postcard::to_stdvec(self).expect("postcard")) }
            fn from_value(v: Value) -> Result<Self, ValueError> {
                match v {
                    Value::Bytes(b) => postcard::from_bytes(&b).map_err(|_| ValueError { expected: $name, got: "undecodable bytes" }),
                    other => Err(ValueError { expected: $name, got: other.kind_name() }),
                }
            }
        }
    };
}
opaque_fv!(crate::relax::Expm2, "expm2");
opaque_fv!(crate::relax::Logistic, "logistic");
opaque_fv!(crate::relax::Bateman, "bateman");
opaque_fv!(crate::relax::PiecewiseLinear, "piecewise");

/// Opaque fields: vectors and maps travel as postcard bytes. Rules cannot range-check them.
impl<T: Serialize + DeserializeOwned> FieldValue for Vec<T> {
    fn to_value(&self) -> Value { Value::Bytes(postcard::to_stdvec(self).expect("postcard")) }
    fn from_value(v: Value) -> Result<Self, ValueError> {
        match v {
            Value::Bytes(b) => postcard::from_bytes(&b).map_err(|_| ValueError { expected: "bytes(vec)", got: "undecodable bytes" }),
            other => Err(ValueError { expected: "bytes(vec)", got: other.kind_name() }),
        }
    }
}
impl<K, V> FieldValue for indexmap::IndexMap<K, V>
where
    K: Serialize + DeserializeOwned + std::hash::Hash + Eq,
    V: Serialize + DeserializeOwned,
{
    fn to_value(&self) -> Value { Value::Bytes(postcard::to_stdvec(self).expect("postcard")) }
    fn from_value(v: Value) -> Result<Self, ValueError> {
        match v {
            Value::Bytes(b) => postcard::from_bytes(&b).map_err(|_| ValueError { expected: "bytes(map)", got: "undecodable bytes" }),
            other => Err(ValueError { expected: "bytes(map)", got: other.kind_name() }),
        }
    }
}

/// Declare a unit-variant enum usable as a component field (carried as `Value::Enum`).
///
/// ```ignore
/// value_enum! { pub enum Stage { Awake, Light, Deep, Rem } }
/// ```
#[macro_export]
macro_rules! value_enum {
    ($(#[$m:meta])* $vis:vis enum $name:ident { $($(#[$vm:meta])* $var:ident),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, ::serde::Serialize, ::serde::Deserialize)]
        #[repr(u16)]
        $vis enum $name { $($(#[$vm])* $var),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$var),+];
            pub fn name(self) -> &'static str { match self { $($name::$var => stringify!($var)),+ } }
            pub fn from_name(s: &str) -> Option<$name> { Self::ALL.iter().copied().find(|v| v.name() == s) }
        }
        impl $crate::value::FieldValue for $name {
            fn to_value(&self) -> $crate::value::Value { $crate::value::Value::Enum(*self as u16) }
            fn from_value(v: $crate::value::Value) -> Result<Self, $crate::value::ValueError> {
                match v {
                    $crate::value::Value::Enum(i) => Self::ALL.get(i as usize).copied()
                        .ok_or($crate::value::ValueError { expected: stringify!($name), got: "enum index out of range" }),
                    other => Err($crate::value::ValueError { expected: stringify!($name), got: other.kind_name() }),
                }
            }
        }
        impl ::std::fmt::Display for $name {
            fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result { f.write_str(self.name()) }
        }
    };
}
