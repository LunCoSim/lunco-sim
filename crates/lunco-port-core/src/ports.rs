//! Shared **port registry** — the FMI/SSP scalar-exchange surface shared
//! by every participant so they all read/write exposed values through ONE path.
//!
//! A *port* is a named scalar (`f64`) on a participant entity. Modelica variables,
//! avian rigid-body state, joint angles, the SysML/hardware "nervous system"
//! ports, and (in future) an imported FMU all present as ports, so that wires,
//! the API (`ListPorts` / `GetPort` / `SetPorts`), the UI inspector, and every
//! scripting runtime (rhai/python) treat them uniformly — the FMI/SSP contract.
//!
//! ## Ownership
//!
//! Ports are co-sim *substrate*, not an engine or API concern: the wire engine
//! (`lunco-cosim`) runs ON them, the API and scripts merely consume them. Putting
//! the registry here — below every participant — lets each crate **register** its
//! backends downward and **consume** the registry, with nobody depending "up".
//! This is what lets `lunco-scripting` reach ports even though `lunco-cosim`
//! (which owns the avian/joint/Modelica backends) depends ON scripting: both
//! depend down on this module. A future FMU-import or script-defined component is
//! just one more registered backend the wire engine then honours.
//!
//! ## Value model
//!
//! The wire currency is `f64` (continuous Real — what FMI-CS exchanges almost
//! everywhere), and it is the currency end to end: a [`Port`] holds one `f64`,
//! whatever the signal means. A backend whose own storage is narrower converts at
//! its boundary. We deliberately do **not** model `Bool`/`Enum`/`String` ports
//! until a concrete need appears.
//!
//! [`Port`]: crate::Port
//!
//! ## One registry, one discovery path and four thin access operations
//!
//! Every port-bearing backend is one [`PortBackend`] entry with an entity
//! enumerator and the access operations (declare-ports / read-output / read-input /
//! write-input), registered into the [`PortRegistry`] resource. Discovery and
//! access fold over the registered backends in order, so a new backend is added
//! by **registering** it — no consumer changes. Registration order *is*
//! resolution precedence (first match wins).

use bevy::prelude::*;
use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::{Deref, Index, IndexMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::InputPorts;
use lunco_engineering_values::{CoordinateFrameId, UnitReference};

/// Maximum number of entity targets accepted by one atomic scalar-port batch.
pub const MAX_PORT_BATCH_TARGETS: usize = 256;

/// Incrementally maintained identity for a set of port names.
///
/// The key excludes live values and is order independent, so owners can update
/// samples without rebuilding port metadata. Insert/remove operations update
/// this key at the point where the owner changes its declared shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PortNameSetKey {
    sum: u64,
    count: u64,
}

impl Default for PortNameSetKey {
    fn default() -> Self {
        Self {
            sum: 0xcbf29ce484222325,
            count: 0,
        }
    }
}

impl PortNameSetKey {
    /// Add one unique name to this identity.
    #[inline]
    pub fn insert(&mut self, name: &str) {
        self.insert_contribution(port_name_contribution(name));
    }

    #[inline]
    fn insert_contribution(&mut self, contribution: u64) {
        self.sum = self.sum.wrapping_add(contribution);
        self.count = self.count.wrapping_add(1);
    }

    /// Remove one existing name from this identity.
    #[inline]
    pub fn remove(&mut self, name: &str) {
        self.sum = self.sum.wrapping_sub(port_name_contribution(name));
        self.count = self.count.wrapping_sub(1);
    }

    /// Finish the order-independent cache key.
    #[inline]
    pub fn finish(self) -> u64 {
        self.sum ^ self.count.rotate_left(41)
    }
}

#[inline]
fn port_name_contribution(name: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut hasher);
    hasher.finish().rotate_left(17)
}

/// One named value in a dynamic port surface. Retired slots are not reused until
/// compaction changes the map's process-local layout id.
#[derive(Clone, Debug)]
struct PortMapSlot<T: Copy> {
    name: Option<Arc<str>>,
    value: Option<T>,
}

static NEXT_PORT_MAP_LAYOUT_ID: AtomicU32 = AtomicU32::new(1);

fn next_port_map_layout_id() -> u32 {
    NEXT_PORT_MAP_LAYOUT_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("process-local port-map layout identifier space exhausted")
}

/// A dynamic name-to-value map with a compact, process-local slot for hot reads.
///
/// Names remain the authored/API identity. Structural edits update the name
/// index and topology key; compiled consumers resolve a name once and exchange
/// values directly through the slot array. Existing slots survive unrelated
/// additions/removals, while retired slots are never reused before compaction.
/// A process-local layout id rejects handles from a clone, clear, or compaction.
#[derive(Debug)]
pub struct PortMap<T: Copy> {
    by_name: HashMap<Arc<str>, usize>,
    slots: Vec<PortMapSlot<T>>,
    retired_slots: usize,
    names: PortNameSetKey,
    topology_version: u64,
    layout_id: u32,
}

impl<T: Copy> Clone for PortMap<T> {
    fn clone(&self) -> Self {
        Self {
            by_name: self.by_name.clone(),
            slots: self.slots.clone(),
            retired_slots: self.retired_slots,
            names: self.names,
            topology_version: self.topology_version,
            layout_id: next_port_map_layout_id(),
        }
    }
}

impl<T: Copy> Default for PortMap<T> {
    fn default() -> Self {
        Self {
            by_name: HashMap::new(),
            slots: Vec::new(),
            retired_slots: 0,
            names: PortNameSetKey::default(),
            topology_version: 0,
            layout_id: next_port_map_layout_id(),
        }
    }
}

impl<T: Copy + PartialEq> PartialEq for PortMap<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len() == other.len()
            && self
                .iter()
                .all(|(name, value)| other.get(name).is_some_and(|other| value == other))
    }
}

impl<T: Copy> PortMap<T> {
    /// Insert or replace a named value. Name storage is shared between the
    /// lookup index and its slot, so each declared name has one string buffer.
    pub fn insert(&mut self, name: String, value: T) -> Option<T> {
        if let Some(index) = self.by_name.get(name.as_str()).copied() {
            let slot = self.slots.get_mut(index)?;
            return slot.value.replace(value);
        }

        let name: Arc<str> = Arc::from(name);
        self.names.insert(&name);
        let index = self.allocate_slot(Arc::clone(&name), value);
        self.by_name.insert(name, index);
        self.topology_version = self.topology_version.wrapping_add(1);
        None
    }

    fn allocate_slot(&mut self, name: Arc<str>, value: T) -> usize {
        let index = self.slots.len();
        self.slots.push(PortMapSlot {
            name: Some(name),
            value: Some(value),
        });
        index
    }

    /// Read a value by its authored name.
    #[inline]
    pub fn get(&self, name: &str) -> Option<&T> {
        let index = *self.by_name.get(name)?;
        self.slots.get(index)?.value.as_ref()
    }

    /// Mutably access a value by its authored name.
    #[inline]
    pub fn get_mut(&mut self, name: &str) -> Option<&mut T> {
        let index = *self.by_name.get(name)?;
        self.slots.get_mut(index)?.value.as_mut()
    }

    /// Whether this surface declares `name`.
    #[inline]
    pub fn contains_key(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Resolve a declared name to a process-local slot handle.
    #[inline]
    pub fn resolve_slot(&self, name: &str) -> Option<u64> {
        let index = *self.by_name.get(name)?;
        let slot = self.slots.get(index)?;
        slot.value.as_ref()?;
        encode_port_slot(self.layout_id, index)
    }

    /// Return the process-local layout identity used by resolved slots.
    /// Appending names preserves existing handles; clone, clear, and compaction
    /// replace this identity.
    #[inline]
    pub fn layout_key(&self) -> u32 {
        self.layout_id
    }

    /// Read by a previously resolved slot. Removing that entry retires its
    /// handle; compaction and map replacement retire the old layout.
    #[inline]
    pub fn get_slot(&self, handle: u64) -> Option<&T> {
        self.get_slot_entry(handle).map(|(_, value)| value)
    }

    /// Read the name and value at a previously resolved slot.
    #[inline]
    pub fn get_slot_entry(&self, handle: u64) -> Option<(&str, &T)> {
        let (layout_id, index) = decode_port_slot(handle)?;
        if layout_id != self.layout_id {
            return None;
        }
        let slot = self.slots.get(index)?;
        Some((slot.name.as_deref()?, slot.value.as_ref()?))
    }

    /// Mutably access a live value by a previously resolved slot.
    #[inline]
    pub fn get_slot_mut(&mut self, handle: u64) -> Option<&mut T> {
        let (layout_id, index) = decode_port_slot(handle)?;
        if layout_id != self.layout_id {
            return None;
        }
        let slot = self.slots.get_mut(index)?;
        slot.value.as_mut()
    }

    /// Remove a declared name and retire its slot without reusing its index.
    pub fn remove(&mut self, name: &str) -> Option<T> {
        let index = self.by_name.remove(name)?;
        let slot = self.slots.get_mut(index)?;
        let removed_name = slot.name.take()?;
        let value = slot.value.take()?;
        self.names.remove(&removed_name);
        self.retired_slots += 1;
        self.topology_version = self.topology_version.wrapping_add(1);
        if self.retired_slots >= 64 && self.retired_slots > self.by_name.len() {
            self.compact();
        }
        Some(value)
    }

    fn compact(&mut self) {
        let mut slots = Vec::with_capacity(self.by_name.len());
        let mut by_name = HashMap::with_capacity(self.by_name.len());
        for slot in self.slots.drain(..) {
            if let (Some(name), Some(value)) = (slot.name, slot.value) {
                let index = slots.len();
                by_name.insert(Arc::clone(&name), index);
                slots.push(PortMapSlot {
                    name: Some(name),
                    value: Some(value),
                });
            }
        }
        self.slots = slots;
        self.by_name = by_name;
        self.retired_slots = 0;
        self.layout_id = next_port_map_layout_id();
    }

    /// Remove every value and release retired storage. Repeated empty clears
    /// are no-ops.
    pub fn clear(&mut self) {
        if self.by_name.is_empty() && self.slots.is_empty() {
            return;
        }
        self.by_name.clear();
        self.names = PortNameSetKey::default();
        self.slots.clear();
        self.retired_slots = 0;
        self.topology_version = self.topology_version.wrapping_add(1);
        self.layout_id = next_port_map_layout_id();
    }

    /// Number of declared names.
    #[inline]
    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    /// Whether the map has no declared names.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Return the incremental topology identity for this map, including
    /// structural edits that restore the same final set of names.
    #[inline]
    pub fn topology_key(&self) -> u64 {
        self.names.finish()
            ^ self.topology_version.rotate_left(23)
            ^ u64::from(self.layout_id).rotate_left(47)
    }

    /// Iterate live names.
    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.slots.iter().filter_map(|slot| slot.name.as_deref())
    }

    /// Iterate live values.
    pub fn values(&self) -> impl Iterator<Item = &T> {
        self.slots.iter().filter_map(|slot| slot.value.as_ref())
    }

    /// Iterate live name/value pairs.
    pub fn iter(&self) -> PortMapIter<'_, T> {
        PortMapIter {
            inner: self.slots.iter().filter_map(port_map_slot_ref::<T>),
        }
    }

    /// Mutably iterate live values.
    pub fn values_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.slots.iter_mut().filter_map(|slot| slot.value.as_mut())
    }

    /// Mutably iterate live name/value pairs.
    pub fn iter_mut(&mut self) -> PortMapIterMut<'_, T> {
        PortMapIterMut {
            inner: self.slots.iter_mut().filter_map(port_map_slot_mut::<T>),
        }
    }
}

const PORT_SLOT_INDEX_MASK: u64 = u32::MAX as u64;

#[inline]
fn encode_port_slot(layout_id: u32, index: usize) -> Option<u64> {
    if layout_id == 0 {
        return None;
    }
    let index = u32::try_from(index).ok()?;
    Some((u64::from(layout_id) << 32) | u64::from(index))
}

#[inline]
fn decode_port_slot(handle: u64) -> Option<(u32, usize)> {
    let layout_id = (handle >> 32) as u32;
    (layout_id != 0).then_some((layout_id, (handle & PORT_SLOT_INDEX_MASK) as usize))
}

fn port_map_slot_ref<T: Copy>(slot: &PortMapSlot<T>) -> Option<(&str, &T)> {
    Some((slot.name.as_deref()?, slot.value.as_ref()?))
}

fn port_map_slot_mut<T: Copy>(slot: &mut PortMapSlot<T>) -> Option<(&str, &mut T)> {
    Some((slot.name.as_deref()?, slot.value.as_mut()?))
}

