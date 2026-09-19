//! Typed values crossing the in-process API boundary.
//!
//! `HookValue` is the owned, language-neutral representation used between
//! Rhai/native callers and the API runtime. JSON conversion is kept at explicit
//! transport edges; in-process reflection consumes the typed value directly.

use lunco_hooks::HookValue;
use serde::ser::{
    self, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant, SerializeTuple,
    SerializeTupleStruct, SerializeTupleVariant,
};
use std::fmt;

/// The typed value accepted by in-process command/query callers.
pub type ApiValue = HookValue;

/// Convert common Rust values into the typed API ABI.
pub trait IntoApiValue {
    /// Consume this value as an owned API value.
    fn into_api_value(self) -> ApiValue;
}

impl IntoApiValue for ApiValue {
    fn into_api_value(self) -> ApiValue {
        self
    }
}

impl IntoApiValue for () {
    fn into_api_value(self) -> ApiValue {
        ApiValue::Unit
    }
}

impl IntoApiValue for bool {
    fn into_api_value(self) -> ApiValue {
        ApiValue::Bool(self)
    }
}

impl IntoApiValue for String {
    fn into_api_value(self) -> ApiValue {
        ApiValue::Str(self)
    }
}

impl IntoApiValue for &str {
    fn into_api_value(self) -> ApiValue {
        ApiValue::str(self)
    }
}

impl IntoApiValue for &String {
    fn into_api_value(self) -> ApiValue {
        ApiValue::str(self.clone())
    }
}

macro_rules! signed_api_values {
    ($($ty:ty),* $(,)?) => {
        $(
            impl IntoApiValue for $ty {
                fn into_api_value(self) -> ApiValue {
                    ApiValue::Int(self.into())
                }
            }
        )*
    };
}

signed_api_values!(i8, i16, i32, i64);

impl IntoApiValue for isize {
    fn into_api_value(self) -> ApiValue {
        ApiValue::Int(self as i64)
    }
}

macro_rules! unsigned_api_values {
    ($($ty:ty),* $(,)?) => {
        $(
            impl IntoApiValue for $ty {
                fn into_api_value(self) -> ApiValue {
                    api_value_from_u64(self.into())
                }
            }
        )*
    };
}

unsigned_api_values!(u8, u16, u32, u64);

impl IntoApiValue for usize {
    fn into_api_value(self) -> ApiValue {
        api_value_from_u64(self as u64)
    }
}

impl IntoApiValue for f32 {
    fn into_api_value(self) -> ApiValue {
        ApiValue::Float(f64::from(self))
    }
}

impl IntoApiValue for f64 {
    fn into_api_value(self) -> ApiValue {
        ApiValue::Float(self)
    }
}

impl<T: IntoApiValue> IntoApiValue for Option<T> {
    fn into_api_value(self) -> ApiValue {
        self.map(IntoApiValue::into_api_value)
            .unwrap_or(ApiValue::Unit)
    }
}

impl<T: IntoApiValue> IntoApiValue for Vec<T> {
    fn into_api_value(self) -> ApiValue {
        ApiValue::Array(self.into_iter().map(IntoApiValue::into_api_value).collect())
    }
}

impl<T: IntoApiValue, const N: usize> IntoApiValue for [T; N] {
    fn into_api_value(self) -> ApiValue {
        ApiValue::Array(self.into_iter().map(IntoApiValue::into_api_value).collect())
    }
}

/// Construct a typed map without routing its fields through JSON.
#[macro_export]
macro_rules! api_map {
    ($($key:expr => $value:expr),* $(,)?) => {
        $crate::ApiValue::map([
            $(
                ($key, $crate::IntoApiValue::into_api_value($value)),
            )*
        ])
    };
}

/// Construct a typed array without routing its fields through JSON.
#[macro_export]
macro_rules! api_array {
    ($($value:expr),* $(,)?) => {
        $crate::ApiValue::Array(vec![
            $($crate::IntoApiValue::into_api_value($value),)*
        ])
    };
}

