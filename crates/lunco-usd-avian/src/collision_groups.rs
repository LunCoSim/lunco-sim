//! `UsdPhysicsCollisionGroup` — group-vs-group collision filtering.
//!
//! [`filtered_pairs`](crate::filtered_pairs) says "these two never touch", one
//! pair at a time. That is the right tool for two parts and the wrong one for
//! twenty: a six-wheel rover whose wheels must not touch its own rockers is
//! fifteen rels, and every part added reopens the file. Group filtering is the
//! O(n) form of the same statement — put the wheels in one group, the chassis in
//! another, say the two do not collide, and adding a wheel is one line in a
//! collection.
//!
//! ```usda
//! def PhysicsCollisionGroup "Wheels"
//! {
//!     prepend rel collection:colliders:includes = [</Rover/WheelFL>, </Rover/WheelFR>]
//!     prepend rel physics:filteredGroups = </Scene/Groups/Chassis>
//! }
//! ```
//!
//! # How it maps
//!
//! Each group gets one bit of avian's [`CollisionLayers`]: a collider's
//! `memberships` are its groups' bits, and its `filters` are everything except
//! the bits of the groups it is filtered against. avian collides a pair only when
//! each side's memberships intersect the other's filters, and the blocked-pair
//! table below is built symmetrically, so one authored opinion blocks both
//! directions — which is what the schema means.
//!
//! Three details the schema specifies and this honours:
//!
//! - **`physics:mergeGroup`** — groups carrying the same non-empty merge key ARE
//!   one group, and share a bit. That is how two layers can each contribute
//!   members to the same logical group without either knowing about the other.
//! - **`physics:invertFilteredGroups`** — the listed groups become the only ones
//!   this group DOES collide with. Read literally, including for the group
//!   itself: a group that inverts and does not list itself stops colliding with
//!   its own members.
//! - **Ungrouped bodies keep colliding.** They sit on avian's default bit, which
//!   no group ever blocks, so introducing a group never silently switches off a
//!   contact between two parts that are not in it.
//!
//! # Membership is a `UsdCollectionAPI`
//!
//! `PhysicsCollisionGroup` applies `CollectionAPI:colliders`, so membership is
//! `collection:colliders:includes` / `:excludes` under the standard
//! `expandPrims` rule: an included prim brings its whole subtree, an excluded one
//! takes its subtree back out, and the deepest opinion wins. That is one more
//! reason to prefer this over a bespoke `lunco:group` token — the collection is
//! the same construct material binding and light linking already use.

use std::collections::{HashMap, HashSet};

use avian3d::prelude::*;
use bevy::prelude::*;
use lunco_usd_bevy::{StageView, UsdRead};
use openusd::schemas::physics::tokens as ptok;
use openusd::sdf::Path as SdfPath;

/// Collection instance name `PhysicsCollisionGroup` applies (from the schema:
/// `apiSchemas = ["CollectionAPI:colliders"]`).
const COLLIDERS_COLLECTION: &str = "colliders";

/// Avian layer bit reserved for "in no collision group at all".
///
/// `LayerMask::DEFAULT` is bit 0, and every body that names no group keeps it.
/// Groups are assigned bits from 1 up, so a group's filter mask never clears bit
/// 0 and a grouped body still collides with the ungrouped world.
const UNGROUPED_BIT: u32 = 0;

/// Bits a group may not take: bit 0 is "ungrouped" and bit 7 is the trigger-zone
/// layer (`lunco_core::TRIGGER_COLLISION_LAYER`), which spatial queries mask out
/// by name. A group landing on either would silently redefine it.
fn bit_is_reserved(bit: u32) -> bool {
    bit == UNGROUPED_BIT || (1u32 << bit) == lunco_core::TRIGGER_COLLISION_LAYER
}

/// One merged collision group: its members, and the bits it does not collide with.
#[derive(Debug, Clone)]
struct Group {
    /// Merge key — `physics:mergeGroup` when non-empty, else the prim path. Two
    /// group prims sharing a key are ONE group.
    key: String,
    /// Layer bit index.
    bit: u32,
    /// `collection:colliders:includes` targets, as path prefixes.
    includes: Vec<String>,
    /// `collection:colliders:excludes` targets, as path prefixes.
    excludes: Vec<String>,
    /// Group prims that merged into this one, for diagnostics.
    prims: Vec<String>,
    /// Raw `physics:filteredGroups` targets, resolved to keys after all groups
    /// are known.
    filtered: Vec<String>,
    /// `physics:invertFilteredGroups`.
    invert: bool,
    /// Group bits this group must not collide with. Symmetric, filled last.
    blocked: u32,
}

