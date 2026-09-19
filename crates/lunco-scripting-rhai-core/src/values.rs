//! Generic Rhai value conversion and reflected writes.
//!
//! This module is the single Rhai-to-typed-value boundary and the reflected
//! write implementation. JSON serialization belongs to the API/transport
//! adapter, never to the scripting core or world bridge.

use bevy::math::{DQuat, DVec3};
use bevy::prelude::GetPath;
use lunco_hooks::HookValue;
use rhai::{Dynamic, Map};

/// Convert a Rhai value to the shared typed in-process value ABI.
///
/// Native vectors and quaternions are lowered to arrays because the generic
/// hook ABI intentionally has no Bevy dependency. The canonical scalar/map
/// conversion is owned by `lunco-hooks-rhai`.
pub fn dynamic_to_hook_value(value: &Dynamic) -> Result<HookValue, String> {
    let normalized = normalize_native_value(value)?;
    lunco_hooks_rhai::dynamic_to_hook(&normalized).map_err(|error| error.to_string())
}

fn normalize_native_value(value: &Dynamic) -> Result<Dynamic, String> {
    if let Some(vector) = value.clone().try_cast::<DVec3>() {
        if !vector.is_finite() {
            return Err("Vec3 contains a non-finite component".to_string());
        }
        return Ok(Dynamic::from_array(vec![
            Dynamic::from_float(vector.x),
            Dynamic::from_float(vector.y),
            Dynamic::from_float(vector.z),
        ]));
    }
    if let Some(quaternion) = value.clone().try_cast::<DQuat>() {
        if !quaternion.is_finite() {
            return Err("Quat contains a non-finite component".to_string());
        }
        return Ok(Dynamic::from_array(vec![
            Dynamic::from_float(quaternion.x),
            Dynamic::from_float(quaternion.y),
            Dynamic::from_float(quaternion.z),
            Dynamic::from_float(quaternion.w),
        ]));
    }
    if let Some(array) = value.clone().try_cast::<rhai::Array>() {
        return array
            .iter()
            .map(normalize_native_value)
            .collect::<Result<Vec<_>, _>>()
            .map(Dynamic::from_array);
    }
    if let Some(map) = value.clone().try_cast::<Map>() {
        let mut normalized = Map::new();
        for (key, item) in map {
            normalized.insert(key, normalize_native_value(&item)?);
        }
        return Ok(Dynamic::from_map(normalized));
    }
    Ok(value.clone())
}

/// Turn a typed-boundary error into a Rhai evaluation error.
pub fn value_boundary_error(message: String) -> Box<rhai::EvalAltResult> {
    rhai::EvalAltResult::ErrorRuntime(message.into(), rhai::Position::NONE).into()
}

/// A rhai [`Dynamic`] as `f64`; reject integers that cannot be represented exactly.
fn dyn_f64(v: &Dynamic) -> Result<f64, String> {
    let value = if let Ok(value) = v.as_float() {
        value
    } else if let Ok(integer) = v.as_int() {
        let value = integer as f64;
        if value as i128 != integer as i128 {
            return Err("integer cannot be represented exactly as f64".to_string());
        }
        value
    } else {
        return Err("expected a number".to_string());
    };
    value
        .is_finite()
        .then_some(value)
        .ok_or_else(|| "expected a finite number".to_string())
}

/// A rhai [`Dynamic`] as `i64`; floating-point inputs must be integral and in range.
fn dyn_i64(v: &Dynamic) -> Result<i64, String> {
    if let Ok(value) = v.as_int() {
        return Ok(value);
    }
    let value = v
        .as_float()
        .map_err(|_| "expected an integer".to_string())?;
    if !value.is_finite() || value.fract() != 0.0 {
        return Err("expected a finite integer".to_string());
    }
    // `i64::MAX as f64` rounds up to 2^63, which is outside the signed range.
    if value < i64::MIN as f64 || value >= -(i64::MIN as f64) {
        return Err("integer is outside the i64 range".to_string());
    }
    Ok(value as i64)
}