/// Construct a typed API value using concise map/array notation.
///
/// Unlike `serde_json::json!`, this macro produces [`ApiValue`] directly and
/// accepts ordinary Rust expressions for scalar leaves.
#[macro_export]
macro_rules! api_value {
    (null) => { $crate::ApiValue::Unit };
    (true) => { $crate::ApiValue::Bool(true) };
    (false) => { $crate::ApiValue::Bool(false) };
    ({ $($tokens:tt)* }) => { $crate::api_value!(@map [] $($tokens)*) };
    ([ $($tokens:tt)* ]) => { $crate::api_value!(@array [] $($tokens)*) };
    ($value:expr) => { $crate::IntoApiValue::into_api_value($value) };

    (@map []) => {
        $crate::ApiValue::Map(::std::vec::Vec::new())
    };
    (@map [$($entries:tt)*]) => {
        $crate::ApiValue::map([$($entries)*])
    };
    (@map [$($entries:tt)*] $key:literal : null, $($rest:tt)*) => {
        $crate::api_value!(@map [$($entries)* ($key, $crate::ApiValue::Unit),] $($rest)*)
    };
    (@map [$($entries:tt)*] $key:literal : true, $($rest:tt)*) => {
        $crate::api_value!(@map [$($entries)* ($key, $crate::ApiValue::Bool(true)),] $($rest)*)
    };
    (@map [$($entries:tt)*] $key:literal : false, $($rest:tt)*) => {
        $crate::api_value!(@map [$($entries)* ($key, $crate::ApiValue::Bool(false)),] $($rest)*)
    };
    (@map [$($entries:tt)*] $key:literal : { $($value:tt)* }, $($rest:tt)*) => {
        $crate::api_value!(@map [$($entries)* ($key, $crate::api_value!({ $($value)* })),] $($rest)*)
    };
    (@map [$($entries:tt)*] $key:literal : [ $($value:tt)* ], $($rest:tt)*) => {
        $crate::api_value!(@map [$($entries)* ($key, $crate::api_value!([ $($value)* ])),] $($rest)*)
    };
    (@map [$($entries:tt)*] $key:literal : $value:expr, $($rest:tt)*) => {
        $crate::api_value!(@map [$($entries)* ($key, $crate::IntoApiValue::into_api_value($value)),] $($rest)*)
    };
    (@map [$($entries:tt)*] $key:literal : null $(,)?) => {
        $crate::ApiValue::map([$($entries)* ($key, $crate::ApiValue::Unit),])
    };
    (@map [$($entries:tt)*] $key:literal : true $(,)?) => {
        $crate::ApiValue::map([$($entries)* ($key, $crate::ApiValue::Bool(true)),])
    };
    (@map [$($entries:tt)*] $key:literal : false $(,)?) => {
        $crate::ApiValue::map([$($entries)* ($key, $crate::ApiValue::Bool(false)),])
    };
    (@map [$($entries:tt)*] $key:literal : { $($value:tt)* } $(,)?) => {
        $crate::ApiValue::map([$($entries)* ($key, $crate::api_value!({ $($value)* })),])
    };
    (@map [$($entries:tt)*] $key:literal : [ $($value:tt)* ] $(,)?) => {
        $crate::ApiValue::map([$($entries)* ($key, $crate::api_value!([ $($value)* ])),])
    };
    (@map [$($entries:tt)*] $key:literal : $value:expr $(,)?) => {
        $crate::ApiValue::map([$($entries)* ($key, $crate::IntoApiValue::into_api_value($value)),])
    };

    (@array [$($values:expr,)*]) => {
        $crate::ApiValue::Array(vec![$($values,)*])
    };
    (@array [$($values:expr,)*] null, $($rest:tt)*) => {
        $crate::api_value!(@array [$($values,)* $crate::ApiValue::Unit,] $($rest)*)
    };
    (@array [$($values:expr,)*] true, $($rest:tt)*) => {
        $crate::api_value!(@array [$($values,)* $crate::ApiValue::Bool(true),] $($rest)*)
    };
    (@array [$($values:expr,)*] false, $($rest:tt)*) => {
        $crate::api_value!(@array [$($values,)* $crate::ApiValue::Bool(false),] $($rest)*)
    };
    (@array [$($values:expr,)*] { $($value:tt)* }, $($rest:tt)*) => {
        $crate::api_value!(@array [$($values,)* $crate::api_value!({ $($value)* }),] $($rest)*)
    };
    (@array [$($values:expr,)*] [ $($value:tt)* ], $($rest:tt)*) => {
        $crate::api_value!(@array [$($values,)* $crate::api_value!([ $($value)* ]),] $($rest)*)
    };
    (@array [$($values:expr,)*] $value:expr, $($rest:tt)*) => {
        $crate::api_value!(@array [$($values,)* $crate::IntoApiValue::into_api_value($value),] $($rest)*)
    };
    (@array [$($values:expr,)*] null $(,)?) => {
        $crate::ApiValue::Array(vec![$($values,)* $crate::ApiValue::Unit])
    };
    (@array [$($values:expr,)*] true $(,)?) => {
        $crate::ApiValue::Array(vec![$($values,)* $crate::ApiValue::Bool(true)])
    };
    (@array [$($values:expr,)*] false $(,)?) => {
        $crate::ApiValue::Array(vec![$($values,)* $crate::ApiValue::Bool(false)])
    };
    (@array [$($values:expr,)*] { $($value:tt)* } $(,)?) => {
        $crate::ApiValue::Array(vec![$($values,)* $crate::api_value!({ $($value)* })])
    };
    (@array [$($values:expr,)*] [ $($value:tt)* ] $(,)?) => {
        $crate::ApiValue::Array(vec![$($values,)* $crate::api_value!([ $($value)* ])])
    };
    (@array [$($values:expr,)*] $value:expr $(,)?) => {
        $crate::ApiValue::Array(vec![$($values,)* $crate::IntoApiValue::into_api_value($value)])
    };
}

