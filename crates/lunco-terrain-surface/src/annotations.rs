//! Persistent terrain-local vector annotations. Producers edit stable segment
//! identities; bounded workers update only intersected index leaves. The render
//! adapter uploads completed dirty texel ranges into the existing data texture.

use crate::{DemHeightField, LodTileOf};
use bevy::math::DVec2;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task, futures_lite::future};
use lunco_materials::float_texture::{FloatTexturePatch, FloatTextureUpdates};
use lunco_materials::{ShaderLook, ShaderLookSourceInterface, TextureLayer};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::Arc;
use wgpu_types::{Extent3d, TextureDimension, TextureFormat};

#[derive(SystemSet, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SurfaceAnnotationSet {
    Prepare,
    Publish,
}

type Pair = [DVec2; 2];
type Key = (Entity, u64);
const IMAGE_WIDTH: usize = 256;
const MAX_TEXELS: usize = IMAGE_WIDTH * 8192;

/// CPU picking geometry and coalesced edits share one source identity.
#[derive(Component, Clone)]
pub struct SurfaceCurveAnnotation {
    pub terrain: Entity,
    pub revision: u64,
    pub streaming: bool,
    pub width_m: f64,
    pub color: LinearRgba,
    segments: Arc<BTreeMap<u64, Pair>>,
    edits: BTreeMap<u64, Option<Pair>>,
    next_id: u64,
}

impl SurfaceCurveAnnotation {
    pub fn new(terrain: Entity, streaming: bool, width_m: f64, color: LinearRgba) -> Self {
        Self {
            terrain,
            revision: 0,
            streaming,
            width_m,
            color,
            segments: Arc::new(BTreeMap::new()),
            edits: BTreeMap::new(),
            next_id: 0,
        }
    }
    pub fn snapshot(
        terrain: Entity,
        revision: u64,
        segments: Vec<Pair>,
        width_m: f64,
        color: LinearRgba,
    ) -> Self {
        let mut result = Self::new(terrain, false, width_m, color);
        for pair in segments {
            let id = result.next_id;
            result.next_id += 1;
            result.set_segment(id, pair);
        }
        result.revision = revision;
        result
    }
    pub fn len(&self) -> usize {
        self.segments.len()
    }
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
    pub fn set_segment(&mut self, id: u64, pair: Pair) {
        if self.segments.get(&id) == Some(&pair) {
            return;
        }
        Arc::make_mut(&mut self.segments).insert(id, pair);
        self.edits.insert(id, Some(pair));
        self.revision += 1;
    }
    pub fn remove_segment(&mut self, id: u64) {
        if self.segments.contains_key(&id) {
            Arc::make_mut(&mut self.segments).remove(&id);
            self.edits.insert(id, None);
            self.revision += 1;
        }
    }
    /// Share canonical geometry with a route-edit worker without copying history
    /// or pending deltas. The commit merges any still-unconsumed source edits.
    pub fn snapshot_for_edit(&self) -> Self {
        Self {
            terrain: self.terrain,
            revision: self.revision,
            streaming: self.streaming,
            width_m: self.width_m,
            color: self.color,
            segments: self.segments.clone(),
            edits: BTreeMap::new(),
            next_id: self.next_id,
        }
    }
    pub fn commit_snapshot_edit(&mut self, mut replacement: Self) {
        let mut edits = std::mem::take(&mut self.edits);
        edits.extend(replacement.edits);
        replacement.edits = edits;
        *self = replacement;
    }
    /// Route edits preserve the identities of unchanged legs, including after
    /// insertion/deletion. This comparison runs once per admitted route edit.
    pub fn replace_snapshot(&mut self, replacement: Self) {
        let key = |pair: Pair| pair.map(|p| p.to_array().map(f64::to_bits));
        let mut previous: BTreeMap<_, Vec<u64>> = BTreeMap::new();
        for (&id, &pair) in self.segments.iter() {
            previous.entry(key(pair)).or_default().push(id);
        }
        let mut retained = BTreeSet::new();
        for pair in Arc::unwrap_or_clone(replacement.segments).into_values() {
            let id = previous
                .get_mut(&key(pair))
                .and_then(Vec::pop)
                .unwrap_or_else(|| {
                    let id = self.next_id;
                    self.next_id += 1;
                    id
                });
            retained.insert(id);
            self.set_segment(id, pair);
        }
        let removed: Vec<_> = self
            .segments
            .keys()
            .filter(|id| !retained.contains(id))
            .copied()
            .collect();
        for id in removed {
            self.remove_segment(id);
        }
        self.width_m = replacement.width_m;
        self.color = replacement.color;
        self.revision = replacement.revision;
    }
    pub fn distance(&self, point: DVec2) -> f64 {
        self.segments
            .values()
            .map(|pair| {
                let delta = pair[1] - pair[0];
                let t = ((point - pair[0]).dot(delta) / delta.length_squared()).clamp(0.0, 1.0);
                (point - pair[0] - t * delta).length()
            })
            .fold(f64::INFINITY, f64::min)
    }
}

