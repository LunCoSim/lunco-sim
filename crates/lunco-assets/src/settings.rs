//! Resolve preparation policy against the dataset's owning Twin, not UI focus.

use lunco_settings::{AssetProcessingSettings, SettingsSection};
use lunco_twin::TwinSettingValue;

pub const TWIN_TEXTURE_DECODE_MAX_BYTES: &str = "assets.texture_decode_max_bytes";

pub fn texture_decode_budget(
    settings: &AssetProcessingSettings,
    twin_value: Option<&TwinSettingValue>,
) -> Result<u64, String> {
    settings.validate_section()?;
    match twin_value {
        None => Ok(settings.texture_decode_max_bytes),
        Some(TwinSettingValue::Integer(bytes)) if *bytes > 0 => Ok(*bytes as u64),
        Some(value) => Err(format!(
            "{TWIN_TEXTURE_DECODE_MAX_BYTES} must be a positive integer number of bytes, got {value:?}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_budget_uses_global_policy_or_valid_explicit_twin_value() {
        let mut settings = AssetProcessingSettings::default();
        assert_eq!(texture_decode_budget(&settings, None).unwrap(), 1 << 30);
        settings.texture_decode_max_bytes = 3 << 30;
        assert_eq!(texture_decode_budget(&settings, None).unwrap(), 3 << 30);
        assert_eq!(
            texture_decode_budget(&settings, Some(&TwinSettingValue::Integer(2 << 30))).unwrap(),
            2 << 30
        );
        for bad in [
            TwinSettingValue::Integer(0),
            TwinSettingValue::Integer(-1),
            TwinSettingValue::Text("2 GiB".into()),
            TwinSettingValue::Bool(true),
        ] {
            assert!(
                texture_decode_budget(&settings, Some(&bad))
                    .unwrap_err()
                    .contains(TWIN_TEXTURE_DECODE_MAX_BYTES)
            );
        }
        settings.texture_decode_max_bytes = 0;
        assert!(texture_decode_budget(&settings, None).is_err());
    }
}