/// A failure while lowering a serializable domain value into the typed API ABI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApiValueError(String);

impl fmt::Display for ApiValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ApiValueError {}

impl ser::Error for ApiValueError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

impl serde::de::Error for ApiValueError {
    fn custom<T: fmt::Display>(message: T) -> Self {
        Self(message.to_string())
    }
}

pub struct ApiValueDeserializer(ApiValue);

impl ApiValueDeserializer {
    pub fn new(value: ApiValue) -> Self {
        Self(value)
    }
}

fn expected_api_value(value: &ApiValue, expected: &str) -> ApiValueError {
    ApiValueError(format!("expected {expected}, found {}", value.type_name()))
}

pub fn validate_reflection_value(value: &ApiValue) -> Result<(), String> {
    match value {
        ApiValue::Float(value) if !value.is_finite() => {
            Err("typed API value contains a non-finite f64".into())
        }
        ApiValue::Bytes(_) => Err("binary API values require an explicit byte transport".into()),
        ApiValue::Array(values) => {
            for value in values {
                validate_reflection_value(value)?;
            }
            Ok(())
        }
        ApiValue::Map(entries) => {
            for (index, (key, value)) in entries.iter().enumerate() {
                if entries[..index].iter().any(|(previous, _)| previous == key) {
                    return Err(format!("typed API map contains duplicate key `{key}`"));
                }
                validate_reflection_value(value)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

macro_rules! api_value_deserialize_signed {
    ($method:ident, $ty:ty, $visit:ident) => {
        fn $method<V>(self, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: serde::de::Visitor<'de>,
        {
            match self.0 {
                ApiValue::Int(value) => <$ty>::try_from(value)
                    .map_err(serde::de::Error::custom)
                    .and_then(|value| visitor.$visit(value)),
                value => Err(expected_api_value(&value, "an integer")),
            }
        }
    };
}

macro_rules! api_value_deserialize_unsigned {
    ($method:ident, $ty:ty, $visit:ident) => {
        fn $method<V>(self, visitor: V) -> Result<V::Value, Self::Error>
        where
            V: serde::de::Visitor<'de>,
        {
            match self.0 {
                ApiValue::Int(value) => <$ty>::try_from(value)
                    .map_err(serde::de::Error::custom)
                    .and_then(|value| visitor.$visit(value)),
                value => Err(expected_api_value(&value, "an unsigned integer")),
            }
        }
    };
}

impl<'de> serde::de::Deserializer<'de> for ApiValueDeserializer {
    type Error = ApiValueError;

    fn deserialize_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Unit => visitor.visit_unit(),
            ApiValue::Int(value) => visitor.visit_i64(value),
            ApiValue::Float(value) => visitor.visit_f64(value),
            ApiValue::Bool(value) => visitor.visit_bool(value),
            ApiValue::Str(value) => visitor.visit_string(value),
            ApiValue::Array(values) => visitor.visit_seq(ApiValueSeqAccess(values.into_iter())),
            ApiValue::Map(values) => visitor.visit_map(ApiValueMapAccess {
                entries: values.into_iter(),
                pending: None,
            }),
            ApiValue::Bytes(value) => visitor.visit_byte_buf(value),
        }
    }

    fn deserialize_bool<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Bool(value) => visitor.visit_bool(value),
            value => Err(expected_api_value(&value, "a boolean")),
        }
    }

    api_value_deserialize_signed!(deserialize_i8, i8, visit_i8);
    api_value_deserialize_signed!(deserialize_i16, i16, visit_i16);
    api_value_deserialize_signed!(deserialize_i32, i32, visit_i32);

    fn deserialize_i64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Int(value) => visitor.visit_i64(value),
            value => Err(expected_api_value(&value, "an integer")),
        }
    }

    api_value_deserialize_unsigned!(deserialize_u8, u8, visit_u8);
    api_value_deserialize_unsigned!(deserialize_u16, u16, visit_u16);
    api_value_deserialize_unsigned!(deserialize_u32, u32, visit_u32);

    fn deserialize_u64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Int(value) => u64::try_from(value)
                .map_err(serde::de::Error::custom)
                .and_then(|value| visitor.visit_u64(value)),
            ApiValue::Str(value) => value
                .parse::<u64>()
                .map_err(serde::de::Error::custom)
                .and_then(|value| visitor.visit_u64(value)),
            value => Err(expected_api_value(&value, "an unsigned integer")),
        }
    }

    fn deserialize_f32<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Int(value) => visitor.visit_f32(value as f32),
            ApiValue::Float(value) => visitor.visit_f32(value as f32),
            value => Err(expected_api_value(&value, "a number")),
        }
    }

    fn deserialize_f64<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Int(value) => visitor.visit_f64(value as f64),
            ApiValue::Float(value) => visitor.visit_f64(value),
            value => Err(expected_api_value(&value, "a number")),
        }
    }

    fn deserialize_char<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Str(value) => {
                let mut chars = value.chars();
                let Some(character) = chars.next() else {
                    return Err(ApiValueError("expected a one-character string".into()));
                };
                if chars.next().is_some() {
                    return Err(ApiValueError("expected a one-character string".into()));
                }
                visitor.visit_char(character)
            }
            value => Err(expected_api_value(&value, "a string")),
        }
    }

    fn deserialize_str<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_string(visitor)
    }

    fn deserialize_string<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Str(value) => visitor.visit_string(value),
            value => Err(expected_api_value(&value, "a string")),
        }
    }

    fn deserialize_bytes<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Bytes(value) => visitor.visit_byte_buf(value),
            value => Err(expected_api_value(&value, "a byte buffer")),
        }
    }

    fn deserialize_byte_buf<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Unit => visitor.visit_none(),
            value => visitor.visit_some(ApiValueDeserializer::new(value)),
        }
    }

    fn deserialize_unit<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Unit => visitor.visit_unit(),
            value => Err(expected_api_value(&value, "unit")),
        }
    }

    fn deserialize_unit_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        visitor.visit_newtype_struct(self)
    }

    fn deserialize_seq<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Array(values) => visitor.visit_seq(ApiValueSeqAccess(values.into_iter())),
            value => Err(expected_api_value(&value, "an array")),
        }
    }

    fn deserialize_tuple<V>(self, _length: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_tuple_struct<V>(
        self,
        _name: &'static str,
        _length: usize,
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_seq(visitor)
    }

    fn deserialize_map<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        match self.0 {
            ApiValue::Map(values) => visitor.visit_map(ApiValueMapAccess {
                entries: values.into_iter(),
                pending: None,
            }),
            value => Err(expected_api_value(&value, "a map")),
        }
    }

    fn deserialize_struct<V>(
        self,
        _name: &'static str,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_map(visitor)
    }

    fn deserialize_enum<V>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        let access = match self.0 {
            ApiValue::Str(variant) => ApiValueEnumAccess {
                variant,
                payload: None,
            },
            ApiValue::Map(mut entries) if entries.len() == 1 => {
                let (variant, payload) = entries.pop().expect("one enum variant entry");
                ApiValueEnumAccess {
                    variant,
                    payload: Some(payload),
                }
            }
            value => {
                return Err(expected_api_value(
                    &value,
                    "a unit enum name or single-variant map",
                ));
            }
        };
        visitor.visit_enum(access)
    }

    fn deserialize_identifier<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        self.deserialize_string(visitor)
    }

    fn deserialize_ignored_any<V>(self, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        visitor.visit_unit()
    }
}

