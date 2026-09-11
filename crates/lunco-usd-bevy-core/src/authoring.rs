//! Authoring-layer readers shared by USD integrations.
//!
//! These helpers intentionally read the authored root layer rather than a
//! composed stage.  Composition-dependent runtime reads remain in
//! [`crate::read`] and [`crate::view`].

use lunco_usd_compose::parse_usda;
use lunco_usd_core::UsdDataExt;
use openusd::sdf::{Data, Path as SdfPath, Value};

/// Read the `defaultPrim` authored on a layer, without composition.
pub fn layer_default_prim(layer: &lunco_usd_core::UsdData) -> Option<String> {
    let name = layer.field(&SdfPath::abs_root(), "defaultPrim")?.as_str()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// A parsed USD layer positioned on its authored `defaultPrim`.
///
/// This is a composition-free reader for root-prim metadata and authored
/// attributes.  Referenced layers are intentionally not consulted.
pub struct DefaultPrim {
    data: Data,
    path: SdfPath,
}

impl DefaultPrim {
    /// Parse `text` and locate its `defaultPrim`.
    pub fn parse(text: &str) -> Option<Self> {
        let data = parse_usda(text).ok()?;
        let name = data
            .field(&SdfPath::abs_root(), "defaultPrim")?
            .as_str()?
            .to_string();
        if name.is_empty() {
            return None;
        }
        let path = SdfPath::new(&format!("/{name}")).ok()?;
        Some(Self { data, path })
    }

    /// The authored `defaultPrim` path, absolute in the source layer.
    pub fn path(&self) -> &SdfPath {
        &self.path
    }

    /// Raw default-time value of `attr`, as authored.
    pub fn value(&self, attr: &str) -> Option<&Value> {
        let attr_path = self.path.append_property(attr).ok()?;
        self.data.field(&attr_path, "default")
    }

    /// The prim's USD `doc` metadata.
    pub fn documentation(&self) -> Option<String> {
        self.data
            .field(&self.path, "documentation")
            .and_then(|value| value.as_str())
            .map(str::to_string)
    }

    /// Typed read using OpenUSD's `Value` conversion.
    pub fn scalar<T: TryFrom<Value>>(&self, attr: &str) -> Option<T> {
        self.value(attr).cloned()?.get::<T>()
    }

    /// Read a string-shaped authored attribute.
    pub fn text(&self, attr: &str) -> Option<String> {
        self.value(attr)?.as_str().map(str::to_string)
    }

    /// Read a real authored as either USD `float` or `double`.
    pub fn real_f32(&self, attr: &str) -> Option<f32> {
        self.scalar::<f32>(attr)
            .or_else(|| self.scalar::<f64>(attr).map(|value| value as f32))
    }
}
