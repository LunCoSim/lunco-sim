//! One content-keyed material cache, shared by both binders.
//!
//! `PbrLook` → `StandardMaterial` and `ShaderLook` → `ShaderMaterial` are the same
//! problem twice: N entities state the same *appearance intent*, and they must
//! collapse onto ONE material handle so they draw in one batch. Terrain depends on
//! it hardest (~150–500 resident tiles → a handful of distinct looks), but the rock
//! field and every repeated USD prim get it too.
//!
//! # Why this is generic rather than written twice
//!
//! It *was* written twice, and the two copies drifted: the shader cache swept dead
//! entries at 1024, the PBR cache never swept at all — same shape, same exposure,
//! one policy. Nothing forced them to agree, so they didn't. Eviction now lives in
//! exactly one place ([`sweep_look_cache`]) and both binders get it by construction.
//!
//! # The two properties this must not give back
//!
//! 1. **Content-keyed sharing.** Equal looks ⇒ one handle ⇒ one bind group. Break
//!    it and the terrain's draw-call count goes linear in the tile count.
//! 2. **`unshared` bypasses the cache entirely.** An ANIMATED look (a USD
//!    `displayColor` sweep, a pulsing highlight) would otherwise re-key on every
//!    distinct value and mint a material per frame that is never freed. That leak
//!    presents as a slow memory climb, not a crash — which is why the opt-out is
//!    explicit rather than inferred. Callers that set `unshared` must MUTATE their
//!    private material in place, never re-`add` it.

use bevy::asset::Asset;
use bevy::platform::collections::{HashMap, HashSet};
use bevy::prelude::*;
use std::hash::Hash;

/// Cached materials tolerated before a sweep runs. A live scene's distinct-look
/// count is in the hundreds at most; anything past this is a dead scene's
/// leftovers — and each entry pins its `Handle<Image>`s (megabytes of GPU texture
/// for a terrain band), so they are worth reclaiming. Below the threshold the
/// steady state pays nothing.
const CACHE_SWEEP_AT: usize = 1024;

/// An appearance-intent component that resolves to a shared material.
///
/// Implemented here (not in the intent crates) on purpose: `lunco-render` and
/// `lunco-materials` are render-free and must not learn what a material is. The
/// trait is local, the types are foreign — the orphan rule allows exactly this.
pub trait CachedLook: Component {
    /// Content hash of everything that affects the resulting material. Two looks
    /// with equal keys MUST be safe to serve with the same handle.
    type Key: Eq + Hash + Clone + Send + Sync + 'static;
    /// The material this look binds to.
    type Material: Asset;

    fn look_key(&self) -> Self::Key;
    /// `true` ⇒ this look owns a private material and never enters the cache.
    fn is_unshared(&self) -> bool;
}

/// Content-key → shared material handle, one instance per look kind.
#[derive(Resource)]
pub struct LookCache<L: CachedLook> {
    map: HashMap<L::Key, Handle<L::Material>>,
}

// Manual: `derive(Default)` would demand `L: Default`, which a Component need not be.
impl<L: CachedLook> Default for LookCache<L> {
    fn default() -> Self {
        Self { map: HashMap::default() }
    }
}

impl<L: CachedLook> LookCache<L> {
    /// Resolve a look to a handle, building the material only on a miss.
    ///
    /// `unshared` looks bypass the cache and get a private material — see the
    /// module docs for why that is load-bearing.
    pub(crate) fn resolve(
        &mut self,
        look: &L,
        materials: &mut Assets<L::Material>,
        build: impl FnOnce(&L) -> L::Material,
    ) -> Handle<L::Material> {
        if look.is_unshared() {
            return materials.add(build(look));
        }
        if let Some(handle) = self.map.get(&look.look_key()) {
            return handle.clone();
        }
        let handle = materials.add(build(look));
        self.map.insert(look.look_key(), handle.clone());
        handle
    }

    /// Number of distinct materials cached (tests / diagnostics).
    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

/// Drop cached materials no live look refers to any more.
///
/// A twin reload or scene swap leaves a dead scene's whole look set behind, each
/// entry pinning its textures. Runs only once the cache is implausibly large, so a
/// steady scene never pays for the `HashSet` build.
pub(crate) fn sweep_look_cache<L: CachedLook>(
    mut cache: ResMut<LookCache<L>>,
    looks: Query<&L>,
) {
    if cache.map.len() <= CACHE_SWEEP_AT {
        return;
    }
    let live: HashSet<L::Key> = looks.iter().map(|l| l.look_key()).collect();
    cache.map.retain(|k, _| live.contains(k));
}
