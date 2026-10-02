//! Named diagram lenses over a source document. Topology is never persisted here.

use crate::{Document, DocumentError, DocumentId, DocumentOp};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagramPosition {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagramView {
    pub scope: String,
    #[serde(default)]
    pub include_descendants: bool,
    #[serde(default)]
    pub positions: BTreeMap<String, DiagramPosition>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagramViews {
    pub version: u32,
    pub source: String,
    pub views: BTreeMap<String, DiagramView>,
}

impl DiagramViews {
    pub fn validate(&self) -> Result<(), DocumentError> {
        let invalid = |message: &str| DocumentError::ValidationFailed(message.into());
        if self.version != 1 {
            return Err(invalid("Unsupported diagram view version"));
        }
        if self.source.trim().is_empty() || self.views.is_empty() {
            return Err(invalid(
                "Diagram views need a source and at least one named view",
            ));
        }
        for (name, view) in &self.views {
            if name.trim().is_empty() || view.scope.trim().is_empty() {
                return Err(invalid("View name and scope must be nonempty"));
            }
            for (path, position) in &view.positions {
                if path.trim().is_empty() || !position.x.is_finite() || !position.y.is_finite() {
                    return Err(invalid(
                        "View positions require a source identity and finite coordinates",
                    ));
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum DiagramViewOp {
    Replace(DiagramViews),
    SetView {
        name: String,
        view: Option<DiagramView>,
    },
    SetPosition {
        view: String,
        path: String,
        position: Option<DiagramPosition>,
    },
}
impl DocumentOp for DiagramViewOp {}

pub struct DiagramViewDocument {
    id: DocumentId,
    generation: u64,
    data: DiagramViews,
}
impl DiagramViewDocument {
    pub fn new(source: String) -> Self {
        Self {
            id: DocumentId::fresh(),
            generation: 0,
            data: DiagramViews {
                version: 1,
                source,
                views: BTreeMap::from([(
                    "Overview".into(),
                    DiagramView {
                        scope: "/".into(),
                        include_descendants: false,
                        positions: BTreeMap::new(),
                    },
                )]),
            },
        }
    }
    pub fn data(&self) -> &DiagramViews {
        &self.data
    }
}
impl Document for DiagramViewDocument {
    type Op = DiagramViewOp;
    fn id(&self) -> DocumentId {
        self.id
    }
    fn generation(&self) -> u64 {
        self.generation
    }
    fn apply(&mut self, op: Self::Op) -> Result<Self::Op, DocumentError> {
        let invalid = |message: &str| DocumentError::ValidationFailed(message.into());
        let inverse = match op {
            DiagramViewOp::Replace(data) => {
                data.validate()?;
                if data.source != self.data.source {
                    return Err(invalid("View source differs from the bound document"));
                }
                DiagramViewOp::Replace(std::mem::replace(&mut self.data, data))
            }
            DiagramViewOp::SetView { name, view } => {
                if name.trim().is_empty() {
                    return Err(invalid("View name must be nonempty"));
                }
                if let Some(view) = &view {
                    DiagramViews {
                        version: 1,
                        source: self.data.source.clone(),
                        views: BTreeMap::from([(name.clone(), view.clone())]),
                    }
                    .validate()?;
                } else if self.data.views.len() == 1 && self.data.views.contains_key(&name) {
                    return Err(invalid("Cannot remove the last view"));
                }
                let previous = match view {
                    Some(view) => self.data.views.insert(name.clone(), view),
                    None => self.data.views.remove(&name),
                };
                DiagramViewOp::SetView {
                    name,
                    view: previous,
                }
            }
            DiagramViewOp::SetPosition {
                view,
                path,
                position,
            } => {
                if path.trim().is_empty()
                    || position
                        .as_ref()
                        .is_some_and(|p| !p.x.is_finite() || !p.y.is_finite())
                {
                    return Err(invalid(
                        "View positions require a source identity and finite coordinates",
                    ));
                }
                let definition = self
                    .data
                    .views
                    .get_mut(&view)
                    .ok_or_else(|| invalid("Named view does not exist"))?;
                let previous = match position {
                    Some(position) => definition.positions.insert(path.clone(), position),
                    None => definition.positions.remove(&path),
                };
                DiagramViewOp::SetPosition {
                    view,
                    path,
                    position: previous,
                }
            }
        };
        self.generation += 1;
        Ok(inverse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DocumentHost, Mutation};
    #[test]
    fn named_views_are_isolated_reversible_and_validate_source() {
        let mut host = DocumentHost::new(DiagramViewDocument::new("asset://scene".into()));
        host.apply(Mutation::local(DiagramViewOp::SetView {
            name: "Power".into(),
            view: Some(DiagramView {
                scope: "/Power".into(),
                include_descendants: false,
                positions: Default::default(),
            }),
        }))
        .unwrap();
        host.apply(Mutation::local(DiagramViewOp::SetPosition {
            view: "Power".into(),
            path: "/Battery".into(),
            position: Some(DiagramPosition { x: 1.0, y: 2.0 }),
        }))
        .unwrap();
        assert!(host.document().data.views["Overview"].positions.is_empty());
        assert_eq!(
            host.document().data.views["Power"].positions["/Battery"].x,
            1.0
        );
        host.undo().unwrap();
        assert!(host.document().data.views["Power"].positions.is_empty());
        host.redo().unwrap();
        let generation = host.document().generation();
        assert!(
            host.apply(Mutation::local(DiagramViewOp::SetPosition {
                view: "Power".into(),
                path: "/Battery".into(),
                position: Some(DiagramPosition {
                    x: f64::NAN,
                    y: 0.0
                })
            }))
            .is_err()
        );
        let mut other = host.document().data.clone();
        other.source = "asset://other".into();
        assert!(
            host.apply(Mutation::local(DiagramViewOp::Replace(other)))
                .is_err()
        );
        assert_eq!(host.document().generation(), generation);
    }
}
