//! MLS 13.5 URI interpretation. This module supplies language facts only;
//! asset sources own addressing, containment and bytes.

use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelicaUri {
    pub qualified: String,
    pub resource: Option<PathBuf>,
    pub fragment: Option<String>,
}

impl ModelicaUri {
    /// `None` means a different scheme. Malformed Modelica URIs are errors.
    pub fn parse(uri: &str) -> Result<Option<Self>, String> {
        if !lunco_assets_path::uri_scheme(uri)
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("modelica"))
        {
            return Ok(None);
        }
        let (_, body) = uri
            .split_once("://")
            .ok_or("Modelica URI requires // and a qualified package authority")?;
        let (body, fragment) = match body.split_once('#') {
            Some((body, fragment)) => (
                body,
                Some(lunco_assets_path::decode_uri_component(fragment)?),
            ),
            None => (body, None),
        };
        if body.contains('?') {
            return Err("Modelica resource URIs do not define a query".into());
        }
        let (qualified, resource) = match body.split_once('/') {
            Some((qualified, path)) => {
                let mut relative = PathBuf::new();
                for segment in path.split('/') {
                    let segment = lunco_assets_path::decode_uri_component(segment)?;
                    if segment.contains(['/', '\\']) {
                        return Err("encoded resource separator".into());
                    }
                    let segment = lunco_assets_path::relative_path(&segment)
                        .ok_or("unsafe Modelica resource path")?;
                    relative.push(segment);
                }
                (qualified, Some(relative))
            }
            None => (body, None),
        };
        let qualified = lunco_assets_path::decode_uri_component(qualified)?;
        let segments = rumoca_core::qualified_name_segments(&qualified).collect::<Vec<_>>();
        if segments.is_empty() || segments.iter().any(|part| part.is_empty()) {
            return Err("invalid fully qualified Modelica URI name".into());
        }
        // Rumoca owns identifier syntax, including quoted identifiers. No URI
        // parser maintains a second Modelica lexer.
        let declaration = format!("within {qualified}; model UriName end UriName;");
        let parsed = rumoca_phase_parse::parse_to_ast(&declaration, "modelica-uri")
            .map_err(|error| format!("invalid Modelica URI name: {error}"))?;
        if parsed.within.as_ref().map(ToString::to_string).as_deref() != Some(&qualified) {
            return Err("invalid fully qualified Modelica URI name".into());
        }
        Ok(Some(Self {
            qualified,
            resource,
            fragment,
        }))
    }

    /// Resource directories follow the qualified hierarchy even when nested
    /// packages share one physical source file (MLS 13.5).
    pub fn package_relative_resource(&self) -> Result<PathBuf, String> {
        let resource = self
            .resource
            .as_ref()
            .ok_or("class reference is not a resource")?;
        let mut path = PathBuf::new();
        for part in rumoca_core::qualified_name_segments(&self.qualified).skip(1) {
            let component = lunco_assets_path::relative_path(part)
                .filter(|path| path.components().count() == 1)
                .ok_or("package name cannot be represented by this asset store")?;
            path.push(component);
        }
        path.push(resource);
        Ok(path)
    }
}

/// Actual package declaration and its containing source identity.
/// Nested resource directories are derived from the qualified package name,
/// independently of whether the package is represented by one or many files.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PackageSource {
    pub namespace: String,
    pub source_uri: String,
    pub members: Vec<String>,
}

pub fn package_sources(
    definition: &rumoca_ir_ast::StoredDefinition,
    source_uri: &str,
) -> Vec<PackageSource> {
    fn visit(
        class: &rumoca_ir_ast::ClassDef,
        parent: &str,
        uri: &str,
        out: &mut Vec<PackageSource>,
    ) {
        let qualified = if parent.is_empty() {
            class.name.text.to_string()
        } else {
            format!("{parent}.{}", class.name.text)
        };
        if class.class_type == rumoca_core::ClassType::Package {
            out.push(PackageSource {
                namespace: qualified.clone(),
                source_uri: uri.to_owned(),
                members: class.classes.keys().cloned().collect(),
            });
        }
        for child in class.classes.values() {
            visit(child, &qualified, uri, out);
        }
    }
    let mut out = Vec::new();
    let parent = definition
        .within
        .as_ref()
        .map(ToString::to_string)
        .unwrap_or_default();
    for class in definition.classes.values() {
        visit(class, &parent, source_uri, &mut out);
    }
    out
}

/// Parsed facts retain their source provenance so one editor generation can
/// replace one file without discarding declarations from sibling files.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SourceFileFacts {
    pub source_uri: String,
    pub packages: Vec<PackageSource>,
    pub classes: Vec<String>,
    pub error: Option<String>,
}

