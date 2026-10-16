use crate::math::{Real, Vector};
use crate::query::{PointProjection, PointQuery, PointQueryWithLocation};
use crate::shape::{FeatureId, Tetrahedron, TetrahedronPointLocation};

impl PointQuery for Tetrahedron {
    #[inline]
    fn project_local_point(&self, pt: Vector, solid: bool) -> PointProjection {
        self.project_local_point_and_get_location(pt, solid).0
    }

    #[inline]
    fn project_local_point_and_get_feature(&self, pt: Vector) -> (PointProjection, FeatureId) {
        let (proj, loc) = self.project_local_point_and_get_location(pt, false);
        let feature = match loc {
            TetrahedronPointLocation::OnVertex(i) => FeatureId::Vertex(i),
            TetrahedronPointLocation::OnEdge(i, _) => FeatureId::Edge(i),
            TetrahedronPointLocation::OnFace(i, _) => FeatureId::Face(i),
            TetrahedronPointLocation::OnSolid => unreachable!(),
        };

        (proj, feature)
    }
}

impl PointQueryWithLocation for Tetrahedron {
    type Location = TetrahedronPointLocation;

    #[inline]
    fn project_local_point_and_get_location(
        &self,
        pt: Vector,
        solid: bool,
    ) -> (PointProjection, Self::Location) {
        use crate::shape::{Triangle, TrianglePointLocation};
        let vertices = [self.a, self.b, self.c, self.d];
        let faces = [[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]];
        let opposite = [3, 2, 1, 0];
        let degenerate =
            crate::utils::tetrahedron_orientation(self.a, self.b, self.c, self.d) == 0.0;
        let outside = core::array::from_fn::<_, 4, _>(|i| {
            let [a, b, c] = faces[i].map(|v| vertices[v]);
            let side = crate::utils::tetrahedron_orientation(a, b, c, pt);
            let interior = crate::utils::tetrahedron_orientation(a, b, c, vertices[opposite[i]]);
            (side < 0.0 && interior > 0.0) || (side > 0.0 && interior < 0.0)
        });
        let inside = !degenerate && !outside.iter().any(|outside| *outside);
        if solid && inside {
            return (
                PointProjection::new(true, pt),
                TetrahedronPointLocation::OnSolid,
            );
        }

        // An exterior point projects onto a visible boundary face. Reuse the
        // triangle owner for its vertex/edge/face classification and witnesses;
        // an unsuccessful Voronoi-region test must never classify it as solid.
        let mut best: Option<(Real, PointProjection, TetrahedronPointLocation)> = None;
        for (index, face) in faces.iter().enumerate() {
            if !degenerate && !inside && !outside[index] {
                continue;
            }
            let [a, b, c] = face.map(|v| vertices[v]);
            let (projection, location) =
                Triangle::new(a, b, c).project_local_point_and_get_location(pt, true);
            let distance = (pt - projection.point).length_squared();
            if best
                .as_ref()
                .is_some_and(|(previous, _, _)| distance >= *previous)
            {
                continue;
            }
            let location = match location {
                TrianglePointLocation::OnVertex(vertex) => {
                    TetrahedronPointLocation::OnVertex(face[vertex as usize] as u32)
                }
                TrianglePointLocation::OnEdge(edge, coordinates) => {
                    let [a, b] = match edge {
                        0 => [face[0], face[1]],
                        1 => [face[1], face[2]],
                        2 => [face[0], face[2]],
                        _ => unreachable!(),
                    };
                    let edge = match [a, b] {
                        [0, 1] => 0,
                        [0, 2] => 1,
                        [0, 3] => 2,
                        [1, 2] => 3,
                        [1, 3] => 4,
                        [2, 3] => 5,
                        _ => unreachable!(),
                    };
                    TetrahedronPointLocation::OnEdge(edge, coordinates)
                }
                TrianglePointLocation::OnFace(_, coordinates) => {
                    TetrahedronPointLocation::OnFace(index as u32, coordinates)
                }
                TrianglePointLocation::OnSolid => {
                    unreachable!("a 3D triangle has no solid interior")
                }
            };
            best = Some((distance, projection, location));
        }
        let (_, mut projection, location) =
            best.expect("an exterior or non-solid query has a boundary face");
        projection.is_inside = inside || (degenerate && projection.point == pt);
        (projection, location)
    }
}