type PortMapIterInner<'a, T> = std::iter::FilterMap<
    std::slice::Iter<'a, PortMapSlot<T>>,
    fn(&'a PortMapSlot<T>) -> Option<(&'a str, &'a T)>,
>;
type PortMapIterMutInner<'a, T> = std::iter::FilterMap<
    std::slice::IterMut<'a, PortMapSlot<T>>,
    fn(&'a mut PortMapSlot<T>) -> Option<(&'a str, &'a mut T)>,
>;

/// Iterator over the public entries of a [`PortMap`].
#[doc(hidden)]
pub struct PortMapIter<'a, T: Copy> {
    inner: PortMapIterInner<'a, T>,
}

impl<'a, T: Copy> Iterator for PortMapIter<'a, T> {
    type Item = (&'a str, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

/// Mutable iterator over the public entries of a [`PortMap`].
#[doc(hidden)]
pub struct PortMapIterMut<'a, T: Copy> {
    inner: PortMapIterMutInner<'a, T>,
}

impl<'a, T: Copy> Iterator for PortMapIterMut<'a, T> {
    type Item = (&'a str, &'a mut T);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, T: Copy> IntoIterator for &'a PortMap<T> {
    type Item = (&'a str, &'a T);
    type IntoIter = PortMapIter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T: Copy> IntoIterator for &'a mut PortMap<T> {
    type Item = (&'a str, &'a mut T);
    type IntoIter = PortMapIterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T: Copy> From<HashMap<String, T>> for PortMap<T> {
    fn from(values: HashMap<String, T>) -> Self {
        values.into_iter().collect()
    }
}

impl<T: Copy> FromIterator<(String, T)> for PortMap<T> {
    fn from_iter<I: IntoIterator<Item = (String, T)>>(iter: I) -> Self {
        let mut map = Self::default();
        map.extend(iter);
        map
    }
}

impl<T: Copy> Extend<(String, T)> for PortMap<T> {
    fn extend<I: IntoIterator<Item = (String, T)>>(&mut self, iter: I) {
        for (name, value) in iter {
            self.insert(name, value);
        }
    }
}

impl<T: Copy> Index<&str> for PortMap<T> {
    type Output = T;

    fn index(&self, name: &str) -> &Self::Output {
        self.get(name).expect("no entry found for key")
    }
}

impl<T: Copy> IndexMut<&str> for PortMap<T> {
    fn index_mut(&mut self, name: &str) -> &mut Self::Output {
        self.get_mut(name).expect("no entry found for key")
    }
}

/// Scalar specialization used by Modelica and command-value surfaces.
#[derive(Debug, Reflect)]
#[reflect(opaque)]
pub struct ScalarPortMap {
    map: PortMap<f64>,
    /// Destination handles aligned to the last borrowed sample iteration.
    /// Every reuse checks its live name; these hints never establish identity.
    sample_slots: Vec<u64>,
}

impl Clone for ScalarPortMap {
    fn clone(&self) -> Self {
        Self {
            map: self.map.clone(),
            sample_slots: Vec::new(),
        }
    }
}

impl Default for ScalarPortMap {
    fn default() -> Self {
        Self {
            map: PortMap::default(),
            sample_slots: Vec::new(),
        }
    }
}

impl ScalarPortMap {
    /// Copy borrowed named samples, retaining destination slot hints between
    /// snapshots. Stable names avoid hashing and allocation. Source reordering
    /// and destination edits are resolved against each live name and layout;
    /// source cardinality or iteration order alone never validates a hint.
    /// Returns only sample/topology changes, preserving exact `f64` bits.
    pub fn upsert_samples<'a>(
        &mut self,
        samples: impl Iterator<Item = (&'a str, &'a f64)>,
    ) -> bool {
        let mut changed = false;
        let mut count = 0;
        for (index, (name, value)) in samples.enumerate() {
            count = index + 1;
            let retained = self.sample_slots.get(index).copied().filter(|slot| {
                self.map
                    .get_slot_entry(*slot)
                    .is_some_and(|(current_name, _)| current_name == name)
            });
            match retained {
                Some(slot) => {
                    changed |= self
                        .set_slot_existing(slot, *value)
                        .expect("a checked sample slot stays live during its exclusive copy");
                }
                None => {
                    changed |= self.set(name, *value);
                    let slot = self
                        .resolve_slot(name)
                        .expect("an admitted named sample has a live destination slot");
                    if let Some(retained) = self.sample_slots.get_mut(index) {
                        *retained = slot;
                    } else {
                        self.sample_slots.push(slot);
                    }
                }
            }
        }
        self.sample_slots.truncate(count);
        changed
    }

    /// Set a scalar by borrowed name and report whether its sample or topology
    /// changed. Existing names perform no allocation.
    #[inline]
    pub fn set(&mut self, name: &str, value: f64) -> bool {
        if let Some(current) = self.get_mut(name) {
            if current.to_bits() == value.to_bits() {
                return false;
            }
            *current = value;
            return true;
        }
        self.insert(name.to_owned(), value);
        true
    }

    /// Update a declared scalar without adding a missing name.
    #[inline]
    pub fn set_existing(&mut self, name: &str, value: f64) -> Option<bool> {
        let current = self.get_mut(name)?;
        let changed = current.to_bits() != value.to_bits();
        if changed {
            *current = value;
        }
        Some(changed)
    }

    /// Update a scalar by a previously resolved slot without adding a missing
    /// name, returning `None` when the slot has been retired.
    #[inline]
    pub fn set_slot_existing(&mut self, slot: u64, value: f64) -> Option<bool> {
        let current = self.get_slot_mut(slot)?;
        let changed = current.to_bits() != value.to_bits();
        if changed {
            *current = value;
        }
        Some(changed)
    }
}

impl Deref for ScalarPortMap {
    type Target = PortMap<f64>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl<'a> IntoIterator for &'a ScalarPortMap {
    type Item = (&'a str, &'a f64);
    type IntoIter = PortMapIter<'a, f64>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter()
    }
}

impl<'a> IntoIterator for &'a mut ScalarPortMap {
    type Item = (&'a str, &'a mut f64);
    type IntoIter = PortMapIterMut<'a, f64>;

    fn into_iter(self) -> Self::IntoIter {
        self.map.iter_mut()
    }
}

impl std::ops::DerefMut for ScalarPortMap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.map
    }
}

impl From<HashMap<String, f64>> for ScalarPortMap {
    fn from(values: HashMap<String, f64>) -> Self {
        Self {
            map: values.into(),
            sample_slots: Vec::new(),
        }
    }
}

impl FromIterator<(String, f64)> for ScalarPortMap {
    fn from_iter<I: IntoIterator<Item = (String, f64)>>(iter: I) -> Self {
        Self {
            map: iter.into_iter().collect(),
            sample_slots: Vec::new(),
        }
    }
}

impl Extend<(String, f64)> for ScalarPortMap {
    fn extend<I: IntoIterator<Item = (String, f64)>>(&mut self, iter: I) {
        self.map.extend(iter);
    }
}

impl Index<&str> for ScalarPortMap {
    type Output = f64;

    fn index(&self, name: &str) -> &Self::Output {
        self.get(name).expect("no entry found for key")
    }
}

impl IndexMut<&str> for ScalarPortMap {
    fn index_mut(&mut self, name: &str) -> &mut Self::Output {
        self.get_mut(name).expect("no entry found for key")
    }
}

/// Durable invalidation generation for the shared runtime port surface.
///
/// Port identity is not a sampled value. A consumer may be hidden when an
/// owner changes its declared contract, so a transient event would be lossy.
/// Owner checks advance this monotonic generation after a component's port
/// surface or published metadata changes; consumers retain the last generation they
/// projected and rebuild only when it differs. Live port values must not advance
/// it.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortTopologyRevision(pub u64);

impl PortTopologyRevision {
    /// Advance the invalidation generation after a port surface or contract change.
    #[inline]
    pub fn bump(&mut self) {
        self.0 = self.0.wrapping_add(1);
    }
}

/// Last structural identity observed for each port-owner component and entity.
///
/// A port owner may update one ECS component for both live samples and declared
/// names. The owner-side change check records only the identity key, so a live
/// update still marks the component changed but does not invalidate the port
/// projection. Entity membership is handled by the lifecycle observers below.
#[derive(Resource, Default)]
pub struct PortTopologyState {
    keys: HashMap<(TypeId, Entity), u64>,
}

impl PortTopologyState {
    /// Observe a component-owned structural key, deriving it on first admission
    /// or when that owner changed. The caller must also mark a new containing
    /// owner changed if this component could have been edited outside its scope.
    /// Returns the current key and whether an already-observed key differs.
    pub fn observe_if_changed<T: 'static>(
        &mut self,
        entity: Entity,
        owner_changed: bool,
        derive_key: impl FnOnce() -> u64,
    ) -> (u64, bool) {
        if !owner_changed && let Some(key) = self.keys.get(&(TypeId::of::<T>(), entity)) {
            return (*key, false);
        }
        let key = derive_key();
        (key, self.changed::<T>(entity, key))
    }

    /// Record one structural key and report whether an already-observed key
    /// differs. The first observation seeds the cache; the lifecycle observer
    /// has already published the add as the structural invalidation.
    pub fn changed<T: 'static>(&mut self, entity: Entity, key: u64) -> bool {
        self.keys
            .insert((TypeId::of::<T>(), entity), key)
            .is_some_and(|previous| previous != key)
    }

    /// Forget a removed owner component so an entity id cannot retain a stale
    /// identity if that id is later reused by Bevy.
    pub fn forget<T: 'static>(&mut self, entity: Entity) {
        self.keys.remove(&(TypeId::of::<T>(), entity));
    }
}

/// Advance the port-surface generation when a component-owned backend candidate
/// is added. The owning plugin supplies the observer for the component types it
/// registers in its backend.
pub fn bump_port_topology_on_add<T: Component>(
    _trigger: On<Add, T>,
    mut revision: ResMut<PortTopologyRevision>,
) {
    revision.bump();
}

/// Advance the port-surface generation when a component-owned backend candidate
/// is removed. `On<Remove, T>` also covers entity teardown, so a hidden panel
/// cannot retain a despawned candidate until its next full sample.
pub fn bump_port_topology_on_remove<T: Component>(
    trigger: On<Remove, T>,
    mut revision: ResMut<PortTopologyRevision>,
    mut state: ResMut<PortTopologyState>,
) {
    state.forget::<T>(trigger.entity);
    revision.bump();
}

/// Direction (causality) of a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Reflect)]
pub enum PortDirection {
    /// Port receives values from connections.
    In,
    /// Port provides values to connections.
    Out,
    /// Port can both receive and provide values.
    InOut,
}

/// Runtime value kind carried by a port.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Reflect)]
pub enum PortValueType {
    /// Continuous engineering scalar exchanged as `f64`.
    Scalar,
}

impl PortValueType {
    /// Stable API/UI spelling for this kind.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Scalar => "scalar",
        }
    }
}

/// Metadata describing the value and control contract of a discovered port.
///
/// The runtime currently exposes scalar `f64` values end to end. Keeping that
/// fact here, beside the registry that owns the port surface, gives native UI
/// and API consumers one authoritative place for units, validation, and
/// control ownership instead of making them infer policy from port names.
#[derive(Debug, Clone, PartialEq)]
pub struct PortMetadata {
    /// Stable value kind shown to generic consumers.
    pub value_type: PortValueType,
    /// Resolved engineering unit, when the owner knows its dimension and scale.
    pub unit: Option<UnitReference>,
    /// Coordinate frame of this scalar, when it represents a frame-bound value.
    pub frame: Option<CoordinateFrameId>,
    /// Inclusive lower validation bound, if one exists.
    pub min: Option<f64>,
    /// Inclusive upper validation bound, if one exists.
    pub max: Option<f64>,
    /// The subsystem that owns the value.
    pub source: String,
    /// The authority currently responsible for changing the value.
    pub authority: String,
    /// Whether the port owner accepts manual writes through `SetPorts`.
    pub writable: bool,
}

impl PortMetadata {
    /// Build metadata for the scalar port contract.
    pub fn scalar(
        direction: PortDirection,
        unit: Option<UnitReference>,
        min: Option<f64>,
        max: Option<f64>,
        source: impl Into<String>,
        authority: impl Into<String>,
        writable: bool,
        frame: Option<CoordinateFrameId>,
    ) -> Self {
        Self {
            value_type: PortValueType::Scalar,
            unit,
            frame,
            min,
            max,
            source: source.into(),
            authority: authority.into(),
            writable: writable && matches!(direction, PortDirection::In | PortDirection::InOut),
        }
    }

    /// Validate the owner-declared contract before it is used to admit writes.
    pub fn validate_contract(&self) -> Result<(), PortMetadataError> {
        if self.source.trim().is_empty() {
            return Err(PortMetadataError::EmptySource);
        }
        if self.authority.trim().is_empty() {
            return Err(PortMetadataError::EmptyAuthority);
        }
        if self.min.is_some_and(|min| !min.is_finite()) {
            return Err(PortMetadataError::NonFiniteMinimum);
        }
        if self.max.is_some_and(|max| !max.is_finite()) {
            return Err(PortMetadataError::NonFiniteMaximum);
        }
        if self.min.zip(self.max).is_some_and(|(min, max)| min > max) {
            return Err(PortMetadataError::ReversedRange);
        }
        Ok(())
    }

    /// Validate a value before dispatching it to a writable port.
    pub fn validate(&self, value: f64) -> Result<(), String> {
        self.validate_contract()
            .map_err(|error| error.to_string())?;
        self.validate_value(value)
    }

    fn validate_value(&self, value: f64) -> Result<(), String> {
        if !value.is_finite() {
            return Err("value must be finite".into());
        }
        if let Some(min) = self.min {
            if value < min {
                return Err(format!("value must be ≥ {min}"));
            }
        }
        if let Some(max) = self.max {
            if value > max {
                return Err(format!("value must be ≤ {max}"));
            }
        }
        Ok(())
    }
}

/// Invalid owner-provided port contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortMetadataError {
    /// The subsystem that owns the value is missing.
    EmptySource,
    /// The authority responsible for changes is missing.
    EmptyAuthority,
    /// The inclusive lower bound is not finite.
    NonFiniteMinimum,
    /// The inclusive upper bound is not finite.
    NonFiniteMaximum,
    /// The inclusive lower bound exceeds the upper bound.
    ReversedRange,
}

impl fmt::Display for PortMetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::EmptySource => "port metadata source must not be empty",
            Self::EmptyAuthority => "port metadata authority must not be empty",
            Self::NonFiniteMinimum => "port metadata lower bound must be finite",
            Self::NonFiniteMaximum => "port metadata upper bound must be finite",
            Self::ReversedRange => "port metadata lower bound exceeds upper bound",
        })
    }
}

impl std::error::Error for PortMetadataError {}

/// A discovered port with its live value and owner-provided metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct PortInfo {
    /// Port name — the key in the owning backend, or the canonical name for a
    /// single-value backend.
    pub name: String,
    /// Causality.
    pub direction: PortDirection,
    /// Current owner sample. `None` means the port is declared but has not
    /// produced a readable sample; it is never encoded as a numeric zero.
    pub value: Option<f64>,
    /// Owner-provided type, unit, validation, source, and authority.
    pub metadata: PortMetadata,
}

