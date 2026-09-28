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
//! enumerator and the access operations (list / read-output / read-input /
//! write-input), registered into the [`PortRegistry`] resource. Discovery and
//! access fold over the registered backends in order, so a new backend is added
//! by **registering** it — no consumer changes. Registration order *is*
//! resolution precedence (first match wins).

use bevy::prelude::*;
use std::any::TypeId;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{Hash, Hasher};
use std::ops::{Deref, Index, IndexMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::InputPorts;

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
        .unwrap_or(0)
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
#[derive(Clone, Debug, Reflect)]
#[reflect(opaque)]
pub struct ScalarPortMap(PortMap<f64>);

impl Default for ScalarPortMap {
    fn default() -> Self {
        Self(PortMap::default())
    }
}

impl ScalarPortMap {
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
        &self.0
    }
}

impl<'a> IntoIterator for &'a ScalarPortMap {
    type Item = (&'a str, &'a f64);
    type IntoIter = PortMapIter<'a, f64>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter()
    }
}

impl<'a> IntoIterator for &'a mut ScalarPortMap {
    type Item = (&'a str, &'a mut f64);
    type IntoIter = PortMapIterMut<'a, f64>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.iter_mut()
    }
}

impl std::ops::DerefMut for ScalarPortMap {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl From<HashMap<String, f64>> for ScalarPortMap {
    fn from(values: HashMap<String, f64>) -> Self {
        Self(values.into())
    }
}

impl FromIterator<(String, f64)> for ScalarPortMap {
    fn from_iter<I: IntoIterator<Item = (String, f64)>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl Extend<(String, f64)> for ScalarPortMap {
    fn extend<I: IntoIterator<Item = (String, f64)>>(&mut self, iter: I) {
        self.0.extend(iter);
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
/// owner changes its declared ports, so a transient event would be lossy. The
/// owner-side structural checks advance this monotonic generation after a
/// component's identity key changes; consumers retain the last generation they
/// projected and rebuild only when it differs. Live port values must not advance
/// it.
#[derive(Resource, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortTopologyRevision(pub u64);

impl PortTopologyRevision {
    /// Advance the invalidation generation after a declared port surface change.
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

/// Metadata describing the value and control contract of a discovered port.
///
/// The runtime currently exposes scalar `f64` values end to end. Keeping that
/// fact here, beside the registry that owns the port surface, gives native UI
/// and API consumers one authoritative place for units, validation, and
/// control ownership instead of making them infer policy from port names.
#[derive(Debug, Clone, PartialEq)]
pub struct PortMetadata {
    /// Stable value kind shown to generic consumers.
    pub value_type: &'static str,
    /// Authored/physical unit, when the owner knows one.
    pub unit: Option<String>,
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
        unit: Option<&str>,
        min: Option<f64>,
        max: Option<f64>,
        source: impl Into<String>,
        authority: impl Into<String>,
        writable: bool,
    ) -> Self {
        Self {
            value_type: "scalar",
            unit: unit.map(str::to_owned),
            min,
            max,
            source: source.into(),
            authority: authority.into(),
            writable: writable && matches!(direction, PortDirection::In | PortDirection::InOut),
        }
    }

    /// Metadata for a backend that has not supplied a richer description.
    pub fn unknown(direction: PortDirection) -> Self {
        Self::scalar(
            direction,
            None,
            None,
            None,
            "unknown backend",
            "backend owner",
            false,
        )
    }

    /// Validate a value before dispatching it to a writable port.
    pub fn validate(&self, value: f64) -> Result<(), String> {
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

/// A discovered port with its live value and owner-provided metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct PortInfo {
    /// Port name — the key in the owning backend, or the canonical name for a
    /// single-value backend.
    pub name: String,
    /// Causality.
    pub direction: PortDirection,
    /// Snapshot of the current value.
    pub value: f64,
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

/// Why the registry could not apply an input write to its precedence winner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortWriteError {
    /// No registered owner declares this input name.
    NoInputOwner,
    /// The winning input owner has a different causality than the caller requires.
    DirectionMismatch {
        /// Causality requested by the caller.
        expected: PortDirection,
        /// Causality declared by the winning owner.
        actual: PortDirection,
    },
    /// The precedence-winning owner declined its own declared input.
    OwnerRejected,
}

/// A discovered port: identity, causality, current value.
///
/// Returned by [`PortRegistry::entity_ports`] for listing/introspection. The
/// `value` is a snapshot read at call time; live consumers read through the
/// registry directly.
#[derive(Debug, Clone)]
pub struct PortRef {
    /// Port name — the key in the owning backend, or the canonical name for a
    /// single-value backend.
    pub name: String,
    /// Causality.
    pub direction: PortDirection,
    /// Snapshot of the current value.
    pub value: f64,
}

/// Append every `(name, value)` in `map` as a [`PortRef`] of direction `dir`.
/// Helper for map-backed backends (e.g. Modelica `inputs`/`outputs`).
#[inline]
pub fn push_map(out: &mut Vec<PortRef>, map: &PortMap<f64>, dir: PortDirection) {
    for (name, value) in map {
        out.push(PortRef {
            name: name.to_string(),
            direction: dir,
            value: *value,
        });
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
/// direction; `write_input` accepts only an existing input slot (the strictness
/// that lets `propagate` report dangling wires). A single-value backend is
/// bidirectional — its one scalar *is* both its output and input.
#[derive(Clone, Copy)]
pub struct PortBackend {
    /// Append entities owned by this backend to `out`.
    ///
    /// This is the backend's authoritative discovery boundary. Consumers that
    /// need to inspect all ports must use [`PortRegistry::port_entities`]
    /// instead of scanning every ECS entity and probing every backend.
    pub list_entities: fn(&mut World, &mut Vec<Entity>),
    /// Return a key for this backend's port identity on `entity`.
    ///
    /// The key must ignore live values and change when the backend's port names
    /// or directions change. The owning plugin's change-filtered structural
    /// check publishes [`PortTopologyRevision`] when that key changes; the key
    /// is evaluated only on the changed-owner path. It lets consumers cache
    /// metadata while still observing dynamic authored surfaces.
    pub topology_key: fn(&World, Entity) -> u64,
    /// Append this backend's ports on `entity` (outputs then inputs) to `out`.
    pub list: fn(&World, Entity, &mut Vec<PortRef>),
    /// Describe one port returned by `list`, or `None` for the generic scalar
    /// fallback. The callback belongs to the backend owner so consumers never
    /// need a second type/name switch to reconstruct its contract.
    pub metadata: Option<fn(&World, Entity, &str, PortDirection) -> PortMetadata>,
    /// Read the **output** named `name`, or `None`.
    pub read_output: fn(&World, Entity, &str) -> Option<f64>,
    /// Read the **input** named `name`, or `None`.
    pub read_input: fn(&World, Entity, &str) -> Option<f64>,
    /// Write `value` to **input** `name`; `true` iff the port existed here.
    pub write_input: fn(&mut World, Entity, &str, f64) -> bool,

    // ── Optional resolve→slot fast path (the FMI valueReference model) ──────────
    //
    // A backend with dynamic names or a multi-owner presence scan can expose
    // these so a hot consumer (the propagation master) resolves an endpoint to
    // a process-local `slot` ONCE and then exchanges by slot every tick — one
    // owner access, no repeated name lookup. Input write ownership and readable
    // input-side sources are resolved separately. `None` means the
    // precedence-winning owner intentionally uses the named operation. See
    // [`PortRegistry::resolve_output`].
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
    /// Write `value` to a previously-resolved input `slot`; `false` if it no
    /// longer backs a live input.
    pub write_slot: Option<fn(&mut World, Entity, u64, f64) -> bool>,
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
/// [`write_resolved`](PortRegistry::write_resolved): the resolver folds over
/// backends ONCE, then the hot loop exchanges by slot with no re-scan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedPort {
    /// Index of the owning backend in the registry (its registration order).
    backend: usize,
    /// Backend-private opaque locator.
    slot: u64,
    /// Causality side used to resolve this locator.
    side: ResolvedPortSide,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResolvedPortSide {
    Input,
    Output,
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
            .map(|inputs| inputs.values.topology_key())
            .unwrap_or(0)
    },
    list: |world, entity, out| {
        if let Some(inputs) = world.get::<InputPorts>(entity) {
            push_map(out, &inputs.values, PortDirection::In);
        }
    },
    metadata: Some(|_world, _entity, name, direction| {
        let (min, max) = match name {
            "throttle" | "steer" | "brake" => (Some(-1.0), Some(1.0)),
            "speed_boost" => (Some(0.0), Some(1.0)),
            _ => (None, None),
        };
        PortMetadata::scalar(
            direction,
            None,
            min,
            max,
            "control surface",
            "control owner",
            true,
        )
    }),
    read_output: |_world, _entity, _name| None,
    read_input: |world, entity, name| {
        world
            .get::<InputPorts>(entity)
            .and_then(|inputs| inputs.values.get(name).copied())
    },
    write_input: |world, entity, name, value| {
        let Some(mut inputs) = world.get_mut::<InputPorts>(entity) else {
            return false;
        };
        let changed = inputs
            .bypass_change_detection()
            .values
            .set_existing(name, value);
        let Some(changed) = changed else {
            return false;
        };
        if changed {
            inputs.set_changed();
        }
        true
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
        let Some(mut inputs) = world.get_mut::<InputPorts>(entity) else {
            return false;
        };
        let changed = inputs
            .bypass_change_detection()
            .values
            .set_slot_existing(slot, value);
        let Some(changed) = changed else {
            return false;
        };
        if changed {
            inputs.set_changed();
        }
        true
    }),
};

fn backend_port_directions(
    backend: &PortBackend,
    world: &World,
    entity: Entity,
) -> BTreeMap<String, PortDirection> {
    let mut ports = Vec::new();
    (backend.list)(world, entity, &mut ports);
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
    by_name
}

impl PortRegistry {
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

    /// Enumerate every exposed port on `entity`, across all backends.
    /// The backbone of `ListPorts`.
    pub fn entity_ports(&self, world: &World, entity: Entity) -> Vec<PortRef> {
        let mut out = Vec::new();
        for backend in &self.backends {
            (backend.list)(world, entity, &mut out);
        }
        out
    }

    /// Enumerate every exposed port with owner-provided metadata.
    ///
    /// This is the native/API inspection surface. The older [`Self::entity_ports`]
    /// remains the compact value-only surface used by compatibility consumers;
    /// both are produced from the same backend list callbacks.
    pub fn entity_port_infos(&self, world: &World, entity: Entity) -> Vec<PortInfo> {
        self.entity_port_infos_with_handles(world, entity)
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
        let mut out = Vec::new();
        for (backend_index, backend) in self.backends.iter().enumerate() {
            let mut ports = Vec::new();
            (backend.list)(world, entity, &mut ports);
            out.extend(ports.into_iter().map(|port| {
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
                (
                    PortHandle {
                        backend: backend_index,
                        slot,
                        reader,
                    },
                    PortInfo {
                        metadata: backend
                            .metadata
                            .map(|describe| describe(world, entity, &port.name, port.direction))
                            .unwrap_or_else(|| PortMetadata::unknown(port.direction)),
                        name: port.name,
                        direction: port.direction,
                        value: port.value,
                    },
                )
            }));
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
            for (name, direction) in backend_port_directions(backend, world, entity) {
                let metadata = backend
                    .metadata
                    .map(|describe| describe(world, entity, &name, direction))
                    .unwrap_or_else(|| PortMetadata::unknown(direction));
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
    /// from its *source*. Searches outputs only (plus bidirectional single-value
    /// ports). Critical when a name exists as both input and output on one entity.
    pub fn read_output_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        self.backends
            .iter()
            .find_map(|b| (b.read_output)(world, entity, name))
    }

    /// Read the current value of port `name`, preferring an **output**, then
    /// falling back to an **input**. The backbone of `GetPort`.
    pub fn read_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        if let Some(v) = self.read_output_port(world, entity, name) {
            return Some(v);
        }
        self.backends
            .iter()
            .find_map(|b| (b.read_input)(world, entity, name))
    }

    /// Read the **input** value of port `name` — the commanded side, skipping
    /// outputs. Use where the input specifically is wanted (e.g. a joint's
    /// commanded motor setpoint vs its measured angle, both named `angle`).
    pub fn read_input_port(&self, world: &World, entity: Entity, name: &str) -> Option<f64> {
        self.backends
            .iter()
            .find_map(|b| (b.read_input)(world, entity, name))
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
        self.backends.iter().any(|backend| {
            if backend
                .resolve_output
                .is_some_and(|resolve| resolve(world, entity, name).is_some())
            {
                return true;
            }
            let mut ports = Vec::new();
            (backend.list)(world, entity, &mut ports);
            ports.iter().any(|port| {
                port.name == name
                    && matches!(port.direction, PortDirection::Out | PortDirection::InOut)
            })
        })
    }

    /// Whether an input port is declared by an owning backend, independently of
    /// its current value.
    pub fn has_input_port(&self, world: &World, entity: Entity, name: &str) -> bool {
        self.backends.iter().any(|backend| {
            if backend
                .resolve_input
                .is_some_and(|resolve| resolve(world, entity, name).is_some())
            {
                return true;
            }
            let mut ports = Vec::new();
            (backend.list)(world, entity, &mut ports);
            ports.iter().any(|port| {
                port.name == name
                    && matches!(port.direction, PortDirection::In | PortDirection::InOut)
            })
        })
    }

    /// Write `value` to the precedence-winning declared input owner for `name`.
    ///
    /// Owner identity and causality come from the backend's public port list;
    /// the write is then sent directly to that owner. An owner that declines its
    /// declared input is an error and does not expose a lower-precedence owner.
    /// Strictly rejects undeclared names, which lets API and propagation callers
    /// report dangling ports instead of silently creating them.
    pub fn write_port(&self, world: &mut World, entity: Entity, name: &str, value: f64) -> bool {
        self.write_input_port(world, entity, name, value).is_ok()
    }

    /// Write `value` through the precedence-winning input owner and return its
    /// declared causality. `InOut` remains eligible for ordinary input writes.
    pub fn write_input_port(
        &self,
        world: &mut World,
        entity: Entity,
        name: &str,
        value: f64,
    ) -> Result<PortDirection, PortWriteError> {
        self.write_input_port_checked(world, entity, name, value, None)
    }

    /// Write `value` only when the precedence-winning input owner declares the
    /// requested causality. The registry resolves and writes through the same
    /// backend, so a lower-precedence owner cannot receive the named write.
    pub fn write_input_port_with_direction(
        &self,
        world: &mut World,
        entity: Entity,
        name: &str,
        value: f64,
        expected: PortDirection,
    ) -> Result<PortDirection, PortWriteError> {
        self.write_input_port_checked(world, entity, name, value, Some(expected))
    }

    fn write_input_port_checked(
        &self,
        world: &mut World,
        entity: Entity,
        name: &str,
        value: f64,
        expected: Option<PortDirection>,
    ) -> Result<PortDirection, PortWriteError> {
        let Some((backend_index, actual)) = self.resolve_input_owner(world, entity, name) else {
            return Err(PortWriteError::NoInputOwner);
        };
        if let Some(expected) = expected
            && expected != actual
        {
            return Err(PortWriteError::DirectionMismatch {
                expected,
                actual,
            });
        }
        let backend = self
            .backends
            .get(backend_index)
            .expect("resolved input owner belongs to this registry");
        if !(backend.write_input)(world, entity, name, value) {
            return Err(PortWriteError::OwnerRejected);
        }
        Ok(actual)
    }

    /// Return the first input-capable owner of `name` in registry order.
    /// Output-only owners do not shadow input owners; `InOut` owners do.
    fn resolve_input_owner(
        &self,
        world: &World,
        entity: Entity,
        name: &str,
    ) -> Option<(usize, PortDirection)> {
        for (backend_index, backend) in self.backends.iter().enumerate() {
            if let Some(direction) = backend_port_directions(backend, world, entity).remove(name)
                && matches!(direction, PortDirection::In | PortDirection::InOut)
            {
                return Some((backend_index, direction));
            }
        }
        None
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
        for (i, b) in self.backends.iter().enumerate() {
            if let Some(resolve) = b.resolve_output {
                if let Some(slot) = resolve(world, entity, name) {
                    return Some(ResolvedPort {
                        backend: i,
                        slot,
                        side: ResolvedPortSide::Output,
                    });
                }
            }
            if (b.read_output)(world, entity, name).is_some() {
                return None;
            }
        }
        None
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
        let (backend_index, _) = self.resolve_input_owner(world, entity, name)?;
        let backend = self.backends.get(backend_index)?;
        let slot = (backend.resolve_input?)(world, entity, name)?;
        Some(ResolvedPort {
            backend: backend_index,
            slot,
            side: ResolvedPortSide::Input,
        })
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
        for (i, backend) in self.backends.iter().enumerate() {
            if (backend.read_input)(world, entity, name).is_some() {
                let slot = (backend.resolve_input?)(world, entity, name)?;
                if backend.read_input_slot.is_none() {
                    return None;
                }
                return Some(ResolvedPort {
                    backend: i,
                    slot,
                    side: ResolvedPortSide::Input,
                });
            }
        }
        None
    }

    /// Read the value at a resolved port. `None` if the slot no longer backs a
    /// live value. A compiled consumer rebuilds handles from the shared topology
    /// revision; this operation never retries through name resolution.
    pub fn read_resolved(&self, world: &World, entity: Entity, r: ResolvedPort) -> Option<f64> {
        let backend = self.backends.get(r.backend)?;
        let read = match r.side {
            ResolvedPortSide::Input => backend.read_input_slot?,
            ResolvedPortSide::Output => backend.read_slot?,
        };
        read(world, entity, r.slot)
    }

    /// Write to a resolved input port. `false` if the slot no longer backs a live
    /// input (component removed) — the caller reports the dangling target.
    pub fn write_resolved(
        &self,
        world: &mut World,
        entity: Entity,
        r: ResolvedPort,
        value: f64,
    ) -> bool {
        if r.side != ResolvedPortSide::Input {
            return false;
        }
        match self
            .backends
            .get(r.backend)
            .and_then(|backend| backend.write_slot)
        {
            Some(write) => write(world, entity, r.slot, value),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PortBackend, PortCollisionDirection, PortDirection, PortMetadata, PortNameSetKey, PortRef,
        PortRegistry, ScalarPortMap, port_name_set_key,
    };
    use crate::InputPorts;
    use bevy::prelude::*;

    fn duplicate_input_list(_world: &World, _entity: Entity, out: &mut Vec<PortRef>) {
        out.push(PortRef {
            name: "release".into(),
            direction: PortDirection::In,
            value: 0.0,
        });
        out.push(PortRef {
            name: "release".into(),
            direction: PortDirection::In,
            value: 0.0,
        });
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

    fn duplicate_inout_list(_world: &World, _entity: Entity, out: &mut Vec<PortRef>) {
        out.push(PortRef {
            name: "release".into(),
            direction: PortDirection::InOut,
            value: 0.0,
        });
        out.push(PortRef {
            name: "release".into(),
            direction: PortDirection::InOut,
            value: 0.0,
        });
    }

    fn owner_a_metadata(
        _world: &World,
        _entity: Entity,
        _name: &str,
        direction: PortDirection,
    ) -> PortMetadata {
        PortMetadata::scalar(direction, None, None, None, "Modelica/OBC", "solver", true)
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
        )
    }

    fn write_input(_world: &mut World, _entity: Entity, _name: &str, _value: f64) -> bool {
        true
    }

    fn no_read(_world: &World, _entity: Entity, _name: &str) -> Option<f64> {
        None
    }

    const OWNER_A_INPUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        list: duplicate_input_list,
        metadata: Some(owner_a_metadata),
        read_output: no_read,
        read_input: no_read,
        write_input,
        resolve_output: None,
        resolve_input: None,
        read_slot: None,
        read_input_slot: None,
        write_slot: None,
    };

    const OWNER_B_INPUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        list: duplicate_input_list,
        metadata: Some(owner_b_metadata),
        read_output: no_read,
        read_input: no_read,
        write_input,
        resolve_output: None,
        resolve_input: None,
        read_slot: None,
        read_input_slot: None,
        write_slot: None,
    };

    const OWNER_A_INOUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        list: duplicate_inout_list,
        metadata: Some(owner_a_metadata),
        read_output: no_read,
        read_input: no_read,
        write_input,
        resolve_output: None,
        resolve_input: None,
        read_slot: None,
        read_input_slot: None,
        write_slot: None,
    };

    const OWNER_B_INOUT: PortBackend = PortBackend {
        list_entities: |_world, _out| {},
        topology_key: |_world, _entity| 0,
        list: duplicate_inout_list,
        metadata: Some(owner_b_metadata),
        read_output: no_read,
        read_input: no_read,
        write_input,
        resolve_output: None,
        resolve_input: None,
        read_slot: None,
        read_input_slot: None,
        write_slot: None,
    };

    #[test]
    fn input_writes_and_fast_resolution_stay_with_the_declared_precedence_owner() {
        #[derive(Component)]
        struct StatePort(f64);

        #[derive(Component)]
        struct ShadowedInput(f64);

        let mut world = World::new();
        let entity = world.spawn((StatePort(0.25), ShadowedInput(0.75))).id();
        let mut registry = PortRegistry::default();
        registry.register(PortBackend {
            list_entities: |_world, _out| {},
            topology_key: |_world, _entity| 1,
            list: |world, entity, out| {
                if let Some(state) = world.get::<StatePort>(entity) {
                    out.push(PortRef {
                        name: "shared".into(),
                        direction: PortDirection::InOut,
                        value: state.0,
                    });
                }
            },
            metadata: None,
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
            write_input: |world, entity, name, value| {
                if name != "shared" {
                    return false;
                }
                let Some(mut state) = world.get_mut::<StatePort>(entity) else {
                    return false;
                };
                state.0 = value;
                true
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
            list: |world, entity, out| {
                if let Some(input) = world.get::<ShadowedInput>(entity) {
                    out.push(PortRef {
                        name: "shared".into(),
                        direction: PortDirection::In,
                        value: input.0,
                    });
                }
            },
            metadata: None,
            read_output: |_, _, _| None,
            read_input: |world, entity, name| {
                (name == "shared")
                    .then(|| world.get::<ShadowedInput>(entity).map(|input| input.0))
                    .flatten()
            },
            write_input: |world, entity, name, value| {
                if name != "shared" {
                    return false;
                }
                let Some(mut input) = world.get_mut::<ShadowedInput>(entity) else {
                    return false;
                };
                input.0 = value;
                true
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
                if slot != 0 {
                    return false;
                }
                let Some(mut input) = world.get_mut::<ShadowedInput>(entity) else {
                    return false;
                };
                input.0 = value;
                true
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
        assert_eq!(
            registry.write_input_port_with_direction(
                &mut world,
                entity,
                "shared",
                0.0,
                PortDirection::In,
            ),
            Err(super::PortWriteError::DirectionMismatch {
                expected: PortDirection::In,
                actual: PortDirection::InOut,
            })
        );
        assert_eq!(world.get::<StatePort>(entity).unwrap().0, 0.25);
        assert_eq!(world.get::<ShadowedInput>(entity).unwrap().0, 0.75);

        assert!(registry.write_port(&mut world, entity, "shared", 0.5));
        assert_eq!(world.get::<StatePort>(entity).unwrap().0, 0.5);
        assert_eq!(world.get::<ShadowedInput>(entity).unwrap().0, 0.75);
    }

    #[test]
    fn generic_input_ports_are_listed_and_written_through_the_registry() {
        let mut world = World::new();
        let entity = world.spawn(InputPorts::new(&["throttle", "arm"])).id();
        let registry = PortRegistry::default();

        assert_eq!(
            registry.read_input_port(&world, entity, "throttle"),
            Some(0.0)
        );
        world.clear_trackers();
        assert!(registry.write_port(&mut world, entity, "throttle", 0.0));
        assert!(
            !world
                .entity(entity)
                .get_ref::<InputPorts>()
                .unwrap()
                .is_changed(),
            "writing an unchanged command must not redirty its endpoint"
        );
        assert!(registry.write_port(&mut world, entity, "throttle", 0.75));
        assert!(
            world
                .entity(entity)
                .get_ref::<InputPorts>()
                .unwrap()
                .is_changed(),
            "a changed command must still publish its new value"
        );
        assert!(!registry.write_port(&mut world, entity, "undeclared", 1.0));
        assert_eq!(
            registry.read_input_port(&world, entity, "throttle"),
            Some(0.75)
        );

        let ports = registry.entity_ports(&world, entity);
        assert!(
            ports
                .iter()
                .any(|port| port.name == "arm" && port.direction == super::PortDirection::In)
        );
    }

    #[test]
    fn generic_input_resolved_slot_updates_samples_and_rejects_retired_entries() {
        let mut world = World::new();
        let entity = world.spawn(InputPorts::new(&["throttle"])).id();
        let registry = PortRegistry::default();
        let slot = registry.resolve_input(&world, entity, "throttle").unwrap();

        world.clear_trackers();
        assert!(registry.write_resolved(&mut world, entity, slot, 0.5));
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
        assert!(registry.write_resolved(&mut world, entity, slot, 0.5));
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
            !registry.write_resolved(&mut world, entity, slot, 0.75),
            "retired handles must not fall back to a name lookup"
        );
        let current = registry.resolve_input(&world, entity, "throttle").unwrap();
        assert!(registry.write_resolved(&mut world, entity, current, 0.75));
        assert_eq!(
            registry.read_input_port(&world, entity, "throttle"),
            Some(0.75)
        );
    }

    #[test]
    fn readable_input_sources_resolve_to_input_slots() {
        let mut world = World::new();
        let entity = world.spawn(InputPorts::new(&["throttle"])).id();
        let registry = PortRegistry::default();
        let slot = registry
            .resolve_input_read(&world, entity, "throttle")
            .expect("declared input has a readable slot");

        assert_eq!(registry.read_resolved(&world, entity, slot), Some(0.0));
        assert!(registry.write_resolved(&mut world, entity, slot, 0.75));
        assert_eq!(registry.read_resolved(&world, entity, slot), Some(0.75));

        {
            let mut inputs = world.get_mut::<InputPorts>(entity).unwrap();
            inputs.values.remove("throttle");
            inputs.values.insert("throttle".into(), 0.25);
        }
        assert_eq!(
            registry.read_resolved(&world, entity, slot),
            None,
            "a retired input slot is rejected without a name lookup"
        );

        let current = registry
            .resolve_input_read(&world, entity, "throttle")
            .expect("the current input surface resolves after its owner rebuilds");
        assert_eq!(registry.read_resolved(&world, entity, current), Some(0.25));
    }

    #[test]
    fn topology_key_ignores_live_values_but_tracks_port_identity() {
        let mut world = World::new();
        let entity = world.spawn(InputPorts::new(&["throttle"])).id();
        let registry = PortRegistry::default();
        let (handle, info) = registry
            .entity_port_infos_with_handles(&world, entity)
            .into_iter()
            .next()
            .unwrap();
        assert_eq!(info.name, "throttle");
        let initial = registry.entity_port_topology_key(&world, entity);

        assert!(registry.write_port(&mut world, entity, "throttle", 0.75));
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

        let mut world = World::new();
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
            list: |world, entity, out| {
                if let Some(input) = world.get::<ProbeInput>(entity) {
                    out.push(PortRef {
                        name: "shared".into(),
                        direction: PortDirection::In,
                        value: input.0,
                    });
                }
            },
            metadata: None,
            read_output: |_, _, _| None,
            read_input: |world, entity, name| {
                (name == "shared")
                    .then(|| world.get::<ProbeInput>(entity).map(|input| input.0))
                    .flatten()
            },
            write_input: |_, _, _, _| false,
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
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut registry = PortRegistry::default();
        registry.register(PortBackend {
            list_entities: |_, _| {},
            topology_key: |_, _| 0,
            list: |_, _, _| panic!("resolved presence must not enumerate port rows"),
            metadata: None,
            read_output: |_, _, _| None,
            read_input: |_, _, _| None,
            write_input: |_, _, _, _| false,
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

        let mut world = World::new();
        let entity = world.spawn(DeclaredOutput).id();
        let mut registry = PortRegistry::default();
        registry.register(PortBackend {
            list_entities: |_, _| {},
            topology_key: |_, _| 1,
            list: |_, _, out| {
                out.push(PortRef {
                    name: "signal".into(),
                    direction: PortDirection::Out,
                    value: 0.0,
                });
            },
            metadata: None,
            read_output: |_, _, _| None,
            read_input: |_, _, _| None,
            write_input: |_, _, _, _| false,
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
        assert_eq!(registry.read_resolved(&world, entity, resolved), Some(2.5));

        let mut precedence = PortRegistry::default();
        precedence.register(PortBackend {
            list_entities: |_, _| {},
            topology_key: |_, _| 1,
            list: |_, _, _| {},
            metadata: None,
            read_output: |_, _, name| (name == "signal").then_some(1.0),
            read_input: |_, _, _| None,
            write_input: |_, _, _, _| false,
            resolve_output: None,
            resolve_input: None,
            read_slot: None,
            read_input_slot: None,
            write_slot: None,
        });
        precedence.register(PortBackend {
            list_entities: |_, _| {},
            topology_key: |_, _| 1,
            list: |_, _, _| {},
            metadata: None,
            read_output: |_, _, name| (name == "signal").then_some(2.0),
            read_input: |_, _, _| None,
            write_input: |_, _, _, _| false,
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
        let mut world = World::new();
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
            list: duplicate_input_list,
            metadata: None,
            read_output: no_read,
            read_input: no_read,
            write_input,
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
    fn generic_input_metadata_exposes_control_bounds_and_write_contract() {
        let mut world = World::new();
        let entity = world
            .spawn(InputPorts::new(&["throttle", "arm", "speed_boost"]))
            .id();
        let registry = PortRegistry::default();

        let infos = registry.entity_port_infos(&world, entity);
        let throttle = infos.iter().find(|port| port.name == "throttle").unwrap();
        assert_eq!(throttle.metadata.value_type, "scalar");
        assert_eq!(throttle.metadata.min, Some(-1.0));
        assert_eq!(throttle.metadata.max, Some(1.0));
        assert_eq!(throttle.metadata.source, "control surface");
        assert!(throttle.metadata.writable);
        assert!(throttle.metadata.validate(1.0).is_ok());
        assert!(throttle.metadata.validate(1.01).is_err());

        let speed_boost = infos
            .iter()
            .find(|port| port.name == "speed_boost")
            .unwrap();
        assert_eq!(speed_boost.metadata.min, Some(0.0));
        assert_eq!(speed_boost.metadata.max, Some(1.0));

        let arm = infos.iter().find(|port| port.name == "arm").unwrap();
        assert_eq!(arm.metadata.min, None);
        assert_eq!(arm.metadata.max, None);
        assert!(arm.metadata.writable);
    }

    #[test]
    fn registry_reports_distinct_input_owners_in_write_precedence_order() {
        let mut world = World::new();
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
        assert!(registry.write_port(&mut world, entity, "release", 1.0));
    }

    #[test]
    fn registry_reports_inout_collision_once_for_both_access_sides() {
        let mut world = World::new();
        let entity = world.spawn_empty().id();
        let mut registry = PortRegistry::default();
        registry.register(OWNER_A_INOUT);
        registry.register(OWNER_B_INOUT);

        let collisions = registry.entity_port_collisions(&world, entity);
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].direction, PortCollisionDirection::InOut);
        assert_eq!(collisions[0].owners.len(), 2);
    }
}