struct ApiValueSeqAccess(std::vec::IntoIter<ApiValue>);

impl<'de> serde::de::SeqAccess<'de> for ApiValueSeqAccess {
    type Error = ApiValueError;

    fn next_element_seed<T>(&mut self, seed: T) -> Result<Option<T::Value>, Self::Error>
    where
        T: serde::de::DeserializeSeed<'de>,
    {
        self.0
            .next()
            .map(|value| seed.deserialize(ApiValueDeserializer::new(value)))
            .transpose()
    }
}

struct ApiValueMapAccess {
    entries: std::vec::IntoIter<(String, ApiValue)>,
    pending: Option<ApiValue>,
}

impl<'de> serde::de::MapAccess<'de> for ApiValueMapAccess {
    type Error = ApiValueError;

    fn next_key_seed<K>(&mut self, seed: K) -> Result<Option<K::Value>, Self::Error>
    where
        K: serde::de::DeserializeSeed<'de>,
    {
        if self.pending.is_some() {
            return Err(ApiValueError("map key requested before its value".into()));
        }
        let Some((key, value)) = self.entries.next() else {
            return Ok(None);
        };
        self.pending = Some(value);
        seed.deserialize(serde::de::value::StringDeserializer::new(key))
            .map(Some)
    }

    fn next_value_seed<V>(&mut self, seed: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::DeserializeSeed<'de>,
    {
        let value = self
            .pending
            .take()
            .ok_or_else(|| ApiValueError("map value requested without a key".into()))?;
        seed.deserialize(ApiValueDeserializer::new(value))
    }
}

struct ApiValueEnumAccess {
    variant: String,
    payload: Option<ApiValue>,
}

impl<'de> serde::de::EnumAccess<'de> for ApiValueEnumAccess {
    type Error = ApiValueError;
    type Variant = ApiValueVariantAccess;