impl SourceFileFacts {
    pub fn parsed(definition: &rumoca_ir_ast::StoredDefinition, source_uri: &str) -> Self {
        let mut classes = definition.declared_class_names();
        classes.sort();
        Self {
            source_uri: source_uri.to_owned(),
            packages: package_sources(definition, source_uri),
            classes,
            error: None,
        }
    }
}

/// Merge language membership across source files, preserving each package's
/// authored source identity. Separate class files participate in MLS 13.5.
pub fn package_sources_for_files<'a>(
    files: impl IntoIterator<Item = &'a SourceFileFacts>,
) -> Result<Vec<PackageSource>, String> {
    let mut packages = Vec::new();
    let mut members =
        std::collections::HashMap::<String, std::collections::BTreeSet<String>>::new();
    for file in files {
        if let Some(error) = &file.error {
            return Err(error.clone());
        }
        packages.extend(file.packages.iter().cloned());
        for qualified in &file.classes {
            let mut parts = rumoca_core::qualified_name_segments(qualified).collect::<Vec<_>>();
            if let Some(member) = (parts.len() > 1).then(|| parts.pop()).flatten() {
                members
                    .entry(parts.join("."))
                    .or_default()
                    .insert(member.to_owned());
            }
        }
    }
    for package in &mut packages {
        if let Some(members) = members.get(&package.namespace) {
            package.members = members.iter().cloned().collect();
        }
    }
    Ok(packages)
}

pub fn source_set_package_sources<'a>(
    documents: impl IntoIterator<Item = (&'a rumoca_ir_ast::StoredDefinition, &'a str)>,
) -> Result<Vec<PackageSource>, String> {
    let files = documents
        .into_iter()
        .map(|(definition, uri)| SourceFileFacts::parsed(definition, uri))
        .collect::<Vec<_>>();
    package_sources_for_files(&files)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn asset_resolution_contract() {
        let uri = ModelicaUri::parse("mOdElIcA://A.B/Resources/literal%20%23%20%25.png#view")
            .unwrap()
            .unwrap();
        assert_eq!(uri.qualified, "A.B");
        assert_eq!(
            uri.package_relative_resource().unwrap(),
            PathBuf::from("B/Resources/literal # %.png")
        );
        assert_eq!(uri.fragment.as_deref(), Some("view"));
        assert_eq!(
            ModelicaUri::parse("modelica://A.%27B.C%27/file.png")
                .unwrap()
                .unwrap()
                .qualified,
            "A.'B.C'"
        );
        let quoted = ModelicaUri::parse("modelica://A.%27B%3AC%27/file.png")
            .unwrap()
            .unwrap();
        assert_eq!(quoted.qualified, "A.'B:C'");
        assert!(quoted.package_relative_resource().is_err());
        assert!(ModelicaUri::parse("https://example.com").unwrap().is_none());
        for invalid in [
            "modelica:/A",
            "modelica://",
            "modelica://A../x",
            "modelica://A/%2e%2e/x",
            "modelica://A/%2Fetc",
            "modelica://A/x%00",
            "modelica://A/x%GG",
            "modelica://A/x?query",
        ] {
            assert!(ModelicaUri::parse(invalid).is_err(), "{invalid}");
        }
        let definition =
            rumoca_phase_parse::parse_to_ast("package A package B end B; end A;", "A.mo").unwrap();
        let packages = package_sources(&definition, "A.mo");
        assert_eq!(
            packages
                .iter()
                .map(|package| package.namespace.as_str())
                .collect::<Vec<_>>(),
            vec!["A", "A.B"]
        );
        assert_eq!(packages[0].members, vec!["B"]);
        let sibling =
            rumoca_phase_parse::parse_to_ast("within A; model Sibling end Sibling;", "Sibling.mo")
                .unwrap();
        let replacement =
            rumoca_phase_parse::parse_to_ast("package A package C end C; end A;", "A.mo").unwrap();
        let files = vec![
            SourceFileFacts::parsed(&replacement, "A.mo"),
            SourceFileFacts::parsed(&sibling, "Sibling.mo"),
        ];
        let packages = package_sources_for_files(&files).unwrap();
        assert_eq!(
            packages
                .iter()
                .find(|package| package.namespace == "A")
                .unwrap()
                .members,
            vec!["C", "Sibling"]
        );
        assert!(!packages.iter().any(|package| package.namespace == "A.B"));
        let mut invalid = files[0].clone();
        invalid.error = Some("source has no current parse".into());
        assert_eq!(
            package_sources_for_files([&invalid]).unwrap_err(),
            "source has no current parse"
        );
    }
}