/// A port owner as seen by the registry, including the precedence used when
/// more than one backend exposes the same public name.
#[derive(Debug, Clone, PartialEq)]
pub struct PortOwnerInfo {
    /// Public port name.
    pub name: String,
    /// Causality declared by the owner.
    pub direction: PortDirection,
    /// Registration order used by [`PortRegistry::write_port`] and the read
    /// methods. Lower values win.
    pub precedence: usize,
    /// Owner-provided domain/backend description.
    pub metadata: PortMetadata,
}

/// A process-local handle to one registered port backend.
///
/// It is captured by inspection projections so a later live-value read can
/// address the original owner directly instead of folding over every backend
/// again. Like [`ResolvedPort`], it is valid only for the current registry
/// ordering and must not be serialized.
#[derive(Clone, Copy, Debug, Default)]
pub struct PortHandle {
    backend: usize,
    slot: Option<u64>,
    reader: Option<fn(&World, Entity, u64) -> Option<f64>>,
}

impl PartialEq for PortHandle {
    fn eq(&self, other: &Self) -> bool {
        self.backend == other.backend && self.slot == other.slot
    }
}

impl Eq for PortHandle {}

/// The access side on which a duplicate public name collides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PortCollisionDirection {
    /// More than one owner can receive a write.
    Input,
    /// More than one owner can provide a read value.
    Output,
    /// More than one bidirectional owner exposes the same name.
    InOut,
}

/// Multiple runtime owners of one public port name on one entity.
#[derive(Debug, Clone, PartialEq)]
pub struct PortCollision {
    /// Public port name.
    pub name: String,
    /// Access side that is ambiguous.
    pub direction: PortCollisionDirection,
    /// Owners in registry precedence order. The first entry is the owner that
    /// receives the corresponding registry operation.
    pub owners: Vec<PortOwnerInfo>,
}

/// A declared port identity and causality, supplied by its owning backend.
/// A declaration does not imply that the owner has produced a live sample.
#[derive(Debug, Clone)]
pub struct PortDeclaration {
    /// Port name — the key in the owning backend, or the canonical name for a
    /// single-value backend.
    pub name: String,
    /// Causality.
    pub direction: PortDirection,
}

/// One backend declaration query. Inspection collects owned rows; a named
/// query borrows its name and records only the first matching direction.
/// Both paths consume the same owner declarations, independent of live samples.
pub struct PortDeclarationQuery<'a> {
    target: PortDeclarationTarget<'a>,
}

enum PortDeclarationTarget<'a> {
    All(&'a mut Vec<PortDeclaration>),
    Named {
        name: &'a str,
        side: ResolvedPortSide,
        direction: Option<PortDirection>,
    },
}

impl<'a> PortDeclarationQuery<'a> {
    /// Collect every declaration in the backend's enumeration order.
    pub fn all(out: &'a mut Vec<PortDeclaration>) -> Self {
        Self {
            target: PortDeclarationTarget::All(out),
        }
    }

    fn named(name: &'a str, side: ResolvedPortSide) -> Self {
        Self {
            target: PortDeclarationTarget::Named {
                name,
                side,
                direction: None,
            },
        }
    }

    /// The exact requested name, when discovery is limited to one port.
    /// Owners with indexed storage can query it without traversing their surface.
    pub fn requested_name(&self) -> Option<&'a str> {
        match &self.target {
            PortDeclarationTarget::All(_) => None,
            PortDeclarationTarget::Named { name, .. } => Some(name),
        }
    }

    /// Whether this query can consume a declaration with this causality.
    pub fn accepts_direction(&self, direction: PortDirection) -> bool {
        match &self.target {
            PortDeclarationTarget::All(_) => true,
            PortDeclarationTarget::Named { side, .. } => match side {
                ResolvedPortSide::Input => {
                    matches!(direction, PortDirection::In | PortDirection::InOut)
                }
                ResolvedPortSide::Output => {
                    matches!(direction, PortDirection::Out | PortDirection::InOut)
                }
            },
        }
    }

    /// Publish a borrowed declaration. Only an inspection query copies its name.
    pub fn declare(&mut self, name: &str, direction: PortDirection) {
        if !self.accepts_direction(direction) {
            return;
        }
        match &mut self.target {
            PortDeclarationTarget::All(out) => out.push(PortDeclaration {
                name: name.to_owned(),
                direction,
            }),
            PortDeclarationTarget::Named {
                name: requested,
                direction: found,
                ..
            } => {
                if name == *requested && found.is_none() {
                    *found = Some(direction);
                }
            }
        }
    }

    fn direction(&self) -> Option<PortDirection> {
        match &self.target {
            PortDeclarationTarget::All(_) => None,
            PortDeclarationTarget::Named { direction, .. } => *direction,
        }
    }
}

/// Declare map-backed ports through the owner's indexed name storage.
#[inline]
pub fn declare_map(out: &mut PortDeclarationQuery<'_>, map: &PortMap<f64>, dir: PortDirection) {
    if !out.accepts_direction(dir) {
        return;
    }
    if let Some(name) = out.requested_name() {
        if map.contains_key(name) {
            out.declare(name, dir);
        }
    } else {
        for name in map.keys() {
            out.declare(name, dir);
        }
    }
}

/// Return an order-independent cache key for a backend's port-name set.
///
/// Values are deliberately excluded: a topology key changes only when port
/// identity changes, so live samples can be refreshed without rebuilding the
/// metadata rows.
pub fn port_name_set_key<'a, I, N>(names: I) -> u64
where
    I: IntoIterator<Item = &'a N>,
    N: AsRef<str> + ?Sized + 'a,
{
    let mut key = PortNameSetKey::default();
    for name in names {
        key.insert(name.as_ref());
    }
    key.finish()
}

/// Return an order-independent cache key for a named port-to-entity map.
///
/// Entity identity is part of a projected surface's structure: replacing the
/// endpoint behind an unchanged name must invalidate consumers just as adding
/// or removing the name does. Values are not part of this key.
pub fn port_entity_map_key<'a, I, N>(entries: I) -> u64
where
    I: IntoIterator<Item = (&'a N, &'a Entity)>,
    N: AsRef<str> + ?Sized + 'a,
{
    let mut key = 0xcbf29ce484222325u64;
    let mut count = 0u64;
    for (name, entity) in entries {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        name.as_ref().hash(&mut hasher);
        entity.hash(&mut hasher);
        key = key.wrapping_add(hasher.finish().rotate_left(17));
        count = count.wrapping_add(1);
    }
    key ^ count.rotate_left(41)
}

/// One port-bearing backend, expressed as an entity enumerator plus operations
/// over `(World, Entity)`.
///
/// Ops are plain `fn` pointers (non-capturing closures), so a backend is `Copy`
/// and the registry is cheap to clone out of the world for `&mut World` access.
/// Each op is causality-correct: `read_output`/`read_input` see only the matching
/// direction. Writable inputs use an owner-resolved slot so validation and
/// application address the same port. A single-value backend is bidirectional —
/// its one scalar *is* both its output and input.
#[derive(Clone, Copy)]
pub struct PortBackend {
    /// Append entities owned by this backend to `out`.
    ///
    /// This is the backend's authoritative discovery boundary. Consumers that
    /// need to inspect all ports must use [`PortRegistry::port_entities`]
    /// instead of scanning every ECS entity and probing every backend.
    pub list_entities: fn(&mut World, &mut Vec<Entity>),
    /// Return a key for this backend's declared contract on `entity`.
    ///
    /// The key must ignore live values and change when port names, directions,
    /// bounds, units, frames, writability, or other published metadata change.
    /// The owning plugin's change-filtered structural
    /// check publishes [`PortTopologyRevision`] when that key changes; the key
    /// is evaluated only on the changed-owner path. It lets consumers cache
    /// metadata while still observing dynamic authored surfaces.
    pub topology_key: fn(&World, Entity) -> u64,
    /// Publish this backend's live declarations (outputs then inputs) through
    /// the query. Use its requested name for indexed discovery; never infer a
    /// declaration from the presence of a numeric sample.
    pub declare_ports: fn(&World, Entity, &mut PortDeclarationQuery<'_>),
    /// Describe every port published by `declare_ports`. This callback is mandatory so
    /// the owning backend, rather than a registry fallback, defines each port's
    /// value, unit, bounds, authority, source, and writability contract.
    pub metadata: fn(&World, Entity, &str, PortDirection) -> PortMetadata,
    /// Read the **output** named `name`, or `None`.
    pub read_output: fn(&World, Entity, &str) -> Option<f64>,
    /// Read the **input** named `name`, or `None`.
    pub read_input: fn(&World, Entity, &str) -> Option<f64>,
    // ── Optional resolve→slot fast path (the FMI valueReference model) ──────────
    //
    // A backend with dynamic names or a multi-owner presence scan resolves an
    // endpoint to a process-local slot once. Input write ownership and readable
    // input-side sources are resolved separately. Every writable input provides
    // an input resolver and slot writer. See [`PortRegistry::resolve_output`].
    /// Resolve an **output** name to a backend-private `slot` (opaque `u64`),
    /// or `None` if this backend doesn't own it. Encodes causality: only an
    /// `Out`/`InOut` port resolves here.
    pub resolve_output: Option<fn(&World, Entity, &str) -> Option<u64>>,
    /// Resolve an **input** name to a backend-private `slot`, or `None`. Only an
    /// `In`/`InOut` port resolves here.
    pub resolve_input: Option<fn(&World, Entity, &str) -> Option<u64>>,
    /// Read the value at a previously-resolved `slot`. `None` if the slot no
    /// longer backs a live value. The shared port-topology revision requires
    /// compiled consumers to rebuild locators at their next deterministic
    /// boundary; a stale locator is not retried as a named read.
    pub read_slot: Option<fn(&World, Entity, u64) -> Option<f64>>,
    /// Read the input-side value at a previously-resolved `slot`. Kept separate
    /// from `read_slot` because one name may expose distinct input and output
    /// values on the same entity.
    pub read_input_slot: Option<fn(&World, Entity, u64) -> Option<f64>>,
    /// Commit `value` to a previously-resolved input slot.
    ///
    /// The registry preflights every write in a batch against the same
    /// exclusive `World` before invoking any writer. This callback is therefore
    /// an infallible commit operation: it must only update the already-resolved
    /// value and must not alter port topology.
    pub write_slot: Option<fn(&mut World, Entity, u64, f64)>,
}

/// A process-local resolved locator for one port on one backend — the FMI
/// *valueReference* analogue, including the causality side selected at resolve.
///
/// `slot` is an opaque `u64` the **owning backend** encodes and decodes; it is
/// meaningful only within this process/run and MUST NEVER be serialized or sent
/// on the wire (resolve fresh on every peer — slots are process-local, like FMI
/// value references). Produced by [`PortRegistry::resolve_output`],
/// [`PortRegistry::resolve_input`], or [`PortRegistry::resolve_input_read`], consumed by
/// [`read_resolved`](PortRegistry::read_resolved) /
/// [`write_resolved`](PortRegistry::write_resolved). Reads dispatch by slot;
/// writes preflight the live owner contract once before committing that slot.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPort {
    /// Index of the owning backend in the registry (its registration order).
    backend: usize,
    /// Backend-private opaque locator.
    slot: u64,
    /// Canonical name required to validate the live owner contract.
    name: Arc<str>,
    /// Declared direction at resolution, preserved for live metadata reads.
    direction: PortDirection,
    /// Required owner contract at resolution time.
    metadata: PortMetadata,
    /// Structural revision at which the owner and metadata were resolved.
    revision: u64,
    /// Causality side used to resolve this locator.
    side: ResolvedPortSide,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolvedPortSide {
    Input,
    Output,
}

/// Why a value could not be admitted by the owning input port.
#[derive(Clone, Debug, PartialEq)]
pub enum PortWriteErrorKind {
    /// No backend owns this input name on the target.
    UnknownInput,
    /// An owning backend did not declare the resolved port row.
    MetadataUnavailable,
    /// The owner explicitly marked the input as non-writable.
    NotWritable,
    /// The owner did not provide a resolved slot writer for this input.
    UnsupportedWritePath,
    /// The owner supplied an invalid port contract.
    InvalidMetadata { reason: String },
    /// The proposed value violated the owner-supplied contract.
    InvalidValue {
        value: f64,
        unit: Option<String>,
        min: Option<f64>,
        max: Option<f64>,
        reason: String,
    },
    /// A cached resolved handle no longer identifies this input.
    StaleResolution,
    /// The shared topology revision resource is required for safe write
    /// preparation and resolved-handle validation.
    TopologyRevisionUnavailable,
    /// A batch contains more than one write to the same input.
    DuplicateWrite,
}

/// Structured failure returned by the shared port write boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct PortWriteError {
    /// Target input name.
    pub port: String,
    /// Port owner when resolution found one.
    pub owner: Option<String>,
    /// Rejection category and owner-supplied reason.
    pub kind: PortWriteErrorKind,
}