/// A rhai [`Dynamic`] as `u64`; inputs must be integral and in range.
fn dyn_u64(v: &Dynamic) -> Result<u64, String> {
    if let Ok(value) = v.as_int() {
        return u64::try_from(value).map_err(|_| "expected a non-negative integer".to_string());
    }
    let value = v
        .as_float()
        .map_err(|_| "expected an integer".to_string())?;
    if !value.is_finite() || value.fract() != 0.0 || value < 0.0 {
        return Err("expected a non-negative finite integer".to_string());
    }
    // `u64::MAX as f64` rounds up to 2^64, which is outside the unsigned range.
    if value >= 2.0_f64.powi(64) {
        return Err("integer is outside the u64 range".to_string());
    }
    Ok(value as u64)
}

fn dyn_f32(v: &Dynamic) -> Result<f32, String> {
    let value = dyn_f64(v)? as f32;
    value
        .is_finite()
        .then_some(value)
        .ok_or_else(|| "number is outside the f32 range".to_string())
}

fn dyn_f32s(v: &Dynamic, n: usize) -> Result<Vec<f32>, String> {
    dyn_f64s(v, n)?
        .into_iter()
        .map(|value| {
            let narrowed = value as f32;
            narrowed
                .is_finite()
                .then_some(narrowed)
                .ok_or_else(|| "vector component is outside the f32 range".to_string())
        })
        .collect()
}

/// A rhai array of exactly `n` numbers as `f64`s — for glam vec/quat fields.
fn dyn_f64s(v: &Dynamic, n: usize) -> Result<Vec<f64>, String> {
    if n == 3 {
        if let Some(vector) = v.clone().try_cast::<DVec3>() {
            return [vector.x, vector.y, vector.z]
                .into_iter()
                .map(|value| {
                    value
                        .is_finite()
                        .then_some(value)
                        .ok_or_else(|| "expected finite vector components".to_string())
                })
                .collect();
        }
    }
    if n == 4 {
        if let Some(quaternion) = v.clone().try_cast::<DQuat>() {
            return [quaternion.x, quaternion.y, quaternion.z, quaternion.w]
                .into_iter()
                .map(|value| {
                    value
                        .is_finite()
                        .then_some(value)
                        .ok_or_else(|| "expected finite quaternion components".to_string())
                })
                .collect();
        }
    }
    let arr = v
        .clone()
        .try_cast::<rhai::Array>()
        .ok_or_else(|| format!("expected an array of {n} numbers"))?;
    if arr.len() != n {
        return Err(format!("expected {n} numbers, got {}", arr.len()));
    }
    arr.iter().map(dyn_f64).collect()
}

/// Write a rhai [`Dynamic`] straight onto a reflected field — the inverse of
/// [`build_from_reflect`](bridge_core::build_from_reflect)'s read. The field's
/// concrete type drives the conversion (integer and float ranges are checked;
/// arrays become glam vectors/quats), so `native → reflect` happens in one hop
/// with no JSON. Unsupported field types return an error the script verb surfaces.
pub fn dynamic_write_supported(type_path: &str) -> bool {
    matches!(
        type_path.rsplit("::").next().unwrap_or(type_path),
        "f64"
            | "f32"
            | "i64"
            | "i32"
            | "u64"
            | "u32"
            | "usize"
            | "bool"
            | "String"
            | "Vec2"
            | "Vec3"
            | "Quat"
            | "DVec2"
            | "DVec3"
    )
}

