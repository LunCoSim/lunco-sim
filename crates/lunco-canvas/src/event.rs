//! Events that cross the canvas ↔ caller boundary.
//!
//! [`SceneEvent`] is emitted by the canvas when the user does
//! something that changes *authored* state (moves a node, creates or
//! deletes an edge, focuses a node for drill-in). Callers translate
//! these into domain operations — e.g. the Modelica projector turns
//! `NodeMoved` into a `SetPlacement` `DocumentOp`.
//!
//! [`InputEvent`] is what the canvas passes to the active
//! [`crate::tool::Tool`] each frame. Its shape deliberately abstracts
//! over egui specifics (modifiers as a flat struct, pointer positions
//! in world + screen) so alternative input frontends (test harnesses,
//! scripted replay, remote control) can drive the canvas without
//! pretending to be egui.

use crate::scene::{EdgeId, NodeId, Pos, PortRef};
use crate::selection::Selection;

/// Editor-facing event. One per user action that mutates the scene.
///
/// These are intentionally *coarse*: "the user moved this node to
/// here", not "the user's mouse was at this pixel". The caller
/// typically buffers them for a frame and emits document ops in bulk.
#[derive(Debug, Clone)]
pub enum SceneEvent {
    /// Node position committed after a drag (fired on mouse-up, not
    /// per frame during the drag — keeps undo history readable).
    NodeMoved {
        id: NodeId,
        old_min: Pos,
        new_min: Pos,
    },
    /// User dragged from one port and dropped on a compatible port.
    /// The canvas has already validated that a connection makes sense
    /// (same node ≠, ports exist); domain validity (connector kinds)
    /// is the caller's call via the tool's hook.
    EdgeCreated {
        from: PortRef,
        to: PortRef,
    },
    /// Existing edge was deleted (Delete key, context menu, etc.).
    EdgeDeleted {
        id: EdgeId,
    },
    /// Node was deleted. `orphaned_edges` lists edges that the scene
    /// had to remove because they pointed at the gone node — so the
    /// caller can emit one combined undo entry rather than discovering
    /// the deletions later.
    NodeDeleted {
        id: NodeId,
        orphaned_edges: Vec<EdgeId>,
    },
    /// Selection changed. Carries the *new* selection as a clone so
    /// consumers don't have to reach back into the canvas — useful
    /// for Inspector panels that live in a different thread/frame.
    SelectionChanged(Selection),
    /// Right-click on a scene element (or empty space). The caller
    /// renders the appropriate menu in the next frame — the canvas
    /// doesn't own the menu contents because they're domain-specific
    /// ("Open class", "Convert to parameter", etc.).
    ContextMenuRequested {
        screen_pos: Pos,
        target: Option<ContextTarget>,
    },
    /// Primary-button double-click on a node — "drill into this".
    /// Modelica uses this to open the class definition in a new tab.
    NodeDoubleClicked {
        id: NodeId,
    },
}

/// What a right-click landed on.
#[derive(Debug, Clone, Copy)]
pub enum ContextTarget {
    Node(NodeId),
    Edge(EdgeId),
    Empty,
}

/// Modifier key state — small struct so input backends don't have to
/// import egui types.
#[derive(Debug, Clone, Copy, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// Which pointer button did a thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Primary,
    Secondary,
    Middle,
}

/// Input seen by the active tool.
///
/// Each event carries the pointer position in **both** coordinate
/// systems. World position is what the tool usually needs (hit-
/// testing, placing things); screen position is needed for things
/// like "should this zoom-pivot stay under the cursor".
#[derive(Debug, Clone)]
pub enum InputEvent {
    PointerDown {
        button: MouseButton,
        world: Pos,
        screen: Pos,
        modifiers: Modifiers,
    },
    PointerMove {
        world: Pos,
        screen: Pos,
        modifiers: Modifiers,
    },
    PointerUp {
        button: MouseButton,
        world: Pos,
        screen: Pos,
        modifiers: Modifiers,
    },
    /// Mouse wheel or trackpad scroll. `delta_y > 0` is "scroll up" /
    /// zoom in (matching egui convention).
    Scroll {
        delta_y: f32,
        screen: Pos,
        modifiers: Modifiers,
    },
    /// Key press (not release). `name` is the platform-independent
    /// key identifier (`"Delete"`, `"F"`, `"Escape"`).
    Key {
        name: &'static str,
        modifiers: Modifiers,
    },
    DoubleClick {
        world: Pos,
        screen: Pos,
        modifiers: Modifiers,
    },
}