    fn variant_seed<V>(self, seed: V) -> Result<(V::Value, Self::Variant), Self::Error>
    where
        V: serde::de::DeserializeSeed<'de>,
    {
        let variant = seed.deserialize(serde::de::value::StringDeserializer::new(self.variant))?;
        Ok((variant, ApiValueVariantAccess(self.payload)))
    }
}

struct ApiValueVariantAccess(Option<ApiValue>);

impl<'de> serde::de::VariantAccess<'de> for ApiValueVariantAccess {
    type Error = ApiValueError;

    fn unit_variant(self) -> Result<(), Self::Error> {
        match self.0 {
            None | Some(ApiValue::Unit) => Ok(()),
            Some(value) => Err(expected_api_value(&value, "a unit enum payload")),
        }
    }

    fn newtype_variant_seed<T>(self, seed: T) -> Result<T::Value, Self::Error>
    where
        T: serde::de::DeserializeSeed<'de>,
    {
        seed.deserialize(ApiValueDeserializer::new(self.0.ok_or_else(|| {
            ApiValueError("newtype enum payload is missing".into())
        })?))
    }

    fn tuple_variant<V>(self, _length: usize, visitor: V) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        serde::de::Deserializer::deserialize_seq(
            ApiValueDeserializer::new(
                self.0
                    .ok_or_else(|| ApiValueError("tuple enum payload is missing".into()))?,
            ),
            visitor,
        )
    }

    fn struct_variant<V>(
        self,
        _fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Self::Error>
    where
        V: serde::de::Visitor<'de>,
    {
        serde::de::Deserializer::deserialize_map(
            ApiValueDeserializer::new(
                self.0
                    .ok_or_else(|| ApiValueError("struct enum payload is missing".into()))?,
            ),
            visitor,
        )
    }
}

/// Serialize an ordinary Rust value directly into the language-neutral API
/// ABI. This avoids using JSON as a temporary representation in query owners.
pub fn api_value_from_serializable<T: serde::Serialize + ?Sized>(
    value: &T,
) -> Result<ApiValue, ApiValueError> {
    value.serialize(ApiValueSerializer)
}

struct ApiValueSerializer;

impl serde::Serializer for ApiValueSerializer {
    type Ok = ApiValue;
    type Error = ApiValueError;
    type SerializeSeq = SequenceSerializer;
    type SerializeTuple = SequenceSerializer;
    type SerializeTupleStruct = SequenceSerializer;
    type SerializeTupleVariant = SequenceSerializer;
    type SerializeMap = MapSerializer;
    type SerializeStruct = MapSerializer;
    type SerializeStructVariant = StructVariantSerializer;