/// Everything the stage says about collision groups, resolved once.
///
/// Built per stage and cached in [`CollisionGroupTables`], because the question
/// "which groups is this prim in" is stage-global while the loader reads one prim
/// at a time — recomputing it per prim would be quadratic in prim count.
#[derive(Debug, Default, Clone)]
pub struct CollisionGroupTable {
    groups: Vec<Group>,
}

impl CollisionGroupTable {
    /// Read every `PhysicsCollisionGroup` on the stage and resolve merges,
    /// membership and the blocked-pair table.
    pub fn read(reader: &StageView<'_>) -> Self {
        let mut by_key: HashMap<String, Group> = HashMap::new();
        let mut order: Vec<String> = Vec::new();

        let mut paths: Vec<String> = reader
            .prim_paths()
            .into_iter()
            .filter(|p| {
                reader.prim_type_name(p).as_deref() == Some(ptok::T_PHYSICS_COLLISION_GROUP)
            })
            .map(|p| p.to_string())
            .collect();
        // Deterministic bit assignment: the same stage must produce the same
        // layers on every load, or a replay diverges from the run it replays.
        paths.sort();

        for path in paths {
            let Ok(prim) = SdfPath::new(&path) else {
                continue;
            };
            let merge = reader
                .text(&prim, ptok::A_MERGE_GROUP)
                .unwrap_or_default()
                .trim()
                .to_string();
            let key = if merge.is_empty() {
                path.clone()
            } else {
                format!("merge:{merge}")
            };

            let includes = rel_paths(
                reader,
                &prim,
                &format!("collection:{COLLIDERS_COLLECTION}:includes"),
            );
            let excludes = rel_paths(
                reader,
                &prim,
                &format!("collection:{COLLIDERS_COLLECTION}:excludes"),
            );
            let filtered = rel_paths(reader, &prim, ptok::A_FILTERED_GROUPS);
            let invert = reader
                .scalar::<bool>(&prim, ptok::A_INVERT_FILTERED_GROUPS)
                .unwrap_or(false);

            let entry = by_key.entry(key.clone()).or_insert_with(|| {
                order.push(key.clone());
                Group {
                    key: key.clone(),
                    bit: 0,
                    includes: Vec::new(),
                    excludes: Vec::new(),
                    prims: Vec::new(),
                    filtered: Vec::new(),
                    invert: false,
                    blocked: 0,
                }
            });
            entry.includes.extend(includes);
            entry.excludes.extend(excludes);
            entry.filtered.extend(filtered);
            entry.prims.push(path.clone());
            // Merged groups are ONE group, so one of them inverting inverts the
            // group. Disagreement is authoring nobody can satisfy — say so rather
            // than picking by visit order.
            if entry.invert != invert && entry.prims.len() > 1 {
                warn!(
                    "[usd-avian] collision groups {:?} share mergeGroup '{}' but disagree \
                     about {} — taking the inverted reading, which is the stricter one.",
                    entry.prims,
                    key,
                    ptok::A_INVERT_FILTERED_GROUPS,
                );
            }
            entry.invert |= invert;
        }

        // Assign bits in key order, skipping the reserved ones.
        let mut groups: Vec<Group> = Vec::new();
        let mut bit = 0u32;
        for key in &order {
            let mut g = by_key.remove(key).expect("key came from the map");
            while bit_is_reserved(bit) {
                bit += 1;
            }
            if bit >= u32::BITS {
                error!(
                    "[usd-avian] more collision groups than avian has layer bits ({}); \
                     '{}' and any after it are UNFILTERED. Merge groups with \
                     physics:mergeGroup, or filter the remaining pairs with \
                     PhysicsFilteredPairsAPI.",
                    u32::BITS,
                    g.key,
                );
                break;
            }
            g.bit = bit;
            bit += 1;
            groups.push(g);
        }

        // Resolve `filteredGroups` (which names group PRIMS) to bits, then make
        // the relation symmetric: a contact filter that held in one direction
        // only would be a contact that exists depending on which body avian
        // happened to put first.
        let bit_of_prim: HashMap<&str, u32> = groups
            .iter()
            .flat_map(|g| g.prims.iter().map(move |p| (p.as_str(), g.bit)))
            .collect();
        let all_group_bits: u32 = groups.iter().map(|g| 1u32 << g.bit).fold(0, |a, b| a | b);

        let mut blocked: Vec<u32> = vec![0; groups.len()];
        for (i, g) in groups.iter().enumerate() {
            let listed: u32 = g
                .filtered
                .iter()
                .filter_map(|t| {
                    let bit = bit_of_prim.get(t.as_str());
                    if bit.is_none() {
                        warn!(
                            "[usd-avian] collision group {:?}: {} names <{t}>, which is not a \
                             PhysicsCollisionGroup on this stage — that filter does nothing.",
                            g.prims,
                            ptok::A_FILTERED_GROUPS,
                        );
                    }
                    bit
                })
                .map(|b| 1u32 << b)
                .fold(0, |a, b| a | b);
            blocked[i] = if g.invert {
                all_group_bits & !listed
            } else {
                listed
            };
        }
        // Symmetry pass: if A blocks B, B blocks A.
        let bit_index: HashMap<u32, usize> =
            groups.iter().enumerate().map(|(i, g)| (g.bit, i)).collect();
        let snapshot = blocked.clone();
        for (i, mask) in snapshot.iter().enumerate() {
            for (&other_bit, &j) in &bit_index {
                if mask & (1u32 << other_bit) != 0 {
                    blocked[j] |= 1u32 << groups[i].bit;
                }
            }
        }
        for (g, mask) in groups.iter_mut().zip(blocked) {
            g.blocked = mask;
        }

        Self { groups }
    }

