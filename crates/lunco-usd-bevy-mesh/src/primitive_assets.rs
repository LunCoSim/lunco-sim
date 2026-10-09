//! Content-keyed primitive preparation with native weak asset lifetimes.

use std::sync::{Arc, Weak};

use bevy::{
    asset::StrongHandle,
    platform::collections::HashMap,
    prelude::*,
    tasks::{AsyncComputeTaskPool, Task, futures_lite::future},
};
use lunco_render::RenderQualityProfile;
use lunco_usd_bevy_scene::ShapeDims;

use super::build_primitive_mesh;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct PrimitiveKey {
    kind: u8,
    dimensions: [u64; 2],
    axis: &'static str,
    tessellation: [u32; 5],
}

impl PrimitiveKey {
    fn new(shape: ShapeDims, quality: RenderQualityProfile) -> Self {
        let (kind, dimensions, axis) = match shape {
            ShapeDims::Cube { size } => (0, [size.to_bits(), 0], ""),
            ShapeDims::Sphere { radius } => (1, [radius.to_bits(), 0], ""),
            ShapeDims::Cylinder {
                radius,
                height,
                axis,
            } => (2, [radius.to_bits(), height.to_bits()], axis.as_token()),
            ShapeDims::Cone {
                radius,
                height,
                axis,
            } => (3, [radius.to_bits(), height.to_bits()], axis.as_token()),
            ShapeDims::Capsule {
                radius,
                height,
                axis,
            } => (4, [radius.to_bits(), height.to_bits()], axis.as_token()),
            ShapeDims::Plane {
                width,
                length,
                axis,
            } => (5, [width.to_bits(), length.to_bits()], axis.as_token()),
        };
        Self {
            kind,
            dimensions,
            axis,
            tessellation: [
                quality.primitive_sphere_longitudes,
                quality.primitive_sphere_latitudes,
                quality.primitive_radial_segments,
                quality.primitive_capsule_longitudes,
                quality.primitive_capsule_latitudes,
            ],
        }
    }
}

/// One prim's interest in a content-only build; source admission stays at the
/// caller's stage/path/generation/quality boundary.
pub struct PrimitiveMeshRequest {
    key: PrimitiveKey,
    reader: Arc<()>,
}

enum Product {
    Building(Task<Option<Mesh>>),
    Ready(Weak<StrongHandle>),
    Rejected,
}

struct Entry {
    readers: Weak<()>,
    product: Product,
}

/// The result of polling an admitted primitive request.
pub enum PrimitiveMeshResult {
    Pending,
    Ready(Handle<Mesh>),
    Rejected,
}

/// Shares CPU preparation and immutable GPU mesh identity without pinning an
/// asset after its native handles disappear. Independent source readers retain
/// their own admission fences and may cancel without cancelling another reader.
#[derive(Resource, Default)]
pub struct PrimitiveMeshAssets {
    entries: HashMap<PrimitiveKey, Entry>,
}

impl PrimitiveMeshAssets {
    pub fn request(
        &mut self,
        shape: ShapeDims,
        quality: RenderQualityProfile,
    ) -> PrimitiveMeshRequest {
        let key = PrimitiveKey::new(shape, quality);
        if let Some(entry) = self.entries.get_mut(&key) {
            let usable = match &entry.product {
                Product::Building(_) | Product::Rejected => entry.readers.strong_count() > 0,
                Product::Ready(handle) => handle.strong_count() > 0,
            };
            if usable {
                let reader = entry.readers.upgrade().unwrap_or_else(|| {
                    let reader = Arc::new(());
                    entry.readers = Arc::downgrade(&reader);
                    reader
                });
                return PrimitiveMeshRequest { key, reader };
            }
        }
        let reader = Arc::new(());
        let task =
            AsyncComputeTaskPool::get().spawn(async move { build_primitive_mesh(shape, quality) });
        self.entries.insert(
            key,
            Entry {
                readers: Arc::downgrade(&reader),
                product: Product::Building(task),
            },
        );
        PrimitiveMeshRequest { key, reader }
    }

    pub fn poll(
        &mut self,
        request: &PrimitiveMeshRequest,
        meshes: &mut Assets<Mesh>,
    ) -> PrimitiveMeshResult {
        let Some(entry) = self.entries.get_mut(&request.key) else {
            return PrimitiveMeshResult::Rejected;
        };
        // A request is meaningful only for the build generation it joined.
        if !Weak::ptr_eq(&entry.readers, &Arc::downgrade(&request.reader)) {
            return PrimitiveMeshResult::Rejected;
        }
        match &mut entry.product {
            Product::Building(task) => match future::block_on(future::poll_once(task)) {
                None => PrimitiveMeshResult::Pending,
                Some(None) => {
                    entry.product = Product::Rejected;
                    PrimitiveMeshResult::Rejected
                }
                Some(Some(mesh)) => {
                    let handle = meshes.add(mesh);
                    let Handle::Strong(strong) = &handle else {
                        unreachable!("Assets::add returns a native strong handle");
                    };
                    entry.product = Product::Ready(Arc::downgrade(strong));
                    PrimitiveMeshResult::Ready(handle)
                }
            },
            Product::Ready(handle) => match handle.upgrade() {
                Some(handle) if meshes.contains(Handle::<Mesh>::Strong(handle.clone()).id()) => {
                    PrimitiveMeshResult::Ready(Handle::Strong(handle))
                }
                None => PrimitiveMeshResult::Rejected,
                Some(_) => PrimitiveMeshResult::Rejected,
            },
            Product::Rejected => PrimitiveMeshResult::Rejected,
        }
    }