    fn serialize_bool(self, value: bool) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::Bool(value))
    }

    fn serialize_i8(self, value: i8) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i16(self, value: i16) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i32(self, value: i32) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_i64(self, value: i64) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::Int(value))
    }

    fn serialize_i128(self, value: i128) -> Result<Self::Ok, Self::Error> {
        Ok(i64::try_from(value)
            .map(ApiValue::Int)
            .unwrap_or_else(|_| ApiValue::Str(value.to_string())))
    }

    fn serialize_u8(self, value: u8) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_u16(self, value: u16) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_u32(self, value: u32) -> Result<Self::Ok, Self::Error> {
        self.serialize_i64(i64::from(value))
    }

    fn serialize_u64(self, value: u64) -> Result<Self::Ok, Self::Error> {
        Ok(api_value_from_u64(value))
    }

    fn serialize_u128(self, value: u128) -> Result<Self::Ok, Self::Error> {
        Ok(i64::try_from(value)
            .map(ApiValue::Int)
            .unwrap_or_else(|_| ApiValue::Str(value.to_string())))
    }

    fn serialize_f32(self, value: f32) -> Result<Self::Ok, Self::Error> {
        self.serialize_f64(f64::from(value))
    }

    fn serialize_f64(self, value: f64) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::Float(value))
    }

    fn serialize_char(self, value: char) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::str(value.to_string()))
    }

    fn serialize_str(self, value: &str) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::str(value))
    }

    fn serialize_bytes(self, value: &[u8]) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::Bytes(value.to_vec()))
    }

    fn serialize_none(self) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::Unit)
    }

    fn serialize_some<T: serde::Serialize + ?Sized>(
        self,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_unit(self) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::Unit)
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::Unit)
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::str(variant))
    }

    fn serialize_newtype_struct<T: serde::Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        value.serialize(self)
    }

    fn serialize_newtype_variant<T: serde::Serialize + ?Sized>(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        value: &T,
    ) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::map([(variant, value.serialize(self)?)]))
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<Self::SerializeSeq, Self::Error> {
        Ok(SequenceSerializer::new(None))
    }

    fn serialize_tuple(self, _len: usize) -> Result<Self::SerializeTuple, Self::Error> {
        Ok(SequenceSerializer::new(None))
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleStruct, Self::Error> {
        Ok(SequenceSerializer::new(None))
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeTupleVariant, Self::Error> {
        Ok(SequenceSerializer::new(Some(variant)))
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<Self::SerializeMap, Self::Error> {
        Ok(MapSerializer::default())
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStruct, Self::Error> {
        Ok(MapSerializer::default())
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _variant_index: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<Self::SerializeStructVariant, Self::Error> {
        Ok(StructVariantSerializer {
            variant,
            fields: Vec::new(),
        })
    }
}

struct SequenceSerializer {
    variant: Option<&'static str>,
    values: Vec<ApiValue>,
}

impl SequenceSerializer {
    fn new(variant: Option<&'static str>) -> Self {
        Self {
            variant,
            values: Vec::new(),
        }
    }

    fn finish(self) -> ApiValue {
        let values = ApiValue::Array(self.values);
        self.variant
            .map(|variant| ApiValue::map([(variant, values.clone())]))
            .unwrap_or(values)
    }
}

impl SerializeSeq for SequenceSerializer {
    type Ok = ApiValue;
    type Error = ApiValueError;

    fn serialize_element<T: serde::Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.values.push(value.serialize(ApiValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.finish())
    }
}

impl SerializeTuple for SequenceSerializer {
    type Ok = ApiValue;
    type Error = ApiValueError;

    fn serialize_element<T: serde::Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.values.push(value.serialize(ApiValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.finish())
    }
}

impl SerializeTupleStruct for SequenceSerializer {
    type Ok = ApiValue;
    type Error = ApiValueError;

    fn serialize_field<T: serde::Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.values.push(value.serialize(ApiValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(self.finish())
    }
}

impl SerializeTupleVariant for SequenceSerializer {
    type Ok = ApiValue;
    type Error = ApiValueError;

    fn serialize_field<T: serde::Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.values.push(value.serialize(ApiValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        let variant = self.variant.expect("tuple variant has a variant name");
        Ok(ApiValue::map([(variant, ApiValue::Array(self.values))]))
    }
}

#[derive(Default)]
struct MapSerializer {
    entries: Vec<(String, ApiValue)>,
    pending_key: Option<String>,
}

impl MapSerializer {
    fn insert(&mut self, key: String, value: ApiValue) {
        if let Some((_, existing)) = self.entries.iter_mut().find(|(name, _)| name == &key) {
            *existing = value;
        } else {
            self.entries.push((key, value));
        }
    }
}

impl SerializeMap for MapSerializer {
    type Ok = ApiValue;
    type Error = ApiValueError;

    fn serialize_key<T: serde::Serialize + ?Sized>(&mut self, key: &T) -> Result<(), Self::Error> {
        if self.pending_key.is_some() {
            return Err(ApiValueError("map key has no corresponding value".into()));
        }
        self.pending_key = Some(match key.serialize(ApiValueSerializer)? {
            ApiValue::Str(key) => key,
            _ => return Err(ApiValueError("API maps require string keys".into())),
        });
        Ok(())
    }

    fn serialize_value<T: serde::Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<(), Self::Error> {
        let key = self
            .pending_key
            .take()
            .ok_or_else(|| ApiValueError("map value has no preceding key".into()))?;
        self.insert(key, value.serialize(ApiValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        if self.pending_key.is_some() {
            return Err(ApiValueError("map key has no corresponding value".into()));
        }
        Ok(ApiValue::Map(self.entries))
    }
}

impl SerializeStruct for MapSerializer {
    type Ok = ApiValue;
    type Error = ApiValueError;

    fn serialize_field<T: serde::Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.insert(key.to_owned(), value.serialize(ApiValueSerializer)?);
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::Map(self.entries))
    }
}

struct StructVariantSerializer {
    variant: &'static str,
    fields: Vec<(String, ApiValue)>,
}

impl SerializeStructVariant for StructVariantSerializer {
    type Ok = ApiValue;
    type Error = ApiValueError;

    fn serialize_field<T: serde::Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<(), Self::Error> {
        self.fields
            .push((key.to_owned(), value.serialize(ApiValueSerializer)?));
        Ok(())
    }

    fn end(self) -> Result<Self::Ok, Self::Error> {
        Ok(ApiValue::map([(self.variant, ApiValue::Map(self.fields))]))
    }
}

/// Represent an unsigned value without narrowing it through the signed
/// in-process integer ABI. Values outside `i64` use decimal text, which the
/// typed deserializer parses back to `u64`.
pub fn api_value_from_u64(value: u64) -> ApiValue {
    i64::try_from(value)
        .map(ApiValue::Int)
        .unwrap_or_else(|_| ApiValue::Str(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_binary_values_fail_visibly() {
        let error = validate_reflection_value(&ApiValue::Bytes(vec![1, 2, 3]))
            .expect_err("binary data is not a reflected command parameter");
        assert!(error.contains("binary API values"));
    }

    #[test]
    fn non_finite_and_duplicate_values_are_rejected_before_reflection() {
        let error = validate_reflection_value(&ApiValue::Float(f64::NAN))
            .expect_err("non-finite numbers cannot cross the command seam");
        assert!(error.contains("non-finite"));

        let error = validate_reflection_value(&ApiValue::map([
            ("value", ApiValue::Int(1)),
            ("value", ApiValue::Int(2)),
        ]))
        .expect_err("duplicate map keys are ambiguous command input");
        assert!(error.contains("duplicate key"));
    }

    #[test]
    fn unsigned_values_round_trip_without_precision_loss() {
        use serde::Deserialize;

        let value = api_value_from_serializable(&u64::MAX).expect("serialize unsigned value");
        assert_eq!(value, ApiValue::Str(u64::MAX.to_string()));
        assert_eq!(
            u64::deserialize(ApiValueDeserializer::new(value)).expect("deserialize unsigned value"),
            u64::MAX
        );
    }
}