    /// Whether this stage authors any collision group at all.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty()
    }

    /// The layers for the collider at `path`, or `None` when it is in no group
    /// — which must leave avian's defaults alone rather than write "collides with
    /// everything", so a trigger zone's explicit layers survive.
    pub fn layers_for(&self, path: &str) -> Option<CollisionLayers> {
        let mut memberships = 0u32;
        let mut blocked = 0u32;
        for g in &self.groups {
            if !g.contains(path) {
                continue;
            }
            memberships |= 1u32 << g.bit;
            blocked |= g.blocked;
        }
        if memberships == 0 {
            return None;
        }
        Some(CollisionLayers::new(
            LayerMask(memberships),
            LayerMask(!blocked),
        ))
    }

    /// Group membership as `(prim, [group keys])`, for diagnostics and tests.
    pub fn groups_of(&self, path: &str) -> Vec<&str> {
        self.groups
            .iter()
            .filter(|g| g.contains(path))
            .map(|g| g.key.as_str())
            .collect()
    }
}

impl Group {
    /// Standard `expandPrims` membership: an include brings its subtree, an
    /// exclude takes a subtree back out, and the DEEPEST opinion wins — so a
    /// group can include a vehicle and exclude one part of it.
    fn contains(&self, path: &str) -> bool {
        let depth = |prefix: &String| -> Option<usize> {
            if path == prefix || path.starts_with(&format!("{prefix}/")) {
                Some(prefix.len())
            } else {
                None
            }
        };
        let inc = self.includes.iter().filter_map(depth).max();
        let exc = self.excludes.iter().filter_map(depth).max();
        match (inc, exc) {
            (Some(i), Some(e)) => i > e,
            (Some(_), None) => true,
            _ => false,
        }
    }
}

