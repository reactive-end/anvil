//! Thin helpers over `yaml-rust2`: ordered mappings addressed by string keys.

pub use yaml_rust2::Yaml;
use yaml_rust2::{YamlEmitter, YamlLoader};

/// An insertion-ordered YAML mapping.
pub type Mapping = yaml_rust2::yaml::Hash;

/// Why a YAML block was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MappingError {
    Syntax(String),
    NotMapping,
}

/// Looks up `name` in `map`.
pub fn get<'a>(map: &'a Mapping, name: &str) -> Option<&'a Yaml> {
    map.get(&Yaml::String(name.to_owned()))
}

/// Mutable lookup of `name`.
pub fn get_mut<'a>(map: &'a mut Mapping, name: &str) -> Option<&'a mut Yaml> {
    map.get_mut(&Yaml::String(name.to_owned()))
}

/// Sets `name`; an existing key keeps its position, a new one goes last.
pub fn set(map: &mut Mapping, name: &str, value: Yaml) {
    map.replace(Yaml::String(name.to_owned()), value);
}

/// Removes `name`, keeping the order of the other keys.
pub fn remove(map: &mut Mapping, name: &str) {
    map.remove(&Yaml::String(name.to_owned()));
}

/// A string value.
pub fn string(text: impl Into<String>) -> Yaml {
    Yaml::String(text.into())
}

/// Parses a block that must be a mapping; an empty block is an empty mapping.
pub fn parse_mapping(text: &str) -> Result<Mapping, MappingError> {
    let docs =
        YamlLoader::load_from_str(text).map_err(|err| MappingError::Syntax(err.to_string()))?;
    match docs.into_iter().next() {
        None | Some(Yaml::Null) => Ok(Mapping::new()),
        Some(Yaml::Hash(map)) => Ok(map),
        Some(_) => Err(MappingError::NotMapping),
    }
}

/// Serialises a mapping as a YAML block without the leading document marker.
pub fn emit(map: &Mapping) -> String {
    if map.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    // Writing to a `String` cannot fail.
    let _ = YamlEmitter::new(&mut out).dump(&Yaml::Hash(map.clone()));
    let body = out.strip_prefix("---\n").unwrap_or(&out);
    format!("{}\n", body.trim_end_matches('\n'))
}
