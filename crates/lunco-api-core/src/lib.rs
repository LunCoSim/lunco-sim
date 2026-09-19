//! Typed, transport-independent API values and request/response contracts.
//!
//! This crate contains no transport representation. JSON belongs to the
//! transport codecs; in-process callers exchange [`ApiValue`] directly.

pub mod schema;
pub mod value;

pub use schema::*;
pub use value::{
    ApiValue, ApiValueDeserializer, ApiValueError, IntoApiValue, api_value_from_serializable,
    api_value_from_u64, validate_reflection_value,
};
