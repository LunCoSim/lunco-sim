//! Owner-controlled runtime state is inspectable without generic mutation.

/// Reflection type data denying raw writes to owner-maintained state.
/// Typed commands and the owning engine mechanism retain their normal access.
/// Install through `#[reflect(ReadOnly)]` alongside this type's import.
#[derive(Clone, Default, bevy::reflect::Reflect)]
pub struct ReflectReadOnly;

impl<T: 'static> bevy::reflect::CreateTypeData<T> for ReflectReadOnly {
    fn create_type_data(_: ()) -> Self {
        Self
    }
}

impl ReflectReadOnly {
    /// Enforce component/resource and root-field ownership before mutable
    /// reflection is obtained. Parent replacement cannot bypass protected fields.
    pub fn check_write(
        registration: &bevy::reflect::TypeRegistration,
        path: &str,
    ) -> Result<(), String> {
        use bevy::reflect::{Access, ParsedPath, TypeInfo};
        let denied = || {
            format!(
                "'{}' is owner-controlled read-only state at '{path}'; use its typed commands",
                registration.type_info().type_path()
            )
        };
        if registration.data::<Self>().is_some() {
            return Err(denied());
        }
        let TypeInfo::Struct(info) = registration.type_info() else {
            return Ok(());
        };
        if path.is_empty() {
            return if info
                .iter()
                .any(|field| field.get_attribute::<Self>().is_some())
            {
                Err(denied())
            } else {
                Ok(())
            };
        }
        let parsed = ParsedPath::parse(path).map_err(|error| error.to_string())?;
        let field = parsed.0.first().and_then(|element| match &element.access {
            Access::Field(name) => info.field(name),
            Access::FieldIndex(index) => info.field_at(*index),
            _ => None,
        });
        if field.is_some_and(|field| field.get_attribute::<Self>().is_some()) {
            Err(denied())
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::reflect::{GetTypeRegistration, Reflect};

    #[derive(Reflect)]
    struct OwnedState {
        #[reflect(@ReflectReadOnly)]
        sample: f64,
        editable: f64,
    }

    #[test]
    fn field_ownership_rejects_named_indexed_and_parent_replacement() {
        let registration = OwnedState::get_type_registration();
        assert!(ReflectReadOnly::check_write(&registration, ".sample").is_err());
        assert!(ReflectReadOnly::check_write(&registration, "#0").is_err());
        assert!(ReflectReadOnly::check_write(&registration, "").is_err());
        assert!(ReflectReadOnly::check_write(&registration, ".editable").is_ok());
    }
}