    /// Rebuild at the existing synchronous quality-edit boundary, once per
    /// distinct shape and tessellation. Pending readers of this same content
    /// consume the resulting asset through their normal admission boundary.
    pub fn resolve_quality_edit(
        &mut self,
        shape: ShapeDims,
        quality: RenderQualityProfile,
        meshes: &mut Assets<Mesh>,
    ) -> Option<Handle<Mesh>> {
        let key = PrimitiveKey::new(shape, quality);
        if let Some(Entry {
            product: Product::Ready(handle),
            ..
        }) = self.entries.get(&key)
        {
            if let Some(handle) = handle.upgrade() {
                let handle = Handle::<Mesh>::Strong(handle);
                return meshes.contains(handle.id()).then_some(handle);
            }
        }
        let mesh = build_primitive_mesh(shape, quality)?;
        let handle = meshes.add(mesh);
        let Handle::Strong(strong) = &handle else {
            unreachable!("Assets::add returns a native strong handle");
        };
        let product = Product::Ready(Arc::downgrade(strong));
        if let Some(entry) = self.entries.get_mut(&key) {
            entry.product = product;
        } else {
            self.entries.insert(
                key,
                Entry {
                    readers: Weak::new(),
                    product,
                },
            );
        }
        Some(handle)
    }

    /// Called on reader/asset retirement, not during steady frames.
    pub fn prune(&mut self) {
        self.entries.retain(|_, entry| {
            entry.readers.strong_count() > 0
                || matches!(&entry.product, Product::Ready(handle) if handle.strong_count() > 0)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready(
        pool: &mut PrimitiveMeshAssets,
        request: &PrimitiveMeshRequest,
        meshes: &mut Assets<Mesh>,
    ) -> Handle<Mesh> {
        for _ in 0..1000 {
            match pool.poll(request, meshes) {
                PrimitiveMeshResult::Ready(handle) => return handle,
                PrimitiveMeshResult::Rejected => panic!("valid primitive rejected"),
                PrimitiveMeshResult::Pending => {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                }
            }
        }
        panic!("primitive worker did not finish");
    }

    #[test]
    fn primitive_preparation_shares_work_and_retires_without_pinning_assets() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        let mut meshes = Assets::<Mesh>::default();
        let mut pool = PrimitiveMeshAssets::default();
        let quality = RenderQualityProfile {
            primitive_sphere_longitudes: 24,
            primitive_sphere_latitudes: 16,
            primitive_radial_segments: 32,
            primitive_capsule_longitudes: 16,
            primitive_capsule_latitudes: 8,
            ..Default::default()
        };
        let shape = ShapeDims::Sphere { radius: 1.0 };
        let first = pool.request(shape, quality);
        let second = pool.request(shape, quality);
        assert!(Arc::ptr_eq(&first.reader, &second.reader));
        assert_eq!(pool.entries.len(), 1);
        drop(first);
        pool.prune();
        let handle = ready(&mut pool, &second, &mut meshes);
        drop(second);
        pool.prune();
        assert_eq!(pool.entries.len(), 1);
        let again = pool.request(shape, quality);
        let reused = ready(&mut pool, &again, &mut meshes);
        assert_eq!(handle.id(), reused.id());
        assert_eq!(
            handle.id(),
            pool.resolve_quality_edit(shape, quality, &mut meshes)
                .unwrap()
                .id()
        );
        let vertices = meshes.get(&handle).unwrap().count_vertices();
        let mut other_quality = quality;
        other_quality.primitive_sphere_longitudes += 1;
        let pending_edit = pool.request(shape, other_quality);
        let edited = pool
            .resolve_quality_edit(shape, other_quality, &mut meshes)
            .unwrap();
        assert_ne!(handle.id(), edited.id());
        assert!(meshes.get(&edited).unwrap().count_vertices() > vertices);
        assert_eq!(meshes.get(&handle).unwrap().count_vertices(), vertices);
        assert_eq!(
            ready(&mut pool, &pending_edit, &mut meshes).id(),
            edited.id()
        );
        drop(pending_edit);
        drop(edited);
        drop(again);
        drop(reused);
        drop(handle);
        pool.prune();
        assert!(pool.entries.is_empty());
        let cancelled = pool.request(shape, quality);
        drop(cancelled);
        pool.prune();
        assert!(pool.entries.is_empty());
        let narrow_collision = ShapeDims::Sphere {
            radius: f64::from_bits(1.0_f64.to_bits() + 1),
        };
        assert_eq!(
            1.0_f32,
            match narrow_collision {
                ShapeDims::Sphere { radius } => radius as f32,
                _ => unreachable!(),
            }
        );
        assert!(PrimitiveKey::new(shape, quality) != PrimitiveKey::new(narrow_collision, quality));
        assert!(PrimitiveKey::new(shape, quality) != PrimitiveKey::new(shape, other_quality));
        let mut invalid = quality;
        invalid.primitive_radial_segments = 2;
        let rejected = pool.request(shape, invalid);
        for _ in 0..1000 {
            match pool.poll(&rejected, &mut meshes) {
                PrimitiveMeshResult::Rejected => return,
                PrimitiveMeshResult::Pending => {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                }
                PrimitiveMeshResult::Ready(_) => panic!("invalid quality admitted"),
            }
        }
        panic!("invalid primitive worker did not finish");
    }
}