impl fmt::Display for PortWriteError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let owner = self
            .owner
            .as_deref()
            .map(|owner| format!(" (owner: {owner})"))
            .unwrap_or_default();
        match &self.kind {
            PortWriteErrorKind::UnknownInput => {
                write!(formatter, "unknown input port `{}`", self.port)
            }
            PortWriteErrorKind::MetadataUnavailable => write!(
                formatter,
                "input port `{}` has no owner metadata{owner}",
                self.port
            ),
            PortWriteErrorKind::NotWritable => write!(
                formatter,
                "input port `{}` is not writable{owner}",
                self.port
            ),
            PortWriteErrorKind::UnsupportedWritePath => write!(
                formatter,
                "input port `{}` has no resolved slot writer{owner}",
                self.port
            ),
            PortWriteErrorKind::InvalidMetadata { reason } => write!(
                formatter,
                "input port `{}` has invalid owner metadata{owner}: {reason}",
                self.port
            ),
            PortWriteErrorKind::InvalidValue {
                value,
                unit,
                min,
                max,
                reason,
            } => {
                let range = match (min, max) {
                    (Some(min), Some(max)) => format!("; allowed range {min}..={max}"),
                    (Some(min), None) => format!("; allowed minimum {min}"),
                    (None, Some(max)) => format!("; allowed maximum {max}"),
                    (None, None) => String::new(),
                };
                write!(
                    formatter,
                    "value {value}{} for input port `{}` was rejected{owner}{range}: {reason}",
                    unit.as_deref()
                        .map(|unit| format!(" {unit}"))
                        .unwrap_or_default(),
                    self.port
                )
            }
            PortWriteErrorKind::StaleResolution => {
                write!(formatter, "resolved input port `{}` is stale", self.port)
            }
            PortWriteErrorKind::TopologyRevisionUnavailable => write!(
                formatter,
                "input port `{}` cannot be written without the shared topology revision{owner}",
                self.port
            ),
            PortWriteErrorKind::DuplicateWrite => write!(
                formatter,
                "input port `{}` appears more than once in the write batch{owner}",
                self.port
            ),
        }
    }
}

impl std::error::Error for PortWriteError {}

/// A validated write awaiting application at the same exclusive world boundary.
#[derive(Clone, Debug, PartialEq)]
pub struct PreparedPortWrite {
    backend: usize,
    entity: Entity,
    name: Arc<str>,
    direction: PortDirection,
    metadata: PortMetadata,
    revision: u64,
    slot: u64,
    value: f64,
}

impl PreparedPortWrite {
    /// Target entity prepared by the owner registry.
    pub fn entity(&self) -> Entity {
        self.entity
    }

    /// Canonical input name prepared by the owner registry.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Finite scalar value validated against the owner contract.
    pub fn value(&self) -> f64 {
        self.value
    }
}

/// The single registry of port-bearing backends — **the** read/write/list surface
/// for every exposed simulation value, whichever backend owns it.
///
/// Backends are registered (in dependency-correct order) by their owning crate's
/// plugin; the registry folds their discovery and access operations. Registration order is
/// resolution precedence (first match wins). `Clone` is cheap (a `Vec` of `Copy`
/// `fn` pointers) so a `&mut World` caller clones it out before writing.
#[derive(Resource, Clone)]
pub struct PortRegistry {
    backends: Vec<PortBackend>,
}

impl Default for PortRegistry {
    fn default() -> Self {
        // Every entity's declared `inputs:*` values use this substrate backend.
        // It is installed with the registry rather than by mobility, Modelica, or
        // a UI plugin, so physical, scripted and future participants share one
        // command/value spelling.
        Self {
            backends: vec![INPUT_PORTS_BACKEND],
        }
    }
}

/// The runtime storage behind a participant's declared `inputs:*` ports.
///
/// This is deliberately generic: it contains neither vehicle vocabulary nor
/// control/authority policy. A controller, wire, script, or network peer writes
/// these inputs through [`PortRegistry`] exactly as it writes a Modelica input.
const INPUT_PORTS_BACKEND: PortBackend = PortBackend {
    list_entities: |world, out| {
        out.extend(
            world
                .query_filtered::<Entity, With<InputPorts>>()
                .iter(world),
        );
    },
    topology_key: |world, entity| {
        world
            .get::<InputPorts>(entity)
            .map_or(0, |inputs| inputs.values.topology_key())
    },
    declare_ports: |world, entity, out| {
        if let Some(inputs) = world.get::<InputPorts>(entity) {
            declare_map(out, &inputs.values, PortDirection::In);
        }
    },
    metadata: |_world, _entity, _name, direction| {
        PortMetadata::scalar(
            direction,
            None,
            None,
            None,
            "input surface",
            "input owner",
            true,
            None,
        )
    },
    read_output: |_world, _entity, _name| None,
    read_input: |world, entity, name| {
        world
            .get::<InputPorts>(entity)
            .and_then(|inputs| inputs.values.get(name).copied())
    },
    resolve_output: None,
    resolve_input: Some(|world, entity, name| {
        world.get::<InputPorts>(entity)?.values.resolve_slot(name)
    }),
    read_slot: None,
    read_input_slot: Some(|world, entity, slot| {
        world
            .get::<InputPorts>(entity)?
            .values
            .get_slot(slot)
            .copied()
    }),
    write_slot: Some(|world, entity, slot, value| {
        let mut inputs = world
            .get_mut::<InputPorts>(entity)
            .expect("prepared input slot still belongs to its InputPorts component");
        let changed = inputs
            .bypass_change_detection()
            .values
            .set_slot_existing(slot, value)
            .expect("prepared input slot remains live through the exclusive commit");
        if changed {
            inputs.set_changed();
        }
    }),
};

impl PortRegistry {
    fn resolved_port(
        &self,
        world: &World,
        entity: Entity,
        backend: usize,
        name: &str,
        slot: u64,
        side: ResolvedPortSide,
    ) -> Option<ResolvedPort> {
        let (direction, metadata) =
            self.port_metadata_for_backend(world, entity, backend, name, side)?;
        let revision = world.get_resource::<PortTopologyRevision>()?.0;
        Some(ResolvedPort {
            backend,
            slot,
            name: Arc::from(name),
            direction,
            metadata,
            revision,
            side,
        })
    }

    /// Register a backend. Later registrations have lower precedence on name
    /// collisions. Call from a plugin `build`.
    ///
    /// Precedence follows plugin add-order, which no plugin controls, so a backend
    /// must claim a name only when it genuinely owns it. One that would otherwise
    /// have to guess needs an authoritative set to answer from instead.
    pub fn register(&mut self, backend: PortBackend) {
        self.backends.push(backend);
    }

    /// Enumerate every entity owned by at least one registered port backend.
    ///
    /// Entity discovery belongs to each backend because only the backend owner
    /// knows which component or authored surface makes an entity eligible. The
    /// registry only merges and deduplicates those authoritative candidate sets;
    /// it never infers ownership by probing the whole ECS world.
    pub fn port_entities(&self, world: &mut World) -> Vec<Entity> {
        self.port_entities_with_topology_keys(world)
            .into_iter()
            .map(|(entity, _)| entity)
            .collect()
    }

    /// Enumerate every backend-owned entity with its combined port-identity key.
    ///
    /// Each backend's key is evaluated alongside that backend's authoritative
    /// entity list. This avoids probing every registered backend for every
    /// candidate while retaining the same combined key for entities exposed by
    /// multiple owners.
    pub fn port_entities_with_topology_keys(&self, world: &mut World) -> Vec<(Entity, u64)> {
        let mut keys: HashMap<Entity, u64> = HashMap::new();
        for (backend_index, backend) in self.backends.iter().enumerate() {
            let mut owned = Vec::new();
            (backend.list_entities)(world, &mut owned);
            owned.sort_unstable_by_key(|entity| entity.to_bits());
            owned.dedup();
            for entity in owned {
                let owner_key = (backend.topology_key)(world, entity);
                let contribution = owner_key.rotate_left((backend_index % 63) as u32);
                keys.entry(entity)
                    .and_modify(|key| *key = key.wrapping_add(contribution))
                    .or_insert(contribution);
            }
        }
        let mut entities: Vec<_> = keys.into_iter().collect();
        entities.sort_unstable_by_key(|(entity, _)| entity.to_bits());
        entities
    }

    /// Return the combined port-identity key for one entity.
    ///
    /// Each backend owns the identity of its own surface; the registry only
    /// combines those owner keys for cache invalidation. Live port values are
    /// intentionally not part of this key.
    pub fn entity_port_topology_key(&self, world: &World, entity: Entity) -> u64 {
        self.backends
            .iter()
            .enumerate()
            .fold(0u64, |key, (index, backend)| {
                let owner_key = (backend.topology_key)(world, entity);
                key.wrapping_add(owner_key.rotate_left((index % 63) as u32))
            })
    }

    /// Enumerate every exposed port with owner-provided metadata.
    /// The sample comes from the owning backend's live reader, independently
    /// from the stable declaration list.
    pub fn entity_port_infos(&self, world: &World, entity: Entity) -> Vec<PortInfo> {
        self.entity_port_infos_with_handles(world, entity)
            .into_iter()
            .map(|(_, info)| info)
            .collect()
    }

    /// Sample only the exposed ports whose exact names are requested.
    ///
    /// Every declared direction and backend owner row for a selected name is
    /// retained. Unselected ports are listed for discovery but their metadata
    /// and live values are not resolved or sampled.
    pub fn entity_port_infos_for_names(
        &self,
        world: &World,
        entity: Entity,
        requested_names: &[String],
    ) -> Vec<PortInfo> {
        let requested_names: HashSet<&str> = requested_names.iter().map(String::as_str).collect();
        self.entity_port_infos_with_handles_matching(world, entity, Some(&requested_names))
            .into_iter()
            .map(|(_, info)| info)
            .collect()
    }

    /// Enumerate every exposed port with its owning backend handle.
    ///
    /// This is the cache-friendly inspection surface. The handle lets a
    /// consumer refresh the value through the same owner without re-running
    /// registry precedence resolution on every sample.
    pub fn entity_port_infos_with_handles(
        &self,
        world: &World,
        entity: Entity,
    ) -> Vec<(PortHandle, PortInfo)> {
        self.entity_port_infos_with_handles_matching(world, entity, None)
    }

    fn entity_port_infos_with_handles_matching(
        &self,
        world: &World,
        entity: Entity,
        requested_names: Option<&HashSet<&str>>,
    ) -> Vec<(PortHandle, PortInfo)> {
        let mut out = Vec::new();
        for (backend_index, backend) in self.backends.iter().enumerate() {
            let mut ports = Vec::new();
            (backend.declare_ports)(world, entity, &mut PortDeclarationQuery::all(&mut ports));
            for port in ports {
                if let Some(names) = requested_names
                    && !names.contains(port.name.as_str())
                {
                    continue;
                }
                let (slot, reader) = match port.direction {
                    PortDirection::In => backend
                        .resolve_input
                        .and_then(|resolve| resolve(world, entity, &port.name))
                        .zip(backend.read_input_slot),
                    PortDirection::Out => backend
                        .resolve_output
                        .and_then(|resolve| resolve(world, entity, &port.name))
                        .zip(backend.read_slot),
                    PortDirection::InOut => backend
                        .resolve_output
                        .and_then(|resolve| resolve(world, entity, &port.name))
                        .zip(backend.read_slot)
                        .or_else(|| {
                            backend
                                .resolve_input
                                .and_then(|resolve| resolve(world, entity, &port.name))
                                .zip(backend.read_input_slot)
                        }),
                }
                .map_or((None, None), |(slot, reader)| (Some(slot), Some(reader)));
                let handle = PortHandle {
                    backend: backend_index,
                    slot,
                    reader,
                };
                let value =
                    self.read_port_for_handle(world, handle, entity, &port.name, port.direction);
                out.push((
                    handle,
                    PortInfo {
                        metadata: (backend.metadata)(world, entity, &port.name, port.direction),
                        name: port.name,
                        direction: port.direction,
                        value,
                    },
                ));
            }
        }
        out
    }