pub fn apply_dynamic(
    field: &mut dyn bevy::reflect::PartialReflect,
    value: &Dynamic,
) -> Result<(), String> {
    use bevy::math::{DVec2, DVec3, Quat, Vec2, Vec3};
    if !dynamic_write_supported(field.reflect_type_path()) {
        return Err(format!(
            "set: unsupported field type '{}'",
            field.reflect_type_path()
        ));
    }
    let any = field
        .try_as_reflect_mut()
        .ok_or_else(|| "field is not concretely reflectable".to_string())?
        .as_any_mut();

    if let Some(s) = any.downcast_mut::<f64>() {
        *s = dyn_f64(value)?;
    } else if let Some(s) = any.downcast_mut::<f32>() {
        *s = dyn_f32(value)?;
    } else if let Some(s) = any.downcast_mut::<i64>() {
        *s = dyn_i64(value)?;
    } else if let Some(s) = any.downcast_mut::<i32>() {
        *s = i32::try_from(dyn_i64(value)?)
            .map_err(|_| "integer is outside the i32 range".to_string())?;
    } else if let Some(s) = any.downcast_mut::<u64>() {
        *s = dyn_u64(value)?;
    } else if let Some(s) = any.downcast_mut::<u32>() {
        *s = u32::try_from(dyn_u64(value)?)
            .map_err(|_| "integer is outside the u32 range".to_string())?;
    } else if let Some(s) = any.downcast_mut::<usize>() {
        *s = usize::try_from(dyn_u64(value)?)
            .map_err(|_| "integer is outside the usize range".to_string())?;
    } else if let Some(s) = any.downcast_mut::<bool>() {
        *s = value.as_bool().map_err(|_| "expected a bool".to_string())?;
    } else if let Some(s) = any.downcast_mut::<String>() {
        *s = value
            .clone()
            .into_string()
            .map_err(|_| "expected a string".to_string())?;
    } else if let Some(v) = any.downcast_mut::<Vec3>() {
        let a = dyn_f32s(value, 3)?;
        *v = Vec3::new(a[0], a[1], a[2]);
    } else if let Some(v) = any.downcast_mut::<Vec2>() {
        let a = dyn_f32s(value, 2)?;
        *v = Vec2::new(a[0], a[1]);
    } else if let Some(v) = any.downcast_mut::<Quat>() {
        let a = dyn_f32s(value, 4)?;
        *v = Quat::from_xyzw(a[0], a[1], a[2], a[3]);
    } else if let Some(v) = any.downcast_mut::<DVec3>() {
        let a = dyn_f64s(value, 3)?;
        *v = DVec3::new(a[0], a[1], a[2]);
    } else if let Some(v) = any.downcast_mut::<DVec2>() {
        let a = dyn_f64s(value, 2)?;
        *v = DVec2::new(a[0], a[1]);
    } else {
        return Err(format!(
            "set: unsupported field type '{}'",
            field.reflect_type_path()
        ));
    }
    Ok(())
}

/// Patch a default-constructed component with a rhai field map — each `key: val`
/// is written onto the matching reflected field via [`apply_dynamic`]. Used by the
/// `add` verb to build a component natively (no JSON).
pub fn apply_dynamic_fields(
    component: &mut dyn bevy::reflect::Reflect,
    fields: &Map,
) -> Result<(), String> {
    for (k, v) in fields.iter() {
        let path = format!(".{k}");
        let field = component
            .reflect_path_mut(path.as_str())
            .map_err(|e| format!("no field '{k}': {e}"))?;
        apply_dynamic(field, v)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{apply_dynamic, dyn_f64, dyn_i64, dyn_u64};
    use rhai::Dynamic;

    #[test]
    fn numeric_conversions_reject_precision_loss_fraction_and_overflow() {
        assert!(dyn_f64(&Dynamic::from_int(9_007_199_254_740_993)).is_err());
        assert!(dyn_i64(&Dynamic::from_float(1.5)).is_err());
        assert!(dyn_i64(&Dynamic::from_float(2.0_f64.powi(63))).is_err());
        assert!(dyn_u64(&Dynamic::from_int(-1)).is_err());
        assert!(dyn_u64(&Dynamic::from_float(2.0_f64.powi(64))).is_err());
    }

    #[test]
    fn reflected_integer_writes_reject_out_of_range_values() {
        let mut narrow = i32::default();
        assert!(apply_dynamic(&mut narrow, &Dynamic::from_int(i64::MAX)).is_err());
        assert_eq!(narrow, 0);

        let mut unsigned = u32::default();
        assert!(apply_dynamic(&mut unsigned, &Dynamic::from_int(-1)).is_err());
        assert_eq!(unsigned, 0);
    }
}