fn rel_paths(reader: &StageView<'_>, prim: &SdfPath, rel: &str) -> Vec<String> {
    reader
        .rel_targets(prim, rel)
        .into_iter()
        .map(|p| p.to_string())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Per-stage cache of [`CollisionGroupTable`], keyed by stage asset.
///
/// Cleared on scene teardown with the rest of the world: the groups belong to the
/// scene being replaced, and a stale table would put the next scene's colliders on
/// layers nothing in it defines.
#[derive(Resource, Default)]
pub struct CollisionGroupTables {
    by_stage: HashMap<AssetId<lunco_usd_bevy::UsdStageAsset>, CollisionGroupTable>,
    /// Stages already reported as having groups, so the summary is logged once
    /// rather than per prim.
    announced: HashSet<AssetId<lunco_usd_bevy::UsdStageAsset>>,
}

impl CollisionGroupTables {
    /// The table for `stage`, reading it on first ask.
    pub fn get_or_read(
        &mut self,
        stage: AssetId<lunco_usd_bevy::UsdStageAsset>,
        reader: &StageView<'_>,
    ) -> &CollisionGroupTable {
        let table = self
            .by_stage
            .entry(stage)
            .or_insert_with(|| CollisionGroupTable::read(reader));
        if !table.is_empty() && self.announced.insert(stage) {
            info!(
                "[usd-avian] {} collision group(s) on this stage",
                table.groups.len()
            );
        }
        table
    }

    /// Drop every cached table — scene teardown.
    pub fn clear(&mut self) {
        self.by_stage.clear();
        self.announced.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lunco_usd_bevy::{CanonicalStage, StageRecipe};

    /// Wheels and chassis in two groups that filter each other, plus a body in no
    /// group at all.
    const GROUPS: &str = r#"#usda 1.0
def Xform "Rover"
{
    def Cube "Chassis" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"] ) { double size = 1 }
    def Xform "Wheels"
    {
        def Cube "FL" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"] ) { double size = 1 }
        def Cube "FR" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"] ) { double size = 1 }
    }
}
# OUTSIDE the rover, and so outside every group — `ChassisGroup` includes the
# whole `/Rover` subtree, which would otherwise sweep this up.
def Cube "Boulder" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"] ) { double size = 1 }
def Scope "Groups"
{
    def PhysicsCollisionGroup "WheelGroup"
    {
        prepend rel collection:colliders:includes = </Rover/Wheels>
        prepend rel physics:filteredGroups = </Groups/ChassisGroup>
    }
    def PhysicsCollisionGroup "ChassisGroup"
    {
        prepend rel collection:colliders:includes = </Rover>
        prepend rel collection:colliders:excludes = </Rover/Wheels>
    }
}
"#;

    fn table(src: &str) -> CollisionGroupTable {
        let recipe = StageRecipe::from_source("t.usda", src);
        let cs = CanonicalStage::from_recipe(&recipe).expect("build stage");
        CollisionGroupTable::read(&cs.view())
    }

    /// The deepest opinion wins, which is what lets one group say "the whole
    /// vehicle except its wheels" without listing every part.
    #[test]
    fn a_deeper_exclude_takes_a_subtree_back_out_of_a_group() {
        let t = table(GROUPS);
        assert_eq!(t.groups_of("/Rover/Chassis"), vec!["/Groups/ChassisGroup"]);
        assert_eq!(t.groups_of("/Rover/Wheels/FL"), vec!["/Groups/WheelGroup"]);
    }

    /// One authored opinion blocks BOTH directions. avian collides a pair only if
    /// each side's memberships meet the other's filters, so a one-sided table
    /// would make the contact depend on which body was seen first.
    #[test]
    fn filtering_is_symmetric_even_though_only_one_group_authored_it() {
        let t = table(GROUPS);
        let wheel = t.layers_for("/Rover/Wheels/FL").expect("wheel is grouped");
        let chassis = t.layers_for("/Rover/Chassis").expect("chassis is grouped");

        assert!(
            !wheel.interacts_with(chassis),
            "wheel and chassis still collide: only WheelGroup authored the filter, and \
             the blocked table was not made symmetric"
        );
    }

    /// A body in no group keeps colliding with everything. Introducing a group
    /// must never switch off a contact between two parts that are not in it —
    /// which is why groups never take avian's default bit.
    #[test]
    fn an_ungrouped_body_still_collides_with_a_grouped_one() {
        let t = table(GROUPS);
        assert!(
            t.layers_for("/Boulder").is_none(),
            "the boulder is in no group and must keep avian's default layers"
        );
        let wheel = t.layers_for("/Rover/Wheels/FL").expect("wheel is grouped");
        assert!(
            wheel.interacts_with(CollisionLayers::default()),
            "a grouped wheel stopped colliding with the ungrouped world"
        );
    }

    /// `physics:mergeGroup` makes two group prims ONE group — the same bit, the
    /// union of their members — so two layers can each contribute members without
    /// knowing about each other.
    #[test]
    fn a_shared_merge_group_is_one_group_not_two() {
        let t = table(
            r#"#usda 1.0
def Xform "A" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"] ) {}
def Xform "B" ( prepend apiSchemas = ["PhysicsRigidBodyAPI", "PhysicsCollisionAPI"] ) {}
def Scope "Groups"
{
    def PhysicsCollisionGroup "First"
    {
        prepend rel collection:colliders:includes = </A>
        string physics:mergeGroup = "hull"
    }
    def PhysicsCollisionGroup "Second"
    {
        prepend rel collection:colliders:includes = </B>
        string physics:mergeGroup = "hull"
    }
}
"#,
        );
        assert_eq!(t.groups_of("/A"), t.groups_of("/B"));
        assert_eq!(t.groups_of("/A").len(), 1, "one merged group, not two");
    }
}