    /// Enumerate the distinct runtime owners of every public port on `entity`.
    ///
    /// A backend may expose one port through more than one inspection view. The
    /// registered backend is the owner identity, so repeated views from the
    /// same backend are collapsed while distinct backends remain visible.
    /// The returned precedence is the registry order consumed by the read/write
    /// methods; this is intentionally a diagnostic read and never changes
    /// routing.
    pub fn entity_port_owners(&self, world: &World, entity: Entity) -> Vec<PortOwnerInfo> {
        let mut out = Vec::new();
        for (precedence, backend) in self.backends.iter().enumerate() {
            let mut ports = Vec::new();
            (backend.declare_ports)(world, entity, &mut PortDeclarationQuery::all(&mut ports));
            let mut by_name = BTreeMap::new();
            for port in ports {
                by_name
                    .entry(port.name)
                    .and_modify(|direction| {
                        *direction = match (*direction, port.direction) {
                            (PortDirection::In, PortDirection::In)
                            | (PortDirection::Out, PortDirection::Out) => *direction,
                            _ => PortDirection::InOut,
                        };
                    })
                    .or_insert(port.direction);
            }
            for (name, direction) in by_name {
                let metadata = (backend.metadata)(world, entity, &name, direction);
                out.push(PortOwnerInfo {
                    name,
                    direction,
                    precedence,
                    metadata,
                });
            }
        }
        out.sort_by(|a, b| {
            a.precedence
                .cmp(&b.precedence)
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.direction.cmp(&b.direction))
                .then_with(|| a.metadata.source.cmp(&b.metadata.source))
        });
        out
    }

    /// Enumerate the precedence-winning input owner for each public input name.
    /// Output-only owners do not claim an input name; `InOut` owners do.
    pub fn input_port_owners(&self, world: &World, entity: Entity) -> Vec<PortOwnerInfo> {
        let mut claimed = BTreeSet::new();
        self.entity_port_owners(world, entity)
            .into_iter()
            .filter(|owner| {
                matches!(owner.direction, PortDirection::In | PortDirection::InOut)
                    && claimed.insert(owner.name.clone())
            })
            .collect()
    }

    /// Find public names with more than one runtime owner on `entity`.
    ///
    /// `InOut` owners participate in both the input and output access sides
    /// when mixed with a one-way owner. Two or more `InOut` owners produce one
    /// `InOut` collision, avoiding duplicate findings for the same ambiguity.
    pub fn entity_port_collisions(&self, world: &World, entity: Entity) -> Vec<PortCollision> {
        let mut by_name: BTreeMap<String, Vec<PortOwnerInfo>> = BTreeMap::new();
        for owner in self.entity_port_owners(world, entity) {
            by_name.entry(owner.name.clone()).or_default().push(owner);
        }

        let mut collisions = Vec::new();
        for (name, owners) in by_name {
            if owners.len() < 2 {
                continue;
            }
            let all_inout = owners
                .iter()
                .all(|owner| owner.direction == PortDirection::InOut);
            if all_inout {
                collisions.push(PortCollision {
                    name,
                    direction: PortCollisionDirection::InOut,
                    owners,
                });
                continue;
            }

            let inputs: Vec<_> = owners
                .iter()
                .filter(|owner| matches!(owner.direction, PortDirection::In | PortDirection::InOut))
                .cloned()
                .collect();
            if inputs.len() > 1 {
                collisions.push(PortCollision {
                    name: name.clone(),
                    direction: PortCollisionDirection::Input,
                    owners: inputs,
                });
            }

            let outputs: Vec<_> = owners
                .iter()
                .filter(|owner| {
                    matches!(owner.direction, PortDirection::Out | PortDirection::InOut)
                })
                .cloned()
                .collect();
            if outputs.len() > 1 {
                collisions.push(PortCollision {
                    name,
                    direction: PortCollisionDirection::Output,
                    owners: outputs,
                });
            }
        }
        collisions
    }

    /// Read the **output** named `name` on `entity` — the value a connection reads
    /// from its *source*. Searches the precedence-winning declared output owner
    /// only, even when that owner has not produced a sample yet.
    pub fn read_output_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        let owner = self.port_owner_index(world, entity, name, ResolvedPortSide::Output)?;
        (self.backends[owner].read_output)(world, entity, name)
    }

    /// Read the current value of port `name`, preferring an **output**, then
    /// using the input owner only when no output owner declares that name. A
    /// declared but unsampled output remains unsampled instead of exposing a
    /// lower-precedence or opposite-direction value.
    pub fn read_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        if let Some(owner) = self.port_owner_index(world, entity, name, ResolvedPortSide::Output) {
            return (self.backends[owner].read_output)(world, entity, name);
        }
        self.read_input_port(world, entity, name)
    }

    /// Read the **input** value of port `name` — the commanded side, skipping
    /// outputs. Use where the input specifically is wanted (e.g. a joint's
    /// commanded motor setpoint vs its measured angle, both named `angle`).
    pub fn read_input_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        let owner = self.port_owner_index(world, entity, name, ResolvedPortSide::Input)?;
        (self.backends[owner].read_input)(world, entity, name)
    }

    /// Read the input value from its precedence-winning owner without falling
    /// through to another backend when that owner's current sample is absent.
    pub fn read_owned_input_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        let owner = self.port_owner_index(world, entity, name, ResolvedPortSide::Input)?;
        (self.backends[owner].read_input)(world, entity, name)
    }

    /// Read the output value from its precedence-winning owner without falling
    /// through to another backend when that owner's current sample is absent.
    pub fn read_owned_output_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        let owner = self.port_owner_index(world, entity, name, ResolvedPortSide::Output)?;
        (self.backends[owner].read_output)(world, entity, name)
    }

    /// Read a port through the backend that produced its inspection row.
    ///
    /// This preserves per-owner rows when two backends expose the same public
    /// name and avoids a full registry fold in bounded-cadence inspectors.
    pub fn read_port_for_handle(
        &self,
        world: &World,
        handle: PortHandle,
        entity: Entity,
        name: &str,
        direction: PortDirection,
    ) -> Option<f64> {
        if let (Some(slot), Some(read)) = (handle.slot, handle.reader) {
            return read(world, entity, slot);
        }
        let backend = self.backends.get(handle.backend)?;
        match direction {
            PortDirection::In => (backend.read_input)(world, entity, name),
            PortDirection::Out => (backend.read_output)(world, entity, name),
            PortDirection::InOut => (backend.read_output)(world, entity, name)
                .or_else(|| (backend.read_input)(world, entity, name)),
        }
    }

    /// Whether an output port is declared by an owning backend, independently
    /// of whether it has produced a sample yet.
    ///
    /// Port identity and sample availability are different facts. A physics
    /// body owns its velocity port before the first writeback, and a Modelica
    /// participant owns a declared output while it is still compiling. Using
    /// `read_output_port` as an existence test turns both lifecycle states into
    /// a dangling-wire fault. Backends with a resolver (such as Avian) answer
    /// from their component contract; the ordinary list surface covers
    /// map-backed and authored-output participants.
    pub fn has_output_port(&self, world: &World, entity: Entity, name: &str) -> bool {
        self.port_owner_index(world, entity, name, ResolvedPortSide::Output)
            .is_some()
    }

    /// Whether an input port is declared by an owning backend, independently of
    /// its current value.
    pub fn has_input_port(&self, world: &World, entity: Entity, name: &str) -> bool {
        self.port_owner_index(world, entity, name, ResolvedPortSide::Input)
            .is_some()
    }

    /// Return metadata for the precedence-winning owner of an input port.
    ///
    /// Input and output ports may share a public name (for example a joint's
    /// commanded and measured `angle`). This resolves the input side and passes
    /// its declared direction to the metadata provider instead of merging the
    /// directions into one diagnostic owner record.
    pub fn input_port_metadata(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
    ) -> Option<PortMetadata> {
        self.port_metadata_for_side(world, entity, name, ResolvedPortSide::Input)
    }

    /// Return metadata for the precedence-winning owner of an output port.
    /// Output samples may share their public name with a distinct commanded
    /// input, so this resolves the output side independently.
    pub fn output_port_metadata(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
    ) -> Option<PortMetadata> {
        self.port_metadata_for_side(world, entity, name, ResolvedPortSide::Output)
    }

    fn port_metadata_for_side(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
        side: ResolvedPortSide,
    ) -> Option<PortMetadata> {
        let backend_index = self.port_owner_index(world, entity, name, side)?;
        self.port_metadata_for_backend(world, entity, backend_index, name, side)
            .map(|(_, metadata)| metadata)
    }

    fn port_owner_index(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
        side: ResolvedPortSide,
    ) -> Option<usize> {
        self.backends.iter().position(|backend| {
            let resolver = match side {
                ResolvedPortSide::Input => backend.resolve_input,
                ResolvedPortSide::Output => backend.resolve_output,
            };
            if resolver.is_some_and(|resolve| resolve(world, entity, name).is_some()) {
                return true;
            }
            let mut query = PortDeclarationQuery::named(name, side);
            (backend.declare_ports)(world, entity, &mut query);
            query.direction().is_some()
        })
    }

    /// Validate and write one **input** through its precedence-winning owner.
    /// Every producer uses this contract, including wires and resolved-slot
    /// writers. The call owns an exclusive world boundary, so metadata
    /// validation and backend application observe one topology state.
    pub fn write_port(
        &self,
        world: &mut World,
        entity: Entity,
        name: &str,
        value: f64,
    ) -> Result<(), PortWriteError> {
        let prepared = self.prepare_input_write(world, entity, name, value)?;
        self.apply_prepared_input_writes(world, std::slice::from_ref(&prepared))
    }

    /// Validate a write without changing simulation state. Callers that need
    /// an all-or-none multi-port operation prepare every write before applying
    /// any of them.
    pub fn prepare_input_write(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
        value: f64,
    ) -> Result<PreparedPortWrite, PortWriteError> {
        let Some(owner) = self.port_owner_index(world, entity, name, ResolvedPortSide::Input)
        else {
            return Err(PortWriteError {
                port: name.to_owned(),
                owner: None,
                kind: PortWriteErrorKind::UnknownInput,
            });
        };
        let (direction, metadata) = self
            .port_metadata_for_backend(world, entity, owner, name, ResolvedPortSide::Input)
            .ok_or_else(|| PortWriteError {
                port: name.to_owned(),
                owner: None,
                kind: PortWriteErrorKind::MetadataUnavailable,
            })?;
        metadata
            .validate_contract()
            .map_err(|error| PortWriteError {
                port: name.to_owned(),
                owner: Some(metadata.source.clone()),
                kind: PortWriteErrorKind::InvalidMetadata {
                    reason: error.to_string(),
                },
            })?;
        if !metadata.writable {
            return Err(PortWriteError {
                port: name.to_owned(),
                owner: Some(metadata.source),
                kind: PortWriteErrorKind::NotWritable,
            });
        }
        metadata
            .validate_value(value)
            .map_err(|reason| PortWriteError {
                port: name.to_owned(),
                owner: Some(metadata.source.clone()),
                kind: PortWriteErrorKind::InvalidValue {
                    value,
                    unit: metadata.unit.as_ref().map(|unit| unit.id().to_owned()),
                    min: metadata.min,
                    max: metadata.max,
                    reason,
                },
            })?;
        let backend = &self.backends[owner];
        let slot = backend
            .resolve_input
            .and_then(|resolve| resolve(world, entity, name))
            .filter(|_| backend.write_slot.is_some())
            .ok_or_else(|| PortWriteError {
                port: name.to_owned(),
                owner: Some(metadata.source.clone()),
                kind: PortWriteErrorKind::UnsupportedWritePath,
            })?;
        let revision = world
            .get_resource::<PortTopologyRevision>()
            .map(|revision| revision.0)
            .ok_or_else(|| PortWriteError {
                port: name.to_owned(),
                owner: Some(metadata.source.clone()),
                kind: PortWriteErrorKind::TopologyRevisionUnavailable,
            })?;
        Ok(PreparedPortWrite {
            backend: owner,
            entity,
            name: Arc::from(name),
            direction,
            metadata,
            revision,
            slot,
            value,
        })
    }

    /// Validate every prepared write before committing any of them.
    ///
    /// Commit callbacks are infallible and only mutate already-resolved values.
    /// Since the caller holds the exclusive `World`, no system can observe a
    /// partial batch, and all validation failures leave simulation state intact.
    pub fn apply_prepared_input_writes(
        &self,
        world: &mut World,
        prepared: &[PreparedPortWrite],
    ) -> Result<(), PortWriteError> {
        let mut unique = std::collections::HashSet::with_capacity(prepared.len());
        let mut writers = Vec::with_capacity(prepared.len());
        for write in prepared {
            if !unique.insert((write.entity, Arc::clone(&write.name))) {
                return Err(PortWriteError {
                    port: write.name.to_string(),
                    owner: Some(write.metadata.source.clone()),
                    kind: PortWriteErrorKind::DuplicateWrite,
                });
            }
            writers.push(self.preflight_prepared_input_write(world, write)?);
        }

        for (write, commit) in prepared.iter().zip(writers) {
            commit(world, write.entity, write.slot, write.value);
        }
        Ok(())
    }

    fn preflight_prepared_input_write(
        &self,
        world: &World,
        prepared: &PreparedPortWrite,
    ) -> Result<fn(&mut World, Entity, u64, f64), PortWriteError> {
        let stale = || PortWriteError {
            port: prepared.name.to_string(),
            owner: Some(prepared.metadata.source.clone()),
            kind: PortWriteErrorKind::StaleResolution,
        };
        let Some(revision) = world.get_resource::<PortTopologyRevision>() else {
            return Err(PortWriteError {
                port: prepared.name.to_string(),
                owner: Some(prepared.metadata.source.clone()),
                kind: PortWriteErrorKind::TopologyRevisionUnavailable,
            });
        };
        if revision.0 != prepared.revision
            || self.port_owner_index(
                world,
                prepared.entity,
                &prepared.name,
                ResolvedPortSide::Input,
            ) != Some(prepared.backend)
        {
            return Err(stale());
        }
        let Some((direction, metadata)) = self.port_metadata_for_backend(
            world,
            prepared.entity,
            prepared.backend,
            &prepared.name,
            ResolvedPortSide::Input,
        ) else {
            return Err(stale());
        };
        if direction != prepared.direction || metadata != prepared.metadata {
            return Err(stale());
        }
        metadata
            .validate_contract()
            .map_err(|error| PortWriteError {
                port: prepared.name.to_string(),
                owner: Some(metadata.source.clone()),
                kind: PortWriteErrorKind::InvalidMetadata {
                    reason: error.to_string(),
                },
            })?;
        if !metadata.writable {
            return Err(PortWriteError {
                port: prepared.name.to_string(),
                owner: Some(metadata.source),
                kind: PortWriteErrorKind::NotWritable,
            });
        }
        metadata
            .validate_value(prepared.value)
            .map_err(|reason| PortWriteError {
                port: prepared.name.to_string(),
                owner: Some(metadata.source.clone()),
                kind: PortWriteErrorKind::InvalidValue {
                    value: prepared.value,
                    unit: metadata.unit.as_ref().map(|unit| unit.id().to_owned()),
                    min: metadata.min,
                    max: metadata.max,
                    reason,
                },
            })?;
        let backend = self.backends.get(prepared.backend).ok_or_else(stale)?;
        let slot = backend
            .resolve_input
            .and_then(|resolve| resolve(world, prepared.entity, &prepared.name));
        if slot != Some(prepared.slot) {
            return Err(stale());
        }
        backend.write_slot.ok_or_else(|| PortWriteError {
            port: prepared.name.to_string(),
            owner: Some(prepared.metadata.source.clone()),
            kind: PortWriteErrorKind::UnsupportedWritePath,
        })
    }

    // ── Resolve→slot fast path ─────────────────────────────────────────────────

    /// Resolve an **output** endpoint `(entity, name)` to a [`ResolvedPort`] — a
    /// process-local handle a hot consumer caches once and reads by slot every
    /// tick. Returns `None` when the precedence-winning owner has no fast path;
    /// that owner continues to use [`read_output_port`](Self::read_output_port)
    /// as its canonical access path.
    ///
    /// **Precedence-correct:** walks backends in registration order and stops at
    /// the FIRST that owns `name`. A resolver can establish ownership before a
    /// value is sampled; otherwise a readable name-only backend stops lookup so
    /// a lower-precedence slot backend cannot shadow it.
    pub fn resolve_output(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
    ) -> Option<ResolvedPort> {
        let owner = self.port_owner_index(world, entity, name, ResolvedPortSide::Output)?;
        let resolve = self.backends[owner].resolve_output?;
        let slot = resolve(world, entity, name)?;
        self.resolved_port(world, entity, owner, name, slot, ResolvedPortSide::Output)
    }

    /// Resolve the precedence-winning declared input endpoint to a
    /// [`ResolvedPort`] fast-path locator. See
    /// [`resolve_output`](Self::resolve_output).
    ///
    /// Input ownership comes from the same public port list used by
    /// [`write_port`](Self::write_port). If the winning owner has no slot
    /// resolver, this returns `None`; the caller must use the named write path,
    /// which addresses that same owner and never falls through to a shadowed
    /// input.
    pub fn resolve_input(&self, world: &World, entity: Entity, name: &str) -> Option<ResolvedPort> {
        let owner = self.port_owner_index(world, entity, name, ResolvedPortSide::Input)?;
        let resolve = self.backends[owner].resolve_input?;
        let slot = resolve(world, entity, name)?;
        self.resolved_port(world, entity, owner, name, slot, ResolvedPortSide::Input)
    }

    /// Resolve a readable **input-side** source to a slot. This is distinct
    /// from [`resolve_input`](Self::resolve_input), which resolves write
    /// ownership and may legitimately select a write-only port. Resolution
    /// follows the same first-readable-owner precedence as
    /// [`read_input_port`](Self::read_input_port).
    pub fn resolve_input_read(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
    ) -> Option<ResolvedPort> {
        let owner = self.port_owner_index(world, entity, name, ResolvedPortSide::Input)?;
        let backend = &self.backends[owner];
        (backend.read_input)(world, entity, name)?;
        backend.read_input_slot?;
        let slot = (backend.resolve_input?)(world, entity, name)?;
        self.resolved_port(world, entity, owner, name, slot, ResolvedPortSide::Input)
    }

    /// Read the value at a resolved port. `None` if the slot no longer backs a
    /// live value. A compiled consumer rebuilds handles from the shared topology
    /// revision; this operation never retries through name resolution.
    pub fn read_resolved(&self, world: &World, entity: Entity, r: &ResolvedPort) -> Option<f64> {
        let backend = self.backends.get(r.backend)?;
        let read = match r.side {
            ResolvedPortSide::Input => backend.read_input_slot?,
            ResolvedPortSide::Output => backend.read_slot?,
        };
        read(world, entity, r.slot)
    }

    /// Validate and write through a previously-resolved input owner. A stale
    /// slot is an explicit failure; it is never rerouted to a lower-precedence
    /// backend by name.
    pub fn write_resolved(
        &self,
        world: &mut World,
        entity: Entity,
        r: &ResolvedPort,
        value: f64,
    ) -> Result<(), PortWriteError> {
        if r.side != ResolvedPortSide::Input {
            return Err(PortWriteError {
                port: r.name.to_string(),
                owner: None,
                kind: PortWriteErrorKind::StaleResolution,
            });
        }
        let Some(revision) = world.get_resource::<PortTopologyRevision>() else {
            return Err(PortWriteError {
                port: r.name.to_string(),
                owner: Some(r.metadata.source.clone()),
                kind: PortWriteErrorKind::TopologyRevisionUnavailable,
            });
        };
        if revision.0 != r.revision {
            return Err(PortWriteError {
                port: r.name.to_string(),
                owner: None,
                kind: PortWriteErrorKind::StaleResolution,
            });
        }
        let prepared = self.prepare_input_write(world, entity, &r.name, value)?;
        if prepared.backend != r.backend
            || prepared.slot != r.slot
            || prepared.revision != r.revision
            || prepared.direction != r.direction
            || prepared.metadata != r.metadata
        {
            return Err(PortWriteError {
                port: r.name.to_string(),
                owner: Some(r.metadata.source.clone()),
                kind: PortWriteErrorKind::StaleResolution,
            });
        }
        // Preparation and commit share this exclusive World boundary. Nothing
        // can change the validated owner between them; batch revalidation is
        // needed only when separately prepared writes are admitted together.
        let commit = self.backends[prepared.backend]
            .write_slot
            .ok_or_else(|| PortWriteError {
                port: prepared.name.to_string(),
                owner: Some(prepared.metadata.source.clone()),
                kind: PortWriteErrorKind::UnsupportedWritePath,
            })?;
        commit(world, entity, prepared.slot, prepared.value);
        Ok(())
    }

    fn port_metadata_for_backend(
        &self,
        world: &World,
        entity: Entity,
        backend_index: usize,
        name: &str,
        side: ResolvedPortSide,
    ) -> Option<(PortDirection, PortMetadata)> {
        let backend = self.backends.get(backend_index)?;
        let mut query = PortDeclarationQuery::named(name, side);
        (backend.declare_ports)(world, entity, &mut query);
        let direction = query.direction()?;
        Some((
            direction,
            (backend.metadata)(world, entity, name, direction),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PortBackend, PortCollisionDirection, PortDeclarationQuery, PortDirection, PortMetadata,
        PortNameSetKey, PortRegistry, PortTopologyRevision, PortValueType, ScalarPortMap,
        port_name_set_key,
    };

    #[test]
    fn cached_structural_observation_derives_only_admitted_or_changed_owners() {
        struct Owner;
        let entity = World::new().spawn_empty().id();
        let mut state = super::PortTopologyState::default();
        let calls = std::cell::Cell::new(0);
        let derive = |key| {
            calls.set(calls.get() + 1);
            key
        };
        assert_eq!(
            state.observe_if_changed::<Owner>(entity, false, || derive(7)),
            (7, false)
        );
        assert_eq!(
            state.observe_if_changed::<Owner>(entity, false, || derive(8)),
            (7, false)
        );
        assert_eq!(calls.get(), 1);
        assert_eq!(
            state.observe_if_changed::<Owner>(entity, true, || derive(7)),
            (7, false)
        );
        assert_eq!(
            state.observe_if_changed::<Owner>(entity, true, || derive(8)),
            (8, true)
        );
        assert_eq!(calls.get(), 3);
        state.forget::<Owner>(entity);
        assert_eq!(
            state.observe_if_changed::<Owner>(entity, false, || derive(9)),
            (9, false)
        );
        assert_eq!(calls.get(), 4);
    }
    use crate::InputPorts;
    use bevy::prelude::*;

    fn test_metadata(
        _world: &World,
        _entity: Entity,
        _name: &str,
        direction: PortDirection,
    ) -> PortMetadata {
        PortMetadata::scalar(
            direction,
            None,
            None,
            None,
            "test backend",
            "test owner",
            true,
            None,
        )
    }

    fn read_only_test_metadata(
        _world: &World,
        _entity: Entity,
        _name: &str,
        direction: PortDirection,
    ) -> PortMetadata {
        PortMetadata::scalar(
            direction,
            None,
            None,
            None,
            "test backend",
            "test observer",
            false,
            None,
        )
    }

    fn test_world() -> World {
        let mut world = World::new();
        world.insert_resource(PortTopologyRevision::default());
        world
    }

    #[test]
    fn declaration_queries_preserve_order_and_require_exact_name_and_side() {
        let publish = |query: &mut PortDeclarationQuery<'_>| {
            query.declare("other", PortDirection::In);
            query.declare("signal", PortDirection::Out);
            query.declare("signal", PortDirection::InOut);
            query.declare("signal", PortDirection::In);
        };
        let mut rows = Vec::new();
        publish(&mut PortDeclarationQuery::all(&mut rows));
        assert_eq!(rows.len(), 4);
        let mut input = PortDeclarationQuery::named("signal", super::ResolvedPortSide::Input);
        publish(&mut input);
        assert_eq!(input.direction(), Some(PortDirection::InOut));
        let mut output = PortDeclarationQuery::named("signal", super::ResolvedPortSide::Output);
        publish(&mut output);
        assert_eq!(output.direction(), Some(PortDirection::Out));
        let mut absent = PortDeclarationQuery::named("missing", super::ResolvedPortSide::Input);
        publish(&mut absent);
        assert_eq!(absent.direction(), None);

        let mut ports = ScalarPortMap::default();
        ports.insert("signal".into(), 0.0);
        let mut query = PortDeclarationQuery::named("signal", super::ResolvedPortSide::Input);
        super::declare_map(&mut query, &ports, PortDirection::Out);
        assert_eq!(query.direction(), None);
        super::declare_map(&mut query, &ports, PortDirection::In);
        assert_eq!(query.direction(), Some(PortDirection::In));
        let mut missing = PortDeclarationQuery::named("missing", super::ResolvedPortSide::Input);
        super::declare_map(&mut missing, &ports, PortDirection::In);
        assert_eq!(missing.direction(), None);
    }

    fn duplicate_input_list(_world: &World, _entity: Entity, out: &mut PortDeclarationQuery<'_>) {
        out.declare("release", PortDirection::In);
        out.declare("release", PortDirection::In);
    }

    #[test]
    fn scalar_sample_copy_checks_live_names_and_retired_layouts() {
        let precise = f64::from_bits(0x3ff0000000000001);
        let mut ports = ScalarPortMap::default();
        let samples = [("first", precise), ("second", -0.0)];
        let copy = |ports: &mut ScalarPortMap, samples: &[(&str, f64)]| {
            ports.upsert_samples(samples.iter().map(|(name, value)| (*name, value)))
        };
        assert!(copy(&mut ports, &samples));
        let topology = ports.topology_key();
        let storage = ports.sample_slots.as_ptr();
        assert!(!copy(&mut ports, &samples));
        assert_eq!(ports.topology_key(), topology);
        assert_eq!(ports.sample_slots.as_ptr(), storage);
        assert_eq!(ports["first"].to_bits(), precise.to_bits());
        assert_eq!(ports["second"].to_bits(), (-0.0f64).to_bits());

        // Source order can change independently of its cardinality.
        assert!(copy(&mut ports, &[("second", 3.0), ("first", 4.0)]));
        assert_eq!(ports["first"], 4.0);
        assert_eq!(ports["second"], 3.0);
        assert_eq!(ports.topology_key(), topology);
        assert!(copy(&mut ports, &[("third", precise), ("first", -0.0)]));
        assert_eq!(ports["third"].to_bits(), precise.to_bits());
        assert_eq!(ports["first"].to_bits(), (-0.0f64).to_bits());

        // A destination edit must retire the hint even when the name returns.
        let retired = ports.resolve_slot("third").unwrap();
        ports.remove("third");
        ports.insert("third".into(), 8.0);
        assert_eq!(ports.get_slot(retired), None);
        assert!(copy(&mut ports, &[("third", precise)]));
        assert_eq!(ports["third"].to_bits(), precise.to_bits());
        assert_eq!(ports.sample_slots.len(), 1);

        let mut cloned = ports.clone();
        assert!(cloned.sample_slots.is_empty());
        assert!(!copy(&mut cloned, &[("third", precise)]));
        assert_eq!(cloned["third"].to_bits(), precise.to_bits());
        cloned.clear();
        assert!(copy(&mut cloned, &[("third", -0.0)]));
        assert_eq!(cloned["third"].to_bits(), (-0.0f64).to_bits());

        for index in 0..70 {
            ports.insert(format!("temporary_{index}"), 0.0);
        }
        let previous_layout = ports.layout_key();
        for index in 0..70 {
            ports.remove(&format!("temporary_{index}"));
        }
        assert_ne!(ports.layout_key(), previous_layout);
        assert!(!copy(&mut ports, &[("third", precise)]));
        assert_eq!(ports["third"].to_bits(), precise.to_bits());
    }

    #[test]
    fn port_name_set_key_scalar_map_tracks_name_shape_separately_from_values() {
        let mut ports = ScalarPortMap::default();
        let empty_key = ports.topology_key();
        let empty_names_key = port_name_set_key(std::iter::empty::<&String>());

        assert_eq!(ports.insert("thrust".into(), 1.0), None);
        let thrust_key = ports.topology_key();
        assert_ne!(thrust_key, empty_key);
        let thrust_names_key = port_name_set_key(ports.keys());
        assert_ne!(thrust_names_key, empty_names_key);

        assert_eq!(ports.insert("thrust".into(), 2.0), Some(1.0));
        assert_eq!(ports.topology_key(), thrust_key);
        *ports.get_mut("thrust").unwrap() = 3.0;
        assert_eq!(ports.topology_key(), thrust_key);
        assert!(!ports.set("thrust", 3.0));
        assert!(ports.set("thrust", 4.0));
        assert_eq!(ports.set_existing("thrust", 4.0), Some(false));
        assert_eq!(ports.set_existing("thrust", 5.0), Some(true));
        assert_eq!(ports.set_existing("missing", 5.0), None);
        assert_eq!(ports.topology_key(), thrust_key);

        assert!(ports.set("torque", 4.0));
        let both_key = ports.topology_key();
        assert_ne!(both_key, thrust_key);
        assert_eq!(ports.remove("torque"), Some(4.0));
        assert_eq!(port_name_set_key(ports.keys()), thrust_names_key);
        assert_ne!(ports.topology_key(), thrust_key);

        ports.clear();
        assert_ne!(ports.topology_key(), empty_key);
        assert_eq!(port_name_set_key(ports.keys()), empty_names_key);
    }

    #[test]
    fn resolved_scalar_slots_survive_unrelated_edits_and_reject_retired_entries() {
        let mut ports = ScalarPortMap::default();
        ports.insert("thrust".into(), 1.0);
        let initial = ports.resolve_slot("thrust").unwrap();

        *ports.get_slot_mut(initial).unwrap() = 2.0;
        assert_eq!(ports.get("thrust"), Some(&2.0));
        assert_eq!(ports.get_slot(initial), Some(&2.0));

        ports.insert("torque".into(), 3.0);
        assert_eq!(
            ports.get_slot(initial),
            Some(&2.0),
            "other slots stay stable"
        );
        let after_insert = ports.resolve_slot("thrust").unwrap();
        assert_eq!(ports.get_slot(after_insert), Some(&2.0));

        let clone = ports.clone();
        assert_eq!(
            clone.get_slot(after_insert),
            None,
            "clones get distinct layouts"
        );

        assert_eq!(ports.remove("thrust"), Some(2.0));
        assert_eq!(ports.get_slot(after_insert), None);
        ports.insert("replacement".into(), 4.0);
        assert_eq!(
            ports.get_slot(after_insert),
            None,
            "reused slots cannot alias"
        );

        let retained = ports.resolve_slot("torque").unwrap();
        for index in 0..64 {
            let name = format!("transient_{index}");
            ports.insert(name.clone(), index as f64);
            ports.remove(&name);
        }
        assert_eq!(
            ports.get_slot(retained),
            None,
            "compaction retires old layouts"
        );
        let compacted = ports.resolve_slot("torque").unwrap();
        assert_eq!(ports.get_slot(compacted), Some(&3.0));
    }

    #[test]
    fn port_name_set_key_is_order_independent_and_reversible() {
        let mut key = PortNameSetKey::default();
        key.insert("height");
        key.insert("velocity");
        let combined = key.finish();

        let names = ["velocity".to_owned(), "height".to_owned()];
        assert_eq!(combined, port_name_set_key(names.iter()));

        key.remove("height");
        assert_eq!(key.finish(), port_name_set_key(std::iter::once(&names[0])));
    }

    fn duplicate_inout_list(_world: &World, _entity: Entity, out: &mut PortDeclarationQuery<'_>) {
        out.declare("release", PortDirection::InOut);
        out.declare("release", PortDirection::InOut);
    }

    fn owner_a_metadata(
        _world: &World,
        _entity: Entity,
        _name: &str,
        direction: PortDirection,
    ) -> PortMetadata {
        PortMetadata::scalar(
            direction,
            None,
            None,
            None,
            "Modelica/OBC",
            "solver",
            false,
            None,
        )
    }

    fn owner_b_metadata(
        _world: &World,
        _entity: Entity,
        _name: &str,
        direction: PortDirection,
    ) -> PortMetadata {
        PortMetadata::scalar(
            direction,
            None,
            None,
            None,
            "dock/runtime actuator",
            "actuator",
            true,
            None,
        )
    }

    fn resolve_release_slot(_world: &World, _entity: Entity, name: &str) -> Option<u64> {
        (name == "release").then_some(0)
    }

    fn accept_release_slot(_world: &mut World, _entity: Entity, slot: u64, _value: f64) {
        assert_eq!(slot, 0, "prepared release slot is valid");
    }

    fn trace_release_slot(world: &mut World, _entity: Entity, slot: u64, _value: f64) {
        assert_eq!(slot, 0, "prepared release slot is valid");
        world.resource_mut::<ShadowWriteTrace>().0 += 1;
    }

    fn no_read(_world: &World, _entity: Entity, _name: &str) -> Option<f64> {
        None
    }

    const OWNER_A_INPUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        declare_ports: duplicate_input_list,
        metadata: owner_a_metadata,
        read_output: no_read,
        read_input: no_read,
        resolve_input: Some(resolve_release_slot),
        write_slot: Some(accept_release_slot),
        resolve_output: None,
        read_slot: None,
        read_input_slot: None,
    };

    const OWNER_B_INPUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        declare_ports: duplicate_input_list,
        metadata: owner_b_metadata,
        read_output: no_read,
        read_input: no_read,
        resolve_input: Some(resolve_release_slot),
        write_slot: Some(accept_release_slot),
        resolve_output: None,
        read_slot: None,
        read_input_slot: None,
    };

    const OWNER_A_INOUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        declare_ports: duplicate_inout_list,
        metadata: owner_a_metadata,
        read_output: no_read,
        read_input: no_read,
        resolve_input: Some(resolve_release_slot),
        write_slot: Some(accept_release_slot),
        resolve_output: None,
        read_slot: None,
        read_input_slot: None,
    };

    const OWNER_B_INOUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        declare_ports: duplicate_inout_list,
        metadata: owner_b_metadata,
        read_output: no_read,
        read_input: no_read,
        resolve_input: Some(resolve_release_slot),
        write_slot: Some(accept_release_slot),
        resolve_output: None,
        read_slot: None,
        read_input_slot: None,
    };

    #[test]
    fn declared_input_owner_is_never_shadowed_by_a_lower_precedence_slot() {
        #[derive(Component)]
        struct StatePort(f64);

        #[derive(Component)]
        struct ShadowedInput(f64);

        let mut world = test_world();
        let entity = world.spawn((StatePort(0.25), ShadowedInput(0.75))).id();
        let mut registry = PortRegistry::default();
        registry.register(PortBackend {
            list_entities: |_world, _out| {},
            topology_key: |_world, _entity| 1,
            declare_ports: |world, entity, out| {
                if world.get::<StatePort>(entity).is_some() {
                    out.declare("shared", PortDirection::InOut);
                }
            },
            metadata: read_only_test_metadata,
            read_output: |world, entity, name| {
                (name == "shared")
                    .then(|| world.get::<StatePort>(entity).map(|state| state.0))
                    .flatten()
            },
            read_input: |world, entity, name| {
                (name == "shared")
                    .then(|| world.get::<StatePort>(entity).map(|state| state.0))
                    .flatten()
            },
            resolve_output: None,
            resolve_input: None,
            read_slot: None,
            read_input_slot: None,
            write_slot: None,
        });
        registry.register(PortBackend {
            list_entities: |_world, _out| {},
            topology_key: |_world, _entity| 1,
            declare_ports: |world, entity, out| {
                if world.get::<ShadowedInput>(entity).is_some() {
                    out.declare("shared", PortDirection::In);
                }
            },
            metadata: test_metadata,
            read_output: |_, _, _| None,
            read_input: |world, entity, name| {
                (name == "shared")
                    .then(|| world.get::<ShadowedInput>(entity).map(|input| input.0))
                    .flatten()
            },
            resolve_output: None,
            resolve_input: Some(|world, entity, name| {
                (name == "shared" && world.get::<ShadowedInput>(entity).is_some()).then_some(0)
            }),
            read_slot: None,
            read_input_slot: Some(|world, entity, slot| {
                (slot == 0)
                    .then(|| world.get::<ShadowedInput>(entity).map(|input| input.0))
                    .flatten()
            }),
            write_slot: Some(|world, entity, slot, value| {
                assert_eq!(slot, 0, "prepared shadow slot is valid");
                let mut input = world
                    .get_mut::<ShadowedInput>(entity)
                    .expect("prepared shadow slot still has its input component");
                input.0 = value;
            }),
        });

        let owners = registry.input_port_owners(&world, entity);
        let owner = owners
            .iter()
            .find(|owner| owner.name == "shared")
            .expect("the shared input owner is declared");
        assert_eq!(owner.direction, PortDirection::InOut);
        assert_eq!(owner.precedence, 1);
        assert_eq!(
            registry.resolve_input(&world, entity, "shared"),
            None,
            "the lower In owner's slot cannot bypass the winning InOut owner"
        );
        assert!(matches!(
            registry.write_port(&mut world, entity, "shared", 0.5),
            Err(super::PortWriteError {
                kind: super::PortWriteErrorKind::NotWritable,
                ..
            })
        ));
        assert_eq!(world.get::<StatePort>(entity).unwrap().0, 0.25);
        assert_eq!(world.get::<ShadowedInput>(entity).unwrap().0, 0.75);
    }

    #[test]
    fn generic_input_ports_are_listed_and_written_through_the_registry() {
        let mut world = test_world();
        let entity = world.spawn(InputPorts::new(&["throttle", "arm"])).id();
        let registry = PortRegistry::default();

        assert_eq!(
            registry.read_input_port(&world, entity, "throttle"),
            Some(0.0)
        );
        world.clear_trackers();
        assert!(
            registry
                .write_port(&mut world, entity, "throttle", 0.0)
                .is_ok()
        );
        assert!(
            !world
                .entity(entity)
                .get_ref::<InputPorts>()
                .unwrap()
                .is_changed(),
            "writing an unchanged command must not redirty its endpoint"
        );
        assert!(
            registry
                .write_port(&mut world, entity, "throttle", 0.75)
                .is_ok()
        );
        assert!(
            world
                .entity(entity)
                .get_ref::<InputPorts>()
                .unwrap()
                .is_changed(),
            "a changed command must still publish its new value"
        );
        assert!(
            registry
                .write_port(&mut world, entity, "undeclared", 1.0)
                .is_err()
        );
        assert_eq!(
            registry.read_input_port(&world, entity, "throttle"),
            Some(0.75)
        );

        let ports = registry.entity_port_owners(&world, entity);
        assert!(
            ports
                .iter()
                .any(|port| port.name == "arm" && port.direction == super::PortDirection::In)
        );
    }

    #[test]
    fn resolved_input_write_preflights_once_and_rejects_invalid_or_stale_contracts() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        #[derive(Component)]
        struct Input {
            value: f64,
            writable: bool,
        }
        #[derive(Resource, Default)]
        struct MetadataReads(AtomicUsize);
        let mut world = test_world();
        world.init_resource::<MetadataReads>();
        let entity = world
            .spawn(Input {
                value: 0.0,
                writable: true,
            })
            .id();
        let registry = PortRegistry {
            backends: vec![PortBackend {
                list_entities: |_, _| {},
                topology_key: |_, _| 0,
                declare_ports: |world, entity, ports| {
                    if world.get::<Input>(entity).is_some() {
                        ports.declare("signal", PortDirection::In);
                    }
                },
                metadata: |world, entity, _, direction| {
                    world
                        .resource::<MetadataReads>()
                        .0
                        .fetch_add(1, Ordering::Relaxed);
                    PortMetadata::scalar(
                        direction,
                        None,
                        Some(0.0),
                        Some(1.0),
                        "probe",
                        "probe",
                        world.get::<Input>(entity).unwrap().writable,
                        None,
                    )
                },
                read_output: no_read,
                read_input: no_read,
                resolve_output: None,
                resolve_input: Some(|world, entity, name| {
                    (name == "signal" && world.get::<Input>(entity).is_some()).then_some(7)
                }),
                read_slot: None,
                read_input_slot: None,
                write_slot: Some(|world, entity, slot, value| {
                    assert_eq!(slot, 7);
                    world.get_mut::<Input>(entity).unwrap().value = value;
                }),
            }],
        };
        let locator = registry.resolve_input(&world, entity, "signal").unwrap();
        world
            .resource::<MetadataReads>()
            .0
            .store(0, Ordering::Relaxed);
        registry
            .write_resolved(&mut world, entity, &locator, 0.5)
            .unwrap();
        assert_eq!(
            world.resource::<MetadataReads>().0.load(Ordering::Relaxed),
            1
        );
        for value in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert!(matches!(
                registry.write_resolved(&mut world, entity, &locator, value),
                Err(super::PortWriteError {
                    kind: super::PortWriteErrorKind::InvalidValue { .. },
                    ..
                })
            ));
            assert_eq!(world.get::<Input>(entity).unwrap().value, 0.5);
        }
        let other = world
            .spawn(Input {
                value: 0.0,
                writable: true,
            })
            .id();
        let first = registry
            .prepare_input_write(&world, entity, "signal", 0.75)
            .unwrap();
        let second = registry
            .prepare_input_write(&world, other, "signal", 0.25)
            .unwrap();
        world.get_mut::<Input>(other).unwrap().writable = false;
        assert!(
            registry
                .apply_prepared_input_writes(&mut world, &[first.clone(), second])
                .is_err()
        );
        assert_eq!(world.get::<Input>(entity).unwrap().value, 0.5);
        assert_eq!(world.get::<Input>(other).unwrap().value, 0.0);
        assert!(matches!(
            registry.apply_prepared_input_writes(&mut world, &[first.clone(), first]),
            Err(super::PortWriteError {
                kind: super::PortWriteErrorKind::DuplicateWrite,
                ..
            })
        ));
        assert_eq!(world.get::<Input>(entity).unwrap().value, 0.5);
        world.get_mut::<Input>(entity).unwrap().writable = false;
        assert!(matches!(
            registry.write_resolved(&mut world, entity, &locator, 0.75),
            Err(super::PortWriteError {
                kind: super::PortWriteErrorKind::NotWritable,
                ..
            })
        ));
        let read_only = registry.resolve_input(&world, entity, "signal").unwrap();
        assert!(matches!(
            registry.write_resolved(&mut world, entity, &read_only, 0.75),
            Err(super::PortWriteError {
                kind: super::PortWriteErrorKind::NotWritable,
                ..
            })
        ));
        world.get_mut::<Input>(entity).unwrap().writable = true;
        let current = registry.resolve_input(&world, entity, "signal").unwrap();
        world.resource_mut::<PortTopologyRevision>().bump();
        assert!(matches!(
            registry.write_resolved(&mut world, entity, &current, 0.75),
            Err(super::PortWriteError {
                kind: super::PortWriteErrorKind::StaleResolution,
                ..
            })
        ));
        let current = registry.resolve_input(&world, entity, "signal").unwrap();
        world.entity_mut(entity).remove::<Input>();
        assert!(
            registry
                .write_resolved(&mut world, entity, &current, 0.75)
                .is_err()
        );
    }

    #[test]
    fn generic_input_resolved_slot_updates_samples_and_rejects_retired_entries() {
        let mut world = test_world();
        let entity = world.spawn(InputPorts::new(&["throttle"])).id();
        let registry = PortRegistry::default();
        let slot = registry.resolve_input(&world, entity, "throttle").unwrap();

        world.clear_trackers();
        assert!(
            registry
                .write_resolved(&mut world, entity, &slot, 0.5)
                .is_ok()
        );
        assert_eq!(
            registry.read_input_port(&world, entity, "throttle"),
            Some(0.5)
        );
        assert!(
            world
                .entity(entity)
                .get_ref::<InputPorts>()
                .unwrap()
                .is_changed()
        );

        world.clear_trackers();
        assert!(
            registry
                .write_resolved(&mut world, entity, &slot, 0.5)
                .is_ok()
        );
        assert!(
            !world
                .entity(entity)
                .get_ref::<InputPorts>()
                .unwrap()
                .is_changed()
        );

        {
            let mut inputs = world.get_mut::<InputPorts>(entity).unwrap();
            inputs.values.remove("throttle");
            inputs.values.insert("throttle".into(), 0.0);
        }
        assert!(
            registry
                .write_resolved(&mut world, entity, &slot, 0.75)
                .is_err(),
            "retired handles must not fall back to a name lookup"
        );
        let current = registry.resolve_input(&world, entity, "throttle").unwrap();
        assert!(
            registry
                .write_resolved(&mut world, entity, &current, 0.75)
                .is_ok()
        );
        assert_eq!(
            registry.read_input_port(&world, entity, "throttle"),
            Some(0.75)
        );
    }

    #[test]
    fn readable_input_sources_resolve_to_input_slots() {
        let mut world = test_world();
        let entity = world.spawn(InputPorts::new(&["throttle"])).id();
        let registry = PortRegistry::default();
        let slot = registry
            .resolve_input_read(&world, entity, "throttle")
            .expect("declared input has a readable slot");

        assert_eq!(registry.read_resolved(&world, entity, &slot), Some(0.0));
        assert!(
            registry
                .write_resolved(&mut world, entity, &slot, 0.75)
                .is_ok()
        );
        assert_eq!(registry.read_resolved(&world, entity, &slot), Some(0.75));

        {
            let mut inputs = world.get_mut::<InputPorts>(entity).unwrap();
            inputs.values.remove("throttle");
            inputs.values.insert("throttle".into(), 0.25);
        }
        assert_eq!(
            registry.read_resolved(&world, entity, &slot),
            None,
            "a retired input slot is rejected without a name lookup"
        );

        let current = registry
            .resolve_input_read(&world, entity, "throttle")
            .expect("the current input surface resolves after its owner rebuilds");
        assert_eq!(registry.read_resolved(&world, entity, &current), Some(0.25));
    }

    #[test]
    fn topology_key_ignores_live_values_but_tracks_port_identity() {
        let mut world = test_world();
        let entity = world.spawn(InputPorts::new(&["throttle"])).id();
        let registry = PortRegistry::default();
        let (handle, info) = registry
            .entity_port_infos_with_handles(&world, entity)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(info.name, "throttle");
        let initial = registry.entity_port_topology_key(&world, entity);

        assert!(
            registry
                .write_port(&mut world, entity, "throttle", 0.75)
                .is_ok()
        );
        assert_eq!(registry.entity_port_topology_key(&world, entity), initial);
        assert_eq!(
            registry.read_port_for_handle(&world, handle, entity, "throttle", PortDirection::In,),
            Some(0.75)
        );

        world
            .get_mut::<InputPorts>(entity)
            .unwrap()
            .values
            .insert("steer".into(), 0.0);
        assert_ne!(registry.entity_port_topology_key(&world, entity), initial);
    }

    #[test]
    fn input_inspection_handle_uses_the_input_slot_reader() {
        #[derive(Component)]
        struct ProbeInput(f64);

        let mut world = test_world();
        let entity = world.spawn(ProbeInput(0.5)).id();
        let mut registry = PortRegistry::default();
        registry.register(PortBackend {
            list_entities: |world, out| {
                out.extend(
                    world
                        .query_filtered::<Entity, With<ProbeInput>>()
                        .iter(world),
                );
            },
            topology_key: |world, entity| u64::from(world.get::<ProbeInput>(entity).is_some()),
            declare_ports: |world, entity, out| {
                if world.get::<ProbeInput>(entity).is_some() {
                    out.declare("shared", PortDirection::In);
                }
            },
            metadata: read_only_test_metadata,
            read_output: |_, _, _| None,
            read_input: |world, entity, name| {
                (name == "shared")
                    .then(|| world.get::<ProbeInput>(entity).map(|input| input.0))
                    .flatten()
            },
            resolve_output: None,
            resolve_input: Some(|world, entity, name| {
                (name == "shared" && world.get::<ProbeInput>(entity).is_some()).then_some(0)
            }),
            read_slot: Some(|_, _, _| Some(-1.0)),
            read_input_slot: Some(|world, entity, slot| {
                (slot == 0)
                    .then(|| world.get::<ProbeInput>(entity).map(|input| input.0))
                    .flatten()
            }),
            write_slot: None,
        });

        let (handle, info) = registry
            .entity_port_infos_with_handles(&world, entity)
            .pop()
            .unwrap();
        assert_eq!(info.name, "shared");
        assert_eq!(
            registry.read_port_for_handle(&world, handle, entity, "shared", PortDirection::In),
            Some(0.5)
        );
    }

    #[test]
    fn resolved_port_presence_skips_full_surface_listing() {
        let mut world = test_world();
        let entity = world.spawn_empty().id();
        let mut registry = PortRegistry::default();
        registry.register(PortBackend {
            list_entities: |_, _| {},
            topology_key: |_, _| 0,
            declare_ports: |_, _, _| panic!("resolved presence must not enumerate port rows"),
            metadata: read_only_test_metadata,
            read_output: |_, _, _| None,
            read_input: |_, _, _| None,
            resolve_output: Some(|_, _, name| (name == "signal").then_some(0)),
            resolve_input: Some(|_, _, name| (name == "signal").then_some(0)),
            read_slot: Some(|_, _, _| None),
            read_input_slot: Some(|_, _, _| None),
            write_slot: None,
        });

        assert!(registry.has_output_port(&world, entity, "signal"));
        assert!(registry.has_input_port(&world, entity, "signal"));
    }

    #[test]
    fn declared_output_resolves_before_its_first_sample_and_respects_owner_order() {
        #[derive(Component)]
        struct DeclaredOutput;

        let mut world = test_world();
        let entity = world.spawn(DeclaredOutput).id();
        let mut registry = PortRegistry::default();
        registry.register(PortBackend {
            list_entities: |_, _| {},
            topology_key: |_, _| 1,
            declare_ports: |_, _, out| {
                out.declare("signal", PortDirection::Out);
            },
            metadata: test_metadata,
            read_output: |_, _, _| None,
            read_input: |_, _, _| None,
            resolve_output: Some(|world, entity, name| {
                (name == "signal" && world.get::<DeclaredOutput>(entity).is_some()).then_some(7)
            }),
            resolve_input: None,
            read_slot: Some(|world, entity, slot| {
                (slot == 7 && world.get::<DeclaredOutput>(entity).is_some()).then_some(2.5)
            }),
            read_input_slot: None,
            write_slot: None,
        });

        let resolved = registry
            .resolve_output(&world, entity, "signal")
            .expect("the declared source resolves before its first sample");
        assert_eq!(registry.read_resolved(&world, entity, &resolved), Some(2.5));

        let mut precedence = PortRegistry::default();
        precedence.register(PortBackend {
            list_entities: |_, _| {},
            topology_key: |_, _| 1,
            declare_ports: |_, _, _| {},
            metadata: read_only_test_metadata,
            read_output: |_, _, name| (name == "signal").then_some(1.0),
            read_input: |_, _, _| None,
            resolve_output: None,
            resolve_input: None,
            read_slot: None,
            read_input_slot: None,
            write_slot: None,
        });
        precedence.register(PortBackend {
            list_entities: |_, _| {},
            topology_key: |_, _| 1,
            declare_ports: |_, _, _| {},
            metadata: read_only_test_metadata,
            read_output: |_, _, name| (name == "signal").then_some(2.0),
            read_input: |_, _, _| None,
            resolve_output: Some(|_, _, name| (name == "signal").then_some(8)),
            resolve_input: None,
            read_slot: Some(|_, _, slot| (slot == 8).then_some(2.0)),
            read_input_slot: None,
            write_slot: None,
        });
        assert_eq!(
            precedence.resolve_output(&world, entity, "signal"),
            None,
            "a lower-priority slot owner must not shadow the readable owner"
        );
    }

    #[test]
    fn registry_discovers_backend_owned_entities_once() {
        let mut world = test_world();
        let first = world.spawn(InputPorts::new(&["first"])).id();
        let second = world.spawn(InputPorts::new(&["second"])).id();
        world.spawn_empty();

        let mut registry = PortRegistry::default();
        registry.register(PortBackend {
            list_entities: |world, out| {
                out.extend(
                    world
                        .query_filtered::<Entity, With<InputPorts>>()
                        .iter(world),
                );
            },
            topology_key: |_world, _entity| 0,
            declare_ports: duplicate_input_list,
            metadata: read_only_test_metadata,
            read_output: no_read,
            read_input: no_read,
            resolve_output: None,
            resolve_input: None,
            read_slot: None,
            read_input_slot: None,
            write_slot: None,
        });

        let entities = registry.port_entities(&mut world);
        assert_eq!(entities.len(), 2);
        assert!(entities.contains(&first));
        assert!(entities.contains(&second));
    }

    #[test]
    fn generic_input_metadata_declares_only_the_known_scalar_contract() {
        let mut world = test_world();
        let entity = world
            .spawn(InputPorts::new(&["throttle", "arm", "speed_boost"]))
            .id();
        let registry = PortRegistry::default();

        let infos = registry.entity_port_infos(&world, entity);
        let throttle = infos.iter().find(|port| port.name == "throttle").unwrap();
        assert_eq!(throttle.metadata.value_type, PortValueType::Scalar);
        assert_eq!(throttle.metadata.min, None);
        assert_eq!(throttle.metadata.max, None);
        assert_eq!(throttle.metadata.source, "input surface");
        assert!(throttle.metadata.writable);
        assert!(throttle.metadata.validate(1.0).is_ok());
        assert!(throttle.metadata.validate(100.0).is_ok());

        let speed_boost = infos
            .iter()
            .find(|port| port.name == "speed_boost")
            .unwrap();
        assert_eq!(speed_boost.metadata.min, None);
        assert_eq!(speed_boost.metadata.max, None);

        let arm = infos.iter().find(|port| port.name == "arm").unwrap();
        assert_eq!(arm.metadata.min, None);
        assert_eq!(arm.metadata.max, None);
        assert!(arm.metadata.writable);
    }

    #[test]
    fn registry_reports_distinct_input_owners_in_write_precedence_order() {
        let mut world = test_world();
        let entity = world.spawn_empty().id();
        let mut registry = PortRegistry::default();
        registry.register(OWNER_A_INPUT);
        registry.register(OWNER_B_INPUT);

        let collisions = registry.entity_port_collisions(&world, entity);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].direction, PortCollisionDirection::Input);
        assert_eq!(collisions[0].name, "release");
        assert_eq!(collisions[0].owners.len(), 2);
        assert_eq!(collisions[0].owners[0].metadata.source, "Modelica/OBC");
        assert_eq!(collisions[0].owners[0].precedence, 1);
        assert_eq!(
            collisions[0].owners[1].metadata.source,
            "dock/runtime actuator"
        );
        assert_eq!(collisions[0].owners[1].precedence, 2);
        assert!(matches!(
            registry.write_port(&mut world, entity, "release", 1.0),
            Err(super::PortWriteError {
                kind: super::PortWriteErrorKind::NotWritable,
                ..
            })
        ));
    }

    #[test]
    fn registry_reports_inout_collision_once_for_both_access_sides() {
        let mut world = test_world();
        let entity = world.spawn_empty().id();
        let mut registry = PortRegistry::default();
        registry.register(OWNER_A_INOUT);
        registry.register(OWNER_B_INOUT);

        let collisions = registry.entity_port_collisions(&world, entity);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].direction, PortCollisionDirection::InOut);
        assert_eq!(collisions[0].owners.len(), 2);
    }

    #[derive(Resource, Default)]
    struct ShadowWriteTrace(usize);

    #[test]
    fn metadata_reads_and_writes_stop_at_the_same_input_owner() {
        let mut world = test_world();
        let entity = world.spawn_empty().id();
        world.insert_resource(ShadowWriteTrace::default());
        let mut registry = PortRegistry::default();
        registry.register(PortBackend {
            list_entities: |_, _| {},
            topology_key: |_, _| 0,
            declare_ports: duplicate_input_list,
            metadata: owner_a_metadata,
            read_output: no_read,
            read_input: no_read,
            resolve_input: Some(resolve_release_slot),
            write_slot: None,
            resolve_output: None,
            read_slot: None,
            read_input_slot: None,
        });
        registry.register(PortBackend {
            list_entities: |_, _| {},
            topology_key: |_, _| 0,
            declare_ports: duplicate_input_list,
            metadata: owner_b_metadata,
            read_output: no_read,
            read_input: |_, _, name| (name == "release").then_some(0.75),
            resolve_input: Some(resolve_release_slot),
            write_slot: Some(trace_release_slot),
            resolve_output: None,
            read_slot: None,
            read_input_slot: None,
        });

        let metadata = registry
            .input_port_metadata(&world, entity, "release")
            .expect("winning owner metadata exists");
        assert_eq!(metadata.source, "Modelica/OBC");
        assert_eq!(
            registry.read_input_port(&world, entity, "release"),
            None,
            "the declared owner has no sample, so the shadow sample is not exposed"
        );
        assert_eq!(
            registry.read_owned_input_port(&world, entity, "release"),
            None,
            "an absent sample from the authoritative owner must not read a shadow owner"
        );
        assert!(
            registry
                .write_port(&mut world, entity, "release", 0.5)
                .is_err(),
            "a refusing authoritative owner must not fall through to a shadow writer"
        );
        assert_eq!(world.resource::<ShadowWriteTrace>().0, 0);
    }
}
