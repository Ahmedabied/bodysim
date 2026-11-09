//! Content loading: protos and scenes are parsed ONCE, typed, straight from RON text, with the
//! component kind dispatched by name at parse time (a `DeserializeSeed` per kind). No lossy
//! intermediate: unit variants keep their names, `implicit_some` and `unwrap_newtypes` apply.
use crate::component::{AnyComponent, AnyStore};
use crate::world::Kinds;
use serde::de::{DeserializeSeed, Deserializer, Error as DeError, MapAccess, SeqAccess, Visitor};
use serde::Deserialize;
use std::fmt;

#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct EdgeSpec { pub kind: String, pub dst: String }

#[derive(Clone, Debug)]
pub struct LoadedProto { pub name: String, pub components: Vec<Box<dyn AnyComponent>> }

#[derive(Clone, Debug)]
pub struct LoadedEntity { pub name: String, pub proto: Option<String>, pub components: Vec<Box<dyn AnyComponent>>, pub edges: Vec<EdgeSpec> }

#[derive(Clone, Debug, Default)]
pub struct LoadedScene { pub entities: Vec<LoadedEntity> }

/// Content RON options: `proto: "cup"` not `Some("cup")`; `t0: 0` not `(0)`.
pub fn options() -> ron::Options {
    use ron::extensions::Extensions;
    ron::Options::default().with_default_extension(Extensions::IMPLICIT_SOME | Extensions::UNWRAP_NEWTYPES)
}

pub fn from_ron<T: serde::de::DeserializeOwned>(text: &str) -> Result<T, String> {
    options().from_str(text).map_err(|e| e.to_string())
}

fn with_seed<'a, T>(text: &'a str, seed: impl DeserializeSeed<'a, Value = T>) -> Result<T, String>
where
    T: 'a,
{
    let opts = options();
    let mut de = ron::de::Deserializer::from_str_with_options(text, &opts).map_err(|e| e.to_string())?;
    let v = seed.deserialize(&mut de).map_err(|e| e.to_string())?;
    de.end().map_err(|e| e.to_string())?;
    Ok(v)
}

pub fn parse_protos(text: &str, stores: &[Box<dyn AnyStore>], kinds: &Kinds) -> Result<Vec<LoadedProto>, String> {
    with_seed(text, ProtosSeed { stores, kinds })
}

pub fn parse_scene(text: &str, stores: &[Box<dyn AnyStore>], kinds: &Kinds) -> Result<LoadedScene, String> {
    with_seed(text, SceneSeed { stores, kinds })
}

struct Cx<'a> { stores: &'a [Box<dyn AnyStore>], kinds: &'a Kinds }

// ---- one component, dispatched by kind name ----
struct ComponentSeed<'a> { store: &'a dyn AnyStore }
impl<'de, 'a> DeserializeSeed<'de> for ComponentSeed<'a> {
    type Value = Box<dyn AnyComponent>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        let mut erased = <dyn erased_serde::Deserializer>::erase(d);
        self.store.boxed_from_erased(&mut erased).map_err(D::Error::custom)
    }
}

// ---- the `components: { "kind": (...), ... }` map ----
struct ComponentsSeed<'a> { cx: Cx<'a> }
impl<'de, 'a> DeserializeSeed<'de> for ComponentsSeed<'a> {
    type Value = Vec<Box<dyn AnyComponent>>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> { d.deserialize_map(self) }
}
impl<'de, 'a> Visitor<'de> for ComponentsSeed<'a> {
    type Value = Vec<Box<dyn AnyComponent>>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("a map of component kind name → component") }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut out = Vec::new();
        while let Some(kind) = map.next_key::<String>()? {
            let k = self.cx.kinds.component(&kind).ok_or_else(|| M::Error::custom(format!("unknown component kind `{kind}`")))?;
            let store = &*self.cx.stores[k.0 as usize];
            out.push(map.next_value_seed(ComponentSeed { store })?);
        }
        Ok(out)
    }
}