#[derive(Resource, Clone, PartialEq)]
pub struct SurfaceAnnotationSettings {
    pub grid_resolution: usize,
    pub max_cell_segments: usize,
    pub max_segments: usize,
    pub max_index_nodes: usize,
    pub max_index_depth: usize,
    pub max_index_references: usize,
    pub max_active_builds: usize,
}
impl Default for SurfaceAnnotationSettings {
    fn default() -> Self {
        Self {
            grid_resolution: 32,
            max_cell_segments: 64,
            max_segments: 262144,
            max_index_nodes: 65536,
            max_index_depth: 12,
            max_index_references: 1048576,
            max_active_builds: 2,
        }
    }
}
impl SurfaceAnnotationSettings {
    fn validate(&self) -> Result<(), String> {
        if !(1..=64).contains(&self.grid_resolution)
            || !(1..=256).contains(&self.max_cell_segments)
            || !(1..=262144).contains(&self.max_segments)
            || !(self.grid_resolution * self.grid_resolution..=65536)
                .contains(&self.max_index_nodes)
            || !(1..=12).contains(&self.max_index_depth)
            || !(1..=1048576).contains(&self.max_index_references)
            || !(1..=2).contains(&self.max_active_builds)
        {
            return Err("invalid surface annotation preparation bounds".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Default)]
pub struct AnnotationWork {
    pub updated_segments: usize,
    pub touched_nodes: usize,
    pub patch_bytes: usize,
    pub full_uploads: u64,
    pub upload_sequence: u64,
}
#[derive(Clone)]
pub struct PublishedSurfaceAnnotations {
    pub sources: Vec<(Entity, u64)>,
    pub image: Option<Handle<Image>>,
    pub error: Option<String>,
    pub work: AnnotationWork,
    pub source_work: BTreeMap<Entity, AnnotationWork>,
}
#[derive(Resource, Default)]
pub struct SurfaceAnnotationImages {
    pub published: HashMap<Entity, PublishedSurfaceAnnotations>,
    owners: BTreeMap<Entity, TerrainIndex>,
    source_owners: HashMap<Entity, Entity>,
}
struct TerrainIndex {
    index: Option<Index>,
    task: Option<Task<Build>>,
    pending: BTreeMap<Entity, SourcePatch>,
    generation: u64,
    half: f64,
    settings: SurfaceAnnotationSettings,
    reset: bool,
    completed: Option<(u64, Completion)>,
    resident_height: usize,
    desired: BTreeMap<Entity, u64>,
    failed: bool,
    full_initialization: bool,
}
#[derive(Clone)]
struct SourcePatch {
    entity: Entity,
    revision: u64,
    width: f64,
    color: LinearRgba,
    clear: bool,
    remove: bool,
    edits: BTreeMap<u64, Option<Pair>>,
}
impl SourcePatch {
    fn removed(entity: Entity) -> Self {
        Self {
            entity,
            revision: 0,
            width: 1.0,
            color: LinearRgba::WHITE,
            clear: true,
            remove: true,
            edits: BTreeMap::new(),
        }
    }
}
struct Completion {
    result: Result<AnnotationWork, String>,
    sources: Vec<(Entity, u64)>,
    upload: Option<PreparedUpload>,
}
struct PreparedUpload {
    height: usize,
    full: Option<Vec<u8>>,
    texels: BTreeMap<usize, [f32; 4]>,
}
struct Build {
    index: Index,
    generation: u64,
    completion: Completion,
}

#[derive(Clone)]
struct Segment {
    pair: Pair,
    radius: f64,
    color: LinearRgba,
    slot: usize,
}
#[derive(Clone)]
struct Node {
    lo: DVec2,
    hi: DVec2,
    depth: usize,
    count: usize,
    children: Option<usize>,
    members: BTreeSet<usize>,
    references: Option<(usize, usize)>,
}
impl Node {
    fn new(lo: DVec2, hi: DVec2, depth: usize) -> Self {
        Self {
            lo,
            hi,
            depth,
            count: 0,
            children: None,
            members: BTreeSet::new(),
            references: None,
        }
    }
}
struct Index {
    half: f64,
    settings: SurfaceAnnotationSettings,
    texels: Vec<[f32; 4]>,
    free: BTreeMap<usize, BTreeSet<usize>>,
    nodes: BTreeMap<usize, Node>,
    segments: BTreeMap<Key, Segment>,
    slots: BTreeMap<usize, Key>,
    sources: BTreeMap<Entity, (u64, f64, LinearRgba)>,
    dirty: BTreeSet<usize>,
    reference_capacity: usize,
    source_work: BTreeMap<Entity, AnnotationWork>,
}
#[derive(Default)]
struct Undo {
    nodes: BTreeMap<usize, Option<Node>>,
    segments: BTreeMap<Key, Option<Segment>>,
    texels: BTreeMap<usize, [f32; 4]>,
    free: Vec<(usize, usize, bool)>,
    length: usize,
    reference_capacity: usize,
    touched_nodes: BTreeSet<usize>,
    touched_texels: BTreeSet<usize>,
}
impl Index {
    fn new(half: f64, settings: SurfaceAnnotationSettings) -> Result<Self, String> {
        settings.validate()?;
        if !half.is_finite() || half <= 0.0 || !(half as f32).is_finite() {
            return Err("surface annotation terrain extent is invalid".into());
        }
        let grid = settings.grid_resolution;
        let mut result = Self {
            half,
            settings,
            texels: vec![[0.0; 4]; 2 + grid * grid],
            free: BTreeMap::new(),
            nodes: BTreeMap::new(),
            segments: BTreeMap::new(),
            slots: BTreeMap::new(),
            sources: BTreeMap::new(),
            dirty: (0..2 + grid * grid).collect(),
            reference_capacity: 0,
            source_work: BTreeMap::new(),
        };
        result.texels[0] = [-half as f32, -half as f32, half as f32, half as f32];
        result.texels[1] = [
            grid as f32,
            result.settings.max_index_depth as f32,
            0.0,
            0.0,
        ];
        let size = 2.0 * half / grid as f64;
        for z in 0..grid {
            for x in 0..grid {
                let lo = DVec2::splat(-half) + DVec2::new(x as f64, z as f64) * size;
                result
                    .nodes
                    .insert(2 + z * grid + x, Node::new(lo, lo + DVec2::splat(size), 0));
            }
        }
        Ok(result)
    }
    fn backup_node(&self, id: usize, undo: &mut Undo) {
        undo.touched_nodes.insert(id);
        undo.nodes
            .entry(id)
            .or_insert_with(|| self.nodes.get(&id).cloned());
    }
    fn write(&mut self, id: usize, value: [f32; 4], undo: &mut Undo) {
        if self.texels[id] == value {
            return;
        }
        undo.touched_texels.insert(id);
        undo.texels.entry(id).or_insert(self.texels[id]);
        self.texels[id] = value;
        self.dirty.insert(id);
    }
    fn allocate(&mut self, length: usize, undo: &mut Undo) -> Result<usize, String> {
        if let Some(address) = self.free.get_mut(&length).and_then(BTreeSet::pop_first) {
            undo.free.push((length, address, true));
            return Ok(address);
        }
        let address = self.texels.len();
        if address + length > MAX_TEXELS {
            return Err("surface annotation texture budget exceeded".into());
        }
        self.texels.resize(address + length, [0.0; 4]);
        Ok(address)
    }
    fn release(&mut self, address: usize, length: usize, undo: &mut Undo) {
        self.free.entry(length).or_default().insert(address);
        undo.free.push((length, address, false));
    }
    fn leaf(&mut self, id: usize, undo: &mut Undo) -> Result<(), String> {
        self.backup_node(id, undo);
        let node = self.nodes[&id].clone();
        let count = node.members.len();
        let required = count.max(1).next_power_of_two();
        let old = node.references;
        let block = if count == 0 {
            if let Some((address, capacity)) = old {
                self.release(address, capacity, undo);
                self.reference_capacity -= capacity;
            }
            None
        } else if old.is_some_and(|(_, capacity)| capacity >= required) {
            old
        } else {
            let old_capacity = old.map_or(0, |(_, n)| n);
            if self.reference_capacity - old_capacity + required
                > self.settings.max_index_references
            {
                return Err("surface annotation reference budget exceeded".into());
            }
            let address = self.allocate(required, undo)?;
            if let Some((old, capacity)) = old {
                self.release(old, capacity, undo);
            }
            self.reference_capacity = self.reference_capacity - old_capacity + required;
            Some((address, required))
        };
        self.nodes.get_mut(&id).unwrap().references = block;
        if let Some((address, _)) = block {
            for (i, slot) in node.members.into_iter().enumerate() {
                self.write(address + i, [slot as f32, 0.0, 0.0, 0.0], undo);
            }
        }
        self.write(
            id,
            [
                block.map_or(0, |(address, _)| address) as f32,
                count as f32,
                0.0,
                0.0,
            ],
            undo,
        );
        Ok(())
    }
    fn intersects(segment: &Segment, node: &Node) -> bool {
        segment_intersects_box(
            segment.pair[0],
            segment.pair[1],
            node.lo - DVec2::splat(segment.radius),
            node.hi + DVec2::splat(segment.radius),
        )
    }
    fn insert_node(
        &mut self,
        id: usize,
        slot: usize,
        segment: &Segment,
        undo: &mut Undo,
    ) -> Result<(), String> {
        if !Self::intersects(segment, &self.nodes[&id]) {
            return Ok(());
        }
        self.backup_node(id, undo);
        self.nodes.get_mut(&id).unwrap().count += 1;
        if let Some(children) = self.nodes[&id].children {
            for child in children..children + 4 {
                self.insert_node(child, slot, segment, undo)?;
            }
            return Ok(());
        }
        self.nodes.get_mut(&id).unwrap().members.insert(slot);
        if self.nodes[&id].members.len() <= self.settings.max_cell_segments {
            return self.leaf(id, undo);
        }
        let node = self.nodes[&id].clone();
        if node.depth == self.settings.max_index_depth {
            return Err("surface annotation cell density exceeds subdivision depth".into());
        }
        if self.nodes.len() + 4 > self.settings.max_index_nodes {
            return Err("surface annotation index node budget exceeded".into());
        }
        let children = self.allocate(4, undo)?;
        if let Some((address, capacity)) = node.references {
            self.release(address, capacity, undo);
            self.reference_capacity -= capacity;
        }
        let parent = self.nodes.get_mut(&id).unwrap();
        parent.children = Some(children);
        parent.references = None;
        parent.members.clear();
        let half = (node.hi - node.lo) * 0.5;
        for quadrant in 0..4 {
            let lo = node.lo + DVec2::new((quadrant % 2) as f64, (quadrant / 2) as f64) * half;
            self.backup_node(children + quadrant, undo);
            self.nodes.insert(
                children + quadrant,
                Node::new(lo, lo + half, node.depth + 1),
            );
            self.write(children + quadrant, [0.0; 4], undo);
        }
        self.write(id, [children as f32, -1.0, 0.0, 0.0], undo);
        for member in node.members {
            let value = if member == slot {
                segment.clone()
            } else {
                self.segments[&self.slots[&member]].clone()
            };
            for child in children..children + 4 {
                self.insert_node(child, member, &value, undo)?;
            }
        }
        Ok(())
    }
    fn remove_node(
        &mut self,
        id: usize,
        slot: usize,
        segment: &Segment,
        undo: &mut Undo,
    ) -> Result<(), String> {
        if !Self::intersects(segment, &self.nodes[&id]) {
            return Ok(());
        }
        self.backup_node(id, undo);
        self.nodes.get_mut(&id).unwrap().count -= 1;
        if let Some(children) = self.nodes[&id].children {
            for child in children..children + 4 {
                self.remove_node(child, slot, segment, undo)?;
            }
            if self.nodes[&id].count == 0 {
                // Empty descendants have already collapsed and released their leaf lists.
                for child in children..children + 4 {
                    self.backup_node(child, undo);
                    self.nodes.remove(&child);
                }
                self.release(children, 4, undo);
                self.nodes.get_mut(&id).unwrap().children = None;
                self.write(id, [0.0; 4], undo);
            }
            return Ok(());
        }
        self.nodes.get_mut(&id).unwrap().members.remove(&slot);
        self.leaf(id, undo)
    }
    fn roots(&self, segment: &Segment) -> Vec<usize> {
        let grid = self.settings.grid_resolution;
        let size = 2.0 * self.half / grid as f64;
        let lower = ((segment.pair[0].min(segment.pair[1]) - DVec2::splat(segment.radius)
            + DVec2::splat(self.half))
            / size)
            .floor();
        let upper = ((segment.pair[0].max(segment.pair[1])
            + DVec2::splat(segment.radius + self.half))
            / size)
            .floor();
        let mut result = Vec::new();
        for z in (lower.y.max(0.0) as usize)..=(upper.y.max(0.0) as usize).min(grid - 1) {
            for x in (lower.x.max(0.0) as usize)..=(upper.x.max(0.0) as usize).min(grid - 1) {
                result.push(2 + z * grid + x);
            }
        }
        result
    }
    fn update(
        &mut self,
        key: Key,
        pair: Option<Pair>,
        width: f64,
        color: LinearRgba,
        undo: &mut Undo,
    ) -> Result<bool, String> {
        let old = self.segments.get(&key).cloned();
        if old.as_ref().is_some_and(|old| {
            Some(old.pair) == pair && old.radius == width * 0.5 && old.color == color
        }) {
            return Ok(false);
        }
        if old.is_none() && pair.is_none() {
            return Ok(false);
        } // coalesced insert followed by retirement
        if let Some(pair) = pair {
            validate_segment(pair, width, color, self.half)?;
        }
        undo.segments.entry(key).or_insert(old.clone());
        if let Some(old) = &old {
            for root in self.roots(old) {
                self.remove_node(root, old.slot, old, undo)?;
            }
            self.segments.remove(&key);
            self.slots.remove(&old.slot);
        }
        if let Some(pair) = pair {
            if self.segments.len() == self.settings.max_segments {
                return Err("surface annotation segment budget exceeded".into());
            }
            let slot = if let Some(old) = old {
                old.slot
            } else {
                self.allocate(3, undo)?
            };
            let segment = Segment {
                pair,
                radius: width * 0.5,
                color,
                slot,
            };
            self.segments.insert(key, segment.clone());
            self.slots.insert(slot, key);
            self.write(
                slot,
                [
                    pair[0].x as f32,
                    pair[0].y as f32,
                    pair[1].x as f32,
                    pair[1].y as f32,
                ],
                undo,
            );
            self.write(slot + 1, [segment.radius as f32, 0.0, 0.0, 0.0], undo);
            self.write(slot + 2, color.to_f32_array(), undo);
            for root in self.roots(&segment) {
                self.insert_node(root, slot, &segment, undo)?;
            }
        } else if let Some(old) = old {
            self.release(old.slot, 3, undo);
        }
        Ok(true)
    }
    fn apply(&mut self, patches: Vec<SourcePatch>) -> Result<AnnotationWork, String> {
        let _span = info_span!("surface_annotation_incremental_worker").entered();
        let mut undo = Undo {
            length: self.texels.len(),
            reference_capacity: self.reference_capacity,
            ..default()
        };
        let old_dirty = self.dirty.clone();
        let old_sources = self.sources.clone();
        let old_work = self.source_work.clone();
        let mut work = AnnotationWork::default();
        let result = (|| {
            for patch in patches {
                undo.touched_nodes.clear();
                undo.touched_texels.clear();
                let before = work.updated_segments;
                if patch.clear || patch.remove {
                    let ids: Vec<_> = self
                        .segments
                        .range((patch.entity, 0)..=(patch.entity, u64::MAX))
                        .map(|(key, _)| *key)
                        .collect();
                    for key in ids {
                        work.updated_segments += usize::from(self.update(
                            key,
                            None,
                            patch.width,
                            patch.color,
                            &mut undo,
                        )?);
                    }
                }
                if patch.remove {
                    self.sources.remove(&patch.entity);
                    self.source_work.remove(&patch.entity);
                    continue;
                }
                let style_changed =
                    self.sources
                        .get(&patch.entity)
                        .is_some_and(|(_, width, color)| {
                            *width != patch.width || *color != patch.color
                        });
                if style_changed {
                    let values: Vec<_> = self
                        .segments
                        .range((patch.entity, 0)..=(patch.entity, u64::MAX))
                        .map(|(key, value)| (*key, value.pair))
                        .collect();
                    for (key, pair) in values {
                        work.updated_segments += usize::from(self.update(
                            key,
                            Some(pair),
                            patch.width,
                            patch.color,
                            &mut undo,
                        )?);
                    }
                }
                for (id, pair) in patch.edits {
                    work.updated_segments += usize::from(self.update(
                        (patch.entity, id),
                        pair,
                        patch.width,
                        patch.color,
                        &mut undo,
                    )?);
                }
                self.sources
                    .insert(patch.entity, (patch.revision, patch.width, patch.color));
                self.source_work.insert(
                    patch.entity,
                    AnnotationWork {
                        updated_segments: work.updated_segments - before,
                        touched_nodes: undo.touched_nodes.len(),
                        patch_bytes: undo.touched_texels.len() * 16,
                        full_uploads: 0,
                        upload_sequence: 0,
                    },
                );
            }
            Ok(())
        })();
        if let Err(error) = result {
            for (id, old) in undo.nodes {
                if let Some(old) = old {
                    self.nodes.insert(id, old);
                } else {
                    self.nodes.remove(&id);
                }
            }
            for key in undo.segments.keys() {
                if let Some(current) = self.segments.remove(key) {
                    self.slots.remove(&current.slot);
                }
            }
            for (key, old) in undo.segments {
                if let Some(old) = old {
                    self.slots.insert(old.slot, key);
                    self.segments.insert(key, old);
                }
            }
            for (id, value) in undo.texels {
                self.texels[id] = value;
            }
            for (length, address, removed) in undo.free.into_iter().rev() {
                if removed {
                    self.free.entry(length).or_default().insert(address);
                } else if let Some(free) = self.free.get_mut(&length) {
                    free.remove(&address);
                }
            }
            self.texels.truncate(undo.length);
            self.reference_capacity = undo.reference_capacity;
            self.sources = old_sources;
            self.source_work = old_work;
            self.dirty = old_dirty;
            return Err(error);
        }
        // Removal and reinsertion can visit a leaf without changing its final data.
        for (id, old) in &undo.texels {
            if !old_dirty.contains(id) && self.texels[*id] == *old {
                self.dirty.remove(id);
            }
        }
        work.touched_nodes = undo.nodes.len();
        work.patch_bytes = self.dirty.len() * 16;
        Ok(work)
    }
}

fn validate_segment(pair: Pair, width: f64, color: LinearRgba, half: f64) -> Result<(), String> {
    let radius = width * 0.5;
    if !width.is_finite()
        || width <= 0.0
        || !color
            .to_f32_array()
            .iter()
            .all(|v| v.is_finite() && *v >= 0.0)
        || color.alpha > 1.0
    {
        return Err("surface annotation width and colour must be finite".into());
    }
    if !pair.iter().all(|p| p.is_finite()) || pair[0].distance_squared(pair[1]) <= 1e-18 {
        return Err("surface annotation needs finite, distinct segment endpoints".into());
    }
    for value in pair.into_iter().flat_map(|p| p.to_array()) {
        let narrowed = value as f32;
        if !narrowed.is_finite() || (value - f64::from(narrowed)).abs() > radius * 0.01 {
            return Err("surface annotation coordinates exceed render precision".into());
        }
        if value.abs() + radius > half {
            return Err("surface annotation exceeds terrain coverage".into());
        }
    }
    if !(radius as f32).is_finite()
        || radius as f32 <= 0.0
        || pair[0].as_vec2() == pair[1].as_vec2()
    {
        return Err("surface annotation segment exceeds render precision".into());
    }
    Ok(())
}
fn segment_intersects_box(a: DVec2, b: DVec2, min: DVec2, max: DVec2) -> bool {
    let delta = b - a;
    let (mut enter, mut leave) = (0.0f64, 1.0f64);
    for axis in 0..2 {
        if delta[axis].abs() < 1e-18 {
            if a[axis] < min[axis] || a[axis] > max[axis] {
                return false;
            }
        } else {
            let t0 = (min[axis] - a[axis]) / delta[axis];
            let t1 = (max[axis] - a[axis]) / delta[axis];
            enter = enter.max(t0.min(t1));
            leave = leave.min(t0.max(t1));
        }
    }
    enter <= leave
}

pub(crate) fn prepare_surface_annotations(
    mut annotations: Query<(Entity, &mut SurfaceCurveAnnotation)>,
    mut removed: RemovedComponents<SurfaceCurveAnnotation>,
    terrains: Query<(Entity, &DemHeightField)>,
    interfaces: Query<Ref<ShaderLookSourceInterface>>,
    settings: Res<SurfaceAnnotationSettings>,
    mut state: ResMut<SurfaceAnnotationImages>,
    mut uploads: ResMut<FloatTextureUpdates>,
) {
    let retired: Vec<_> = state
        .owners
        .keys()
        .filter(|t| !terrains.contains(**t))
        .copied()
        .collect();
    for terrain in retired {
        state.owners.remove(&terrain);
        if let Some(old) = state.published.remove(&terrain).and_then(|p| p.image) {
            uploads.retire(old.id());
        }
        state.source_owners.retain(|_, owner| *owner != terrain);
    }
    for entity in removed.read() {
        if let Some(terrain) = state.source_owners.remove(&entity)
            && let Some(owner) = state.owners.get_mut(&terrain)
        {
            owner.generation += 1;
            owner.desired.remove(&entity);
            owner.pending.insert(entity, SourcePatch::removed(entity));
        }
    }
    // Consume completion before admitting another serial batch for this owner.
    let mut completed = Vec::new();
    for (&terrain, owner) in &mut state.owners {
        if let Some(task) = &mut owner.task
            && let Some(build) = future::block_on(future::poll_once(task))
        {
            completed.push((terrain, build));
        }
    }
    for (terrain, build) in completed {
        let owner = state.owners.get_mut(&terrain).unwrap();
        owner.task = None;
        let current = build.generation == owner.generation && !owner.reset;
        owner.index = Some(build.index);
        owner.failed |= build.completion.result.is_err();
        if !current {
            continue;
        }
        // Publication is committed by Publish; keep the result with the owner.
        owner.completed = Some((build.generation, build.completion));
    }
    // A failed transaction is retried only after an explicit source change.
    // Re-admit canonical geometry then: its previous deltas were rolled back.
    let changed: BTreeSet<_> = annotations
        .iter_mut()
        .filter(|(_, a)| a.is_changed())
        .map(|(_, a)| a.terrain)
        .collect();
    for (&terrain, owner) in &mut state.owners {
        if owner.failed && (changed.contains(&terrain) || !owner.pending.is_empty()) {
            owner.failed = false;
            owner.reset = true;
            owner.full_initialization = true;
            owner.completed = None;
            owner.pending.clear();
            owner.generation += 1;
        }
    }
    // Extent/settings changes are explicit full initializations. Elevation and
    // camera/LOD changes do not affect this terrain-local horizontal index.
    for (&terrain, owner) in &mut state.owners {
        if let Ok((_, field)) = terrains.get(terrain) {
            let half = f64::from(field.0.half_extent());
            let interface_changed = interfaces.get(terrain).is_ok_and(|i| i.is_changed());
            if half != owner.half || owner.settings != *settings || interface_changed {
                owner.generation += 1;
                owner.half = half;
                owner.settings = settings.clone();
                owner.reset = true;
                owner.full_initialization = true;
            }
        }
    }
    for (entity, mut annotation) in &mut annotations {
        let reset = state
            .owners
            .get(&annotation.terrain)
            .is_some_and(|o| o.reset);
        if !annotation.is_changed()
            && !reset
            && state.source_owners.get(&entity) == Some(&annotation.terrain)
        {
            continue;
        }
        let Ok((_, field)) = terrains.get(annotation.terrain) else {
            continue;
        };
        let new_source = state.source_owners.get(&entity) != Some(&annotation.terrain);
        if let Some(previous) = state.source_owners.insert(entity, annotation.terrain)
            && previous != annotation.terrain
            && let Some(owner) = state.owners.get_mut(&previous)
        {
            owner.generation += 1;
            owner.desired.remove(&entity);
            owner.pending.insert(entity, SourcePatch::removed(entity));
        }
        let owner = state
            .owners
            .entry(annotation.terrain)
            .or_insert_with(|| TerrainIndex {
                index: None,
                task: None,
                pending: BTreeMap::new(),
                generation: 1,
                half: f64::from(field.0.half_extent()),
                settings: settings.clone(),
                reset: false,
                completed: None,
                resident_height: 0,
                desired: BTreeMap::new(),
                failed: false,
                full_initialization: true,
            });
        owner.desired.insert(entity, annotation.revision);
        if !annotation.streaming || new_source {
            owner.generation += 1;
        }
        let delta = std::mem::take(&mut annotation.bypass_change_detection().edits);
        let edits = if reset || new_source {
            annotation
                .segments
                .iter()
                .map(|(&id, &pair)| (id, Some(pair)))
                .collect()
        } else {
            delta
        };
        let patch = SourcePatch {
            entity,
            revision: annotation.revision,
            width: annotation.width_m,
            color: annotation.color,
            clear: reset,
            remove: false,
            edits,
        };
        if let Some(previous) = owner.pending.get_mut(&entity) {
            previous.revision = patch.revision;
            previous.width = patch.width;
            previous.color = patch.color;
            previous.clear |= patch.clear || previous.remove;
            previous.remove = false;
            previous.edits.extend(patch.edits);
        } else {
            owner.pending.insert(entity, patch);
        }
    }
    // Last-source removal is immediate and cancels outstanding preparation.
    let empty: Vec<_> = state
        .owners
        .keys()
        .filter(|t| !state.source_owners.values().any(|owner| owner == *t))
        .copied()
        .collect();
    for terrain in empty {
        state.owners.remove(&terrain);
        if let Some(image) = state.published.remove(&terrain).and_then(|p| p.image) {
            uploads.retire(image.id());
        }
    }
    // Source changes admitted in this pass also fence just-completed work.
    // Keep its dirty addresses until a current completion actually publishes.
    for owner in state.owners.values_mut() {
        if owner
            .completed
            .as_ref()
            .is_some_and(|(generation, _)| *generation != owner.generation || owner.reset)
        {
            owner.completed = None;
        } else if owner
            .completed
            .as_ref()
            .is_some_and(|(_, c)| c.result.is_ok())
        {
            owner.index.as_mut().unwrap().dirty.clear();
        }
    }
    let mut active = state.owners.values().filter(|o| o.task.is_some()).count();
    for (&terrain, owner) in &mut state.owners {
        if owner.task.is_some() || owner.pending.is_empty() || owner.completed.is_some() {
            continue;
        }
        if interfaces.get(terrain).is_err() {
            continue;
        } // Shader admission has not resolved yet.
        let error = settings.validate().err().or_else(|| {
            interfaces
                .get(terrain)
                .ok()
                .filter(|i| {
                    i.source_valid && i.capabilities.contains("lunco.surface-annotations.v1")
                })
                .is_none()
                .then(|| "terrain shader does not implement lunco.surface-annotations.v1".into())
        });
        if let Some(error) = error {
            owner.failed = true;
            owner.completed = Some((
                owner.generation,
                Completion {
                    result: Err(error),
                    sources: owner.desired.iter().map(|(&e, &r)| (e, r)).collect(),
                    upload: None,
                },
            ));
            owner.pending.clear();
            continue;
        }
        if active >= settings.max_active_builds {
            continue;
        }
        let retired_index = if owner.reset {
            owner.index.take()
        } else {
            None
        };
        let index = if owner.reset {
            owner.reset = false;
            Index::new(owner.half, owner.settings.clone())
        } else {
            owner
                .index
                .take()
                .map(Ok)
                .unwrap_or_else(|| Index::new(owner.half, owner.settings.clone()))
        };
        let mut index = match index {
            Ok(index) => index,
            Err(error) => {
                owner.failed = true;
                owner.completed = Some((
                    owner.generation,
                    Completion {
                        result: Err(error),
                        sources: owner.desired.iter().map(|(&e, &r)| (e, r)).collect(),
                        upload: None,
                    },
                ));
                owner.pending.clear();
                continue;
            }
        };
        let generation = owner.generation;
        let sources = owner.desired.iter().map(|(&e, &r)| (e, r)).collect();
        let patches = std::mem::take(&mut owner.pending).into_values().collect();
        let resident_height = owner.resident_height;
        let initialize = owner.full_initialization;
        owner.task = Some(AsyncComputeTaskPool::get().spawn(async move {
            drop(retired_index);
            let mut result = index.apply(patches);
            let upload = if result.is_ok() && !index.segments.is_empty() {
                let required = index.texels.len().div_ceil(IMAGE_WIDTH).next_power_of_two();
                let height = required.max(resident_height);
                let full = if initialize || required > resident_height {
                    let mut bytes: Vec<_> = index
                        .texels
                        .iter()
                        .flat_map(|value| value.iter().flat_map(|v| v.to_le_bytes()))
                        .collect();
                    bytes.resize(height * IMAGE_WIDTH * 16, 0);
                    Some(bytes)
                } else {
                    None
                };
                let texels = if full.is_none() {
                    index
                        .dirty
                        .iter()
                        .map(|&id| (id, index.texels[id]))
                        .collect()
                } else {
                    BTreeMap::new()
                };
                if let Ok(work) = &mut result {
                    work.patch_bytes = full.as_ref().map_or(texels.len() * 16, Vec::len);
                }
                Some(PreparedUpload {
                    height,
                    full,
                    texels,
                })
            } else {
                None
            };
            Build {
                index,
                generation,
                completion: Completion {
                    result,
                    sources,
                    upload,
                },
            }
        }));
        active += 1;
    }
}

pub(crate) fn publish_surface_annotations(
    mut state: ResMut<SurfaceAnnotationImages>,
    mut images: ResMut<Assets<Image>>,
    mut uploads: ResMut<FloatTextureUpdates>,
    mut looks: Query<(Entity, &mut ShaderLook, Option<&LodTileOf>)>,
) {
    let completed: Vec<_> = state
        .owners
        .iter_mut()
        .filter_map(|(&terrain, owner)| owner.completed.take().map(|work| (terrain, work)))
        .collect();
    for (
        terrain,
        (
            _,
            Completion {
                result,
                sources,
                upload,
            },
        ),
    ) in completed
    {
        let previous = state.published.get(&terrain).cloned();
        let owner = state.owners.get_mut(&terrain).unwrap();
        let mut work = result.clone().unwrap_or_default();
        work.full_uploads = previous.as_ref().map_or(0, |p| p.work.full_uploads);
        let (image, error) = match result {
            Err(error) => {
                owner.resident_height = 0;
                warn!("[surface-annotations] {error}");
                (None, Some(error))
            }
            Ok(_) => {
                if let Some(PreparedUpload {
                    height,
                    full,
                    texels,
                }) = upload
                {
                    let sequence = uploads.reserve_sequence();
                    for work in owner
                        .index
                        .as_mut()
                        .unwrap()
                        .source_work
                        .values_mut()
                        .filter(|work| work.upload_sequence == 0)
                    {
                        work.upload_sequence = sequence;
                    }
                    let size = Extent3d {
                        width: IMAGE_WIDTH as u32,
                        height: height as u32,
                        depth_or_array_layers: 1,
                    };
                    let previous_handle = previous.as_ref().and_then(|p| p.image.clone());
                    let handle = if let Some(bytes) = full {
                        let image = Image::new(
                            size,
                            TextureDimension::D2,
                            bytes,
                            TextureFormat::Rgba32Float,
                            bevy::asset::RenderAssetUsages::default(),
                        );
                        work.full_uploads += 1;
                        owner.full_initialization = false;
                        if let Some(handle) = previous_handle {
                            uploads.retire(handle.id());
                            *images.get_mut(&handle).unwrap() = image;
                            handle
                        } else {
                            images.add(image)
                        }
                    } else {
                        let handle = previous_handle.expect("partial upload has a resident image");
                        uploads.submit(FloatTexturePatch {
                            image: handle.clone(),
                            sequence,
                            size: UVec2::new(IMAGE_WIDTH as u32, height as u32),
                            texels,
                        });
                        handle
                    };
                    owner.resident_height = height;
                    (Some(handle), None)
                } else {
                    owner.resident_height = 0;
                    (None, None)
                }
            }
        };
        if image.is_none()
            && let Some(handle) = previous.and_then(|p| p.image)
        {
            uploads.retire(handle.id());
        }
        let source_work = owner
            .index
            .as_ref()
            .map_or_else(BTreeMap::new, |index| index.source_work.clone());
        state.published.insert(
            terrain,
            PublishedSurfaceAnnotations {
                sources,
                image,
                error,
                work,
                source_work,
            },
        );
    }
    for (entity, mut look, tile) in &mut looks {
        let terrain = tile.map_or(entity, |tile| tile.0);
        if !state.owners.contains_key(&terrain)
            && !look
                .textures
                .contains_key(&TextureLayer::SurfaceAnnotations)
        {
            continue;
        }
        let image = state.published.get(&terrain).and_then(|p| p.image.as_ref());
        if look.textures.get(&TextureLayer::SurfaceAnnotations) == image {
            continue;
        }
        if let Some(image) = image {
            look.textures
                .insert(TextureLayer::SurfaceAnnotations, image.clone());
        } else {
            look.textures.remove(&TextureLayer::SurfaceAnnotations);
        }
    }
}

pub(crate) fn clear_surface_annotations(
    mut state: ResMut<SurfaceAnnotationImages>,
    mut updates: ResMut<FloatTextureUpdates>,
) {
    for image in state.published.values().filter_map(|p| p.image.as_ref()) {
        updates.retire(image.id());
    }
    *state = SurfaceAnnotationImages::default();
}

#[cfg(test)]
mod tests {
    use super::*;
    fn patch(entity: Entity, edits: impl IntoIterator<Item = (u64, Option<Pair>)>) -> SourcePatch {
        SourcePatch {
            entity,
            revision: 1,
            width: 0.2,
            color: LinearRgba::WHITE,
            clear: false,
            remove: false,
            edits: edits.into_iter().collect(),
        }
    }
    fn pair(x: f64) -> Pair {
        [DVec2::new(x, 0.0), DVec2::new(x + 0.5, 0.0)]
    }
    #[test]
    fn surface_annotation_index_edits_are_local_and_transactional() {
        let source = Entity::from_bits(1);
        let mut index = Index::new(3000.0, SurfaceAnnotationSettings::default()).unwrap();
        index
            .apply(vec![patch(
                source,
                (0..10000).map(|id| (id, Some(pair(-2500.0 + id as f64 * 0.5)))),
            )])
            .unwrap();
        let bounds = index.texels[0];
        let old_slot = index.segments[&(source, 0)].slot;
        let head_slot = index.segments[&(source, 9999)].slot;
        let length = index.texels.len();
        index.dirty.clear();
        let mut head = pair(2499.5);
        head[1].x += 0.1;
        let work = index
            .apply(vec![patch(source, [(9999, Some(head))])])
            .unwrap();
        assert_eq!(work.updated_segments, 1);
        assert!(work.touched_nodes < 32, "{}", work.touched_nodes);
        assert!(work.patch_bytes < 4096, "{}", work.patch_bytes);
        assert_eq!(index.texels.len(), length);
        assert_eq!(index.texels[0], bounds);
        assert_eq!(index.segments[&(source, 0)].slot, old_slot);
        assert_eq!(index.segments[&(source, 9999)].slot, head_slot);
        // Reject a late invalid operation after allocating/reusing slots and
        // changing membership. All committed topology and texels must survive.
        index.dirty.clear();
        let before = index.texels.clone();
        let nodes = index.nodes.len();
        let refs = index.reference_capacity;
        assert!(
            index
                .apply(vec![patch(
                    source,
                    [
                        (0, None),
                        (10000, Some(pair(1.0))),
                        (10001, Some(pair(3001.0)))
                    ]
                )])
                .unwrap_err()
                .contains("coverage")
        );
        assert_eq!(index.texels, before);
        assert_eq!(index.nodes.len(), nodes);
        assert_eq!(index.reference_capacity, refs);
        assert_eq!(index.slots[&old_slot], (source, 0));
        assert_eq!(index.segments.len(), 10000);
        assert!(index.dirty.is_empty());
        index.apply(vec![SourcePatch::removed(source)]).unwrap();
        assert!(index.segments.is_empty());
        assert!(index.sources.is_empty());
        assert_eq!(index.reference_capacity, 0);
        assert_eq!(index.nodes.len(), 32 * 32);
        index.dirty.clear();
        index
            .apply(vec![patch(source, [(0, Some(pair(1.0)))])])
            .unwrap();
        assert_eq!(index.texels.len(), length);
        // Recycle record/reference/node blocks into a differently located tree.
        // Every new child, including empty children, must have its current ABI.
        index
            .apply(vec![patch(
                source,
                (1..10001).map(|id| {
                    let mut segment = pair(-2500.0 + (id - 1) as f64 * 0.5);
                    segment[0].y = -1000.0;
                    segment[1].y = -1000.0;
                    (id, Some(segment))
                }),
            )])
            .unwrap();
        for (&address, node) in &index.nodes {
            let encoded = index.texels[address];
            if let Some(children) = node.children {
                assert_eq!(encoded[0], children as f32);
                assert_eq!(encoded[1], -1.0);
            } else {
                assert_eq!(encoded[1], node.members.len() as f32);
            }
        }
        let mut dense = Index::new(
            10.0,
            SurfaceAnnotationSettings {
                grid_resolution: 1,
                max_cell_segments: 1,
                max_index_depth: 1,
                ..default()
            },
        )
        .unwrap();
        assert!(
            dense
                .apply(vec![patch(
                    source,
                    [(0, Some(pair(1.0))), (1, Some(pair(1.0)))]
                )])
                .unwrap_err()
                .contains("density")
        );
        assert!(dense.segments.is_empty());
        assert_eq!(dense.nodes.len(), 1);
        assert!(validate_segment(pair(1.0), 0.0, LinearRgba::WHITE, 10.0).is_err());
        assert!(
            validate_segment(
                [DVec2::splat(f64::NAN), DVec2::ZERO],
                0.2,
                LinearRgba::WHITE,
                10.0
            )
            .is_err()
        );
    }
}