// ---- protos: [ (name: "cup", components: {...}), ... ] ----
struct ProtosSeed<'a> { stores: &'a [Box<dyn AnyStore>], kinds: &'a Kinds }
impl<'de, 'a> DeserializeSeed<'de> for ProtosSeed<'a> {
    type Value = Vec<LoadedProto>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> { d.deserialize_seq(self) }
}
impl<'de, 'a> Visitor<'de> for ProtosSeed<'a> {
    type Value = Vec<LoadedProto>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("a list of protos") }
    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
        let mut out = Vec::new();
        while let Some(p) = seq.next_element_seed(ProtoSeed { cx: Cx { stores: self.stores, kinds: self.kinds } })? { out.push(p); }
        Ok(out)
    }
}
struct ProtoSeed<'a> { cx: Cx<'a> }
impl<'de, 'a> DeserializeSeed<'de> for ProtoSeed<'a> {
    type Value = LoadedProto;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> { d.deserialize_struct("ProtoDecl", &["name", "components"], self) }
}
impl<'de, 'a> Visitor<'de> for ProtoSeed<'a> {
    type Value = LoadedProto;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("(name: \"…\", components: {…})") }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let (mut name, mut components) = (None, Vec::new());
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" => name = Some(map.next_value::<String>()?),
                "components" => components = map.next_value_seed(ComponentsSeed { cx: Cx { stores: self.cx.stores, kinds: self.cx.kinds } })?,
                other => return Err(M::Error::unknown_field(other, &["name", "components"])),
            }
        }
        Ok(LoadedProto { name: name.ok_or_else(|| M::Error::missing_field("name"))?, components })
    }
}

// ---- scene: (entities: [ (name, proto?, components?, edges?), ... ]) ----
struct SceneSeed<'a> { stores: &'a [Box<dyn AnyStore>], kinds: &'a Kinds }
impl<'de, 'a> DeserializeSeed<'de> for SceneSeed<'a> {
    type Value = LoadedScene;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> { d.deserialize_struct("Scene", &["entities"], self) }
}
impl<'de, 'a> Visitor<'de> for SceneSeed<'a> {
    type Value = LoadedScene;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("(entities: [...])") }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut scene = LoadedScene::default();
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "entities" => scene.entities = map.next_value_seed(EntitiesSeed { cx: Cx { stores: self.stores, kinds: self.kinds } })?,
                other => return Err(M::Error::unknown_field(other, &["entities"])),
            }
        }
        Ok(scene)
    }
}
struct EntitiesSeed<'a> { cx: Cx<'a> }
impl<'de, 'a> DeserializeSeed<'de> for EntitiesSeed<'a> {
    type Value = Vec<LoadedEntity>;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> { d.deserialize_seq(self) }
}
impl<'de, 'a> Visitor<'de> for EntitiesSeed<'a> {
    type Value = Vec<LoadedEntity>;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("a list of entities") }
    fn visit_seq<S: SeqAccess<'de>>(self, mut seq: S) -> Result<Self::Value, S::Error> {
        let mut out = Vec::new();
        while let Some(e) = seq.next_element_seed(EntitySeed { cx: Cx { stores: self.cx.stores, kinds: self.cx.kinds } })? { out.push(e); }
        Ok(out)
    }
}
struct EntitySeed<'a> { cx: Cx<'a> }
impl<'de, 'a> DeserializeSeed<'de> for EntitySeed<'a> {
    type Value = LoadedEntity;
    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> { d.deserialize_struct("EntitySpec", &["name", "proto", "components", "edges"], self) }
}
impl<'de, 'a> Visitor<'de> for EntitySeed<'a> {
    type Value = LoadedEntity;
    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result { f.write_str("(name: \"…\", proto: \"…\", components: {…}, edges: [...])") }
    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut e = LoadedEntity { name: String::new(), proto: None, components: Vec::new(), edges: Vec::new() };
        let mut named = false;
        while let Some(key) = map.next_key::<String>()? {
            match key.as_str() {
                "name" => { e.name = map.next_value()?; named = true; }
                "proto" => e.proto = map.next_value::<Option<String>>()?,
                "components" => e.components = map.next_value_seed(ComponentsSeed { cx: Cx { stores: self.cx.stores, kinds: self.cx.kinds } })?,
                "edges" => e.edges = map.next_value::<Vec<EdgeSpec>>()?,
                other => return Err(M::Error::unknown_field(other, &["name", "proto", "components", "edges"])),
            }
        }
        if !named { return Err(M::Error::missing_field("name")); }
        Ok(e)
    }
}
