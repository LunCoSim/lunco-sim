// The shipped generated Modelica schema.
//
// Rust reads the composed USD graph once and supplies these facts. This policy
// owns the generated source, hierarchy, visual annotations, and layout. The
// Rust seam validates that the returned source represents only the composed
// members, boundary, units, and placements.

// Presentation policy is intentionally tunable here. These are diagram
// coordinates, extents, and typography choices—not Rust/runtime behavior.
const UNIT_POSITION_SCALE = 2;
const HUB_X = -170;
const CAUSAL_HUB_X = -650;
const CAUSAL_HUB_ROW_SPACING = 200;
const BUS_Y = 0;
const BRANCH_FIRST_X = 180;
const BRANCH_LEFT_X = -360;
const BRANCH_LEFT_FIRST_ROW_Y = -80;
const BRANCH_RIGHT_FIRST_ROW_Y = 80;
const BRANCH_COLUMN_SPACING = 180;
const BRANCH_ROW_SPACING = 240;
const BUS_RAIL_MARGIN = 35;

const MEMBER_EXTENT_MARGIN_Y = 240;
const UNIT_MIN_HALF_WIDTH = 240;
const UNIT_MIN_HALF_HEIGHT = 120;
const UNIT_EXTENT_MARGIN_X = 130;
const UNIT_EXTENT_MARGIN_Y = 105;
const DOMAIN_GROUP_GAP = 220;

const COMPONENT_HALF_WIDTH = 55;
const COMPONENT_HALF_HEIGHT = 35;
const BOUNDARY_STUB_LENGTH = 92;
const SHARED_SIGNAL_STUB = 42;
const SHARED_SIGNAL_MIN_TARGETS = 5;
const SHARED_BOUNDARY_MIN_TARGETS = 3;
const SIGNAL_BRANCHES_PER_ROW = 2;
const UNIT_HALF_WIDTH = 110;
const UNIT_HALF_HEIGHT = 55;

// Root-unit placement is presentation policy. Rust supplies only the stable
// connected-component order; this deterministic grid remains editable without
// rebuilding the USD-simulation domain crate.
const GENERATED_UNIT_COLUMN_SPACING = 150;
const GENERATED_UNIT_ROW_SPACING = 100;

const ICON_COORDINATE_HALF_WIDTH = 100;
const ICON_COORDINATE_HALF_HEIGHT = 70;
const ICON_BOX_HALF_WIDTH = 92;
const ICON_BOX_HALF_HEIGHT = 58;
const ICON_CORNER_RADIUS_TEXT = "10";
const DIAGRAM_TITLE_INSET_X = 20;
const DIAGRAM_TITLE_TOP_INSET = 34;
const DIAGRAM_TITLE_BOTTOM_INSET = 12;
const DIAGRAM_TITLE_FONT_SIZE_TEXT = "22";
const DIAGRAM_BUS_FONT_SIZE_TEXT = "18";
const DIAGRAM_SUBSYSTEM_FONT_SIZE_TEXT = "16";
const ICON_TEXT_COLOR = "{255,255,255}";
const UNIT_ICON_LINE_COLOR = "{60,90,145}";
const UNIT_ICON_FILL_COLOR = "{75,130,190}";
const ROOT_ICON_LINE_COLOR = "{70,85,130}";
const ROOT_ICON_FILL_COLOR = "{105,120,180}";
const DIAGRAM_TITLE_COLOR = "{165,175,205}";
const CAUSAL_WIRE_COLOR = "{95,145,225}";
const CAUSAL_WIRE_TEXT_COLOR = "{125,165,230}";
const MEMBER_NAME_COLOR = "{225,235,250}";
const DIAGRAM_MEMBER_FONT_SIZE_TEXT = "28";
const DIAGRAM_SUMMARY_FONT_SIZE_TEXT = "33";
const DIAGRAM_DETAIL_FONT_SIZE_TEXT = "24";

fn string_literal(value) {
    "\"" + value + "\""
}

// Rhai hook functions resolve the long-lived presentation constants above.
// Keep the bus style behind named policy helpers so the route and the
// diagram decoration cannot drift apart.
fn power_rail_x() { HUB_X / 2 }
fn power_rail_color() { DIAGRAM_TITLE_COLOR }
fn power_line_thickness_text() { "2" }

fn rect(x1, y1, x2, y2) {
    "{{" + x1.to_string() + "," + y1.to_string() + "},{"
        + x2.to_string() + "," + y2.to_string() + "}}"
}

fn member_position_index(layout) {
    let result = #{};
    for item in layout.members {
        if !result.contains(item.path) { result[item.path] = item; }
    }
    result
}

fn unit_position_index(layout) {
    let result = #{};
    for item in layout.units {
        if !result.contains(item.name) { result[item.name] = item; }
    }
    result
}

fn position(index, path) {
    if index.contains(path) { return index[path]; }
    if index.contains("members") {
        for item in index.members {
            if item.path == path { return item; }
        }
    }
    throw "generated Modelica layout is missing member position: " + path;
}

fn unit_position(index, name) {
    if index.contains(name) { return index[name]; }
    if index.contains("units") {
        for item in index.units {
            if item.name == name { return item; }
        }
    }
    throw "generated Modelica layout is missing unit position: " + name;
}

fn member_index(members) {
    let result = #{};
    for path in members { result[path] = true; }
    result
}

fn component_index(net) {
    let result = #{};
    for component in net.components {
        // Preserve component_for's first-match behavior if malformed facts
        // ever contain a duplicate path.
        if !result.contains(component.path) { result[component.path] = component; }
    }
    result
}

fn connector_type_for(component, connector) {
    if component.contains("connector_types")
        && component.connector_types.contains(connector) {
        return component.connector_types[connector];
    }
    ""
}

fn connection_type(edge, components) {
    let left_connector = if edge.contains("left_connector") { edge.left_connector } else { "" };
    let right_connector = if edge.contains("right_connector") { edge.right_connector } else { "" };
    let left = connector_type_for(
        component_for(components, edge.left_path), left_connector
    );
    let right = connector_type_for(
        component_for(components, edge.right_path), right_connector
    );
    if left != "" && right != "" && left != right { return "Cross-domain"; }
    if left != "" { return left; }
    if right != "" { return right; }
    "Unclassified"
}

fn connector_domain_label(type_name) {
    if type_name == "Cross-domain" { return "Cross-domain interfaces"; }
    if type_name == "Unclassified" { return "Unclassified connectors"; }
    if type_name == "" { return "Signals and I/O"; }
    let segments = type_name.split(".");
    let member_type = segments[segments.len - 1];
    let domain = if segments.len > 2 {
        segments[1]
    } else if segments.len == 2 {
        segments[0]
    } else {
        "Connector"
    };
    domain + " · " + member_type
}

fn add_domain_count(counts, path, type_name) {
    let member_counts = counts[path];
    if !member_counts.contains(type_name) { member_counts[type_name] = 0; }
    member_counts[type_name] += 1;
    counts[path] = member_counts;
    counts
}

fn connection_domain_counts(net, members, components, membership) {
    let counts = #{};
    for path in members { counts[path] = #{}; }
    for edge in net.connections {
        if !membership.contains(edge.left_path) { continue; }
        if !membership.contains(edge.right_path) { continue; }
        let type_name = connection_type(edge, components);
        if type_name == "Unclassified" { continue; }
        counts = add_domain_count(counts, edge.left_path, type_name);
        counts = add_domain_count(counts, edge.right_path, type_name);
    }
    counts
}

fn member_connector_domain(counts, path, components) {
    let best = "";
    let best_count = 0;
    for type_name in counts[path].keys() {
        let count = counts[path][type_name];
        if count > best_count {
            best = type_name;
            best_count = count;
        }
    }
    if counts[path].keys().len > 1 { return "Cross-domain"; }
    if best != "" { return best; }
    if component_for(components, path).declared_connectors.len > 0 {
        return "Unclassified";
    }
    best
}

fn connector_domains(members, counts, components) {
    let domains = #{};
    for path in members {
        domains[path] = member_connector_domain(counts, path, components);
    }
    domains
}

fn causal_neighbor_domains(net, path, membership, domains) {
    let candidates = [];
    if !net.contains("causal_links") { return candidates; }
    for link in net.causal_links {
        let other = "";
        if link.source_path == path && membership.contains(link.target_path) {
            other = domains[link.target_path];
        } else if link.target_path == path && membership.contains(link.source_path) {
            other = domains[link.source_path];
        }
        if other != "" && !candidates.contains(other) { candidates.push(other); }
    }
    candidates
}

fn propagate_causal_domains(net, members, membership, domains) {
    for iteration in 0..members.len {
        let next = #{};
        for path in members { next[path] = domains[path]; }
        for path in members {
            if domains[path] != "" { continue; }
            let candidates = causal_neighbor_domains(net, path, membership, domains);
            if candidates.len == 1 { next[path] = candidates[0]; }
            if candidates.len > 1 { next[path] = "Cross-domain"; }
        }
        let changed = false;
        for path in members {
            if next[path] != domains[path] {
                domains[path] = next[path];
                changed = true;
            }
        }
        if !changed { break; }
    }
    domains
}

fn assigned_connector_domain(domains, path) {
    domains[path]
}

fn empty_connector_group(type_name) {
    let group = #{};
    group.type = type_name;
    group.members = [];
    group.connections = [];
    group.acausal_connections = [];
    group.signal_connections = [];
    group
}

fn connector_groups(members, domains) {
    let groups = [];
    let group_index = #{};
    for path in members {
        let type_name = assigned_connector_domain(domains, path);
        if !group_index.contains(type_name) {
            group_index[type_name] = groups.len;
            groups.push(empty_connector_group(type_name));
        }
        groups[group_index[type_name]].members.push(path);
    }
    #{ groups: groups, index: group_index }
}

fn add_same_domain_connection(groups, group_index, domains, left_path, right_path, edge, causal) {
    let left_domain = assigned_connector_domain(domains, left_path);
    let right_domain = assigned_connector_domain(domains, right_path);
    if left_domain != right_domain { return groups; }
    let group = groups[group_index[left_domain]];
    group.connections.push(edge);
    if causal {
        group.signal_connections.push(edge);
    } else {
        group.acausal_connections.push(edge);
    }
    groups[group_index[left_domain]] = group;
    groups
}

fn add_domain_connections(net, membership, domains, groups, group_index) {
    let updated = groups;
    for edge in net.connections {
        if !membership.contains(edge.left_path) { continue; }
        if !membership.contains(edge.right_path) { continue; }
        updated = add_same_domain_connection(updated, group_index, domains,
            edge.left_path, edge.right_path, edge, false);
    }
    if !net.contains("causal_links") { return updated; }
    for link in net.causal_links {
        if !membership.contains(link.source_path) { continue; }
        if !membership.contains(link.target_path) { continue; }
        let edge = #{ left_path: link.source_path, right_path: link.target_path };
        updated = add_same_domain_connection(updated, group_index, domains,
            link.source_path, link.target_path, edge, true);
    }
    updated
}

fn member_domain_groups(net, members, components) {
    let membership = member_index(members);
    let counts = connection_domain_counts(net, members, components, membership);
    let direct_domains = connector_domains(members, counts, components);
    let domains = direct_domains;
    domains = propagate_causal_domains(net, members, membership, domains);
    let grouped = connector_groups(members, domains);
    grouped.groups = add_domain_connections(net, membership, domains, grouped.groups, grouped.index);
    grouped.groups
}

fn group_has_signal_links(net, members) {
    if !net.contains("causal_links") { return false; }
    let membership = member_index(members);
    for link in net.causal_links {
        if membership.contains(link.source_path) || membership.contains(link.target_path) {
            return true;
        }
    }
    false
}

fn placement_x_bounds(placements) {
    let min_x = 0;
    let max_x = 0;
    let first = true;
    for item in placements {
        if first {
            min_x = item.x - COMPONENT_HALF_WIDTH;
            max_x = item.x + COMPONENT_HALF_WIDTH;
            first = false;
        } else {
            min_x = min(min_x, item.x - COMPONENT_HALF_WIDTH);
            max_x = max(max_x, item.x + COMPONENT_HALF_WIDTH);
        }
    }
    #{ min: min_x, max: max_x }
}

fn connector_group_layout(group, components) {
    let local_net = #{};
    local_net.connections = group.acausal_connections;
    group_member_layout(local_net, group.members, components)
}

fn causal_out_degree_index(net, members, membership) {
    let degrees = #{};
    let seen = #{};
    for path in members { degrees[path] = 0; }
    if !net.contains("causal_links") { return degrees; }
    for link in net.causal_links {
        if !membership.contains(link.source_path) || !membership.contains(link.target_path) {
            continue;
        }
        let pair = link.source_path + " -> " + link.target_path;
        if seen.contains(pair) { continue; }
        seen[pair] = true;
        degrees[link.source_path] += 1;
    }
    degrees
}

fn causal_hub_threshold(member_count) {
    max(SHARED_SIGNAL_MIN_TARGETS, (member_count + 3) / 4)
}

fn causal_hub_index(net, members, membership) {
    let degrees = causal_out_degree_index(net, members, membership);
    let hubs = #{};
    let threshold = causal_hub_threshold(members.len);
    for path in members {
        if degrees[path] >= threshold { hubs[path] = true; }
    }
    #{ degrees: degrees, hubs: hubs, threshold: threshold }
}

fn causal_component_labels(net, members, membership, hubs) {
    let labels = #{};
    for path in members { labels[path] = path; }
    if !net.contains("causal_links") { return labels; }
    for iteration in 0..members.len {
        let next = #{};
        for path in members { next[path] = labels[path]; }
        for link in net.causal_links {
            if !membership.contains(link.source_path) || !membership.contains(link.target_path) {
                continue;
            }
            if hubs.contains(link.source_path) || hubs.contains(link.target_path) { continue; }
            let source_label = labels[link.source_path];
            let target_label = labels[link.target_path];
            if source_label == target_label { continue; }
            let earliest = if stable_path_precedes(source_label, target_label) {
                source_label
            } else {
                target_label
            };
            next[link.source_path] = earliest;
            next[link.target_path] = earliest;
        }
        let changed = false;
        for path in members {
            if next[path] != labels[path] { changed = true; }
        }
        labels = next;
        if !changed { break; }
    }
    labels
}

fn causal_layout_groups(net, members) {
    let membership = member_index(members);
    let hub_facts = causal_hub_index(net, members, membership);
    let labels = causal_component_labels(net, members, membership, hub_facts.hubs);
    let clusters = [];
    let cluster_indices = #{};
    let hubs = [];
    for path in members {
        if hub_facts.hubs.contains(path) {
            hubs.push(path);
            continue;
        }
        let label = labels[path];
        if !cluster_indices.contains(label) {
            cluster_indices[label] = clusters.len;
            clusters.push([]);
        }
        clusters[cluster_indices[label]].push(path);
    }
    let branches = [];
    let satellites = [];
    for cluster in clusters {
        if cluster.len > 1 { branches.push(cluster); }
        else { satellites.push(cluster[0]); }
    }
    #{ branches: branches, hubs: hubs, satellites: satellites }
}

fn branch_cluster_placements(branch, row, spacing, column_offset) {
    let placements = [];
    for column in 0..branch.len {
        placements.push(#{
            path: branch[column],
            x: BRANCH_FIRST_X + column_offset + column * spacing,
            y: BRANCH_RIGHT_FIRST_ROW_Y + row * BRANCH_ROW_SPACING
        });
    }
    placements
}

fn satellite_placement(path, index, columns, first_row, spacing) {
    #{
        path: path,
        x: BRANCH_FIRST_X + (index % columns) * spacing,
        y: BRANCH_RIGHT_FIRST_ROW_Y + first_row * BRANCH_ROW_SPACING
            + (index / columns) * BRANCH_ROW_SPACING
    }
}

fn clustered_signal_layout(net, members) {
    let groups = causal_layout_groups(net, members);
    let branches = groups.branches;
    let hubs = groups.hubs;
    let satellites = groups.satellites;
    let fits_branch_rows = true;
    for branch in branches {
        if branch.len > 4 { fits_branch_rows = false; }
    }
    if branches.len < 3 || hubs.len == 0 || !fits_branch_rows {
        return #{ available: false, placements: [] };
    }

    let placements = [];
    let spacing = max(layout_branch_spacing(members), 260);
    let max_branch_size = 1;
    for branch in branches {
        max_branch_size = max(max_branch_size, branch.len);
    }
    let branch_pitch = max_branch_size * spacing + 80;
    for index in 0..branches.len {
        let row = index / SIGNAL_BRANCHES_PER_ROW;
        let branch_column = index % SIGNAL_BRANCHES_PER_ROW;
        let column_offset = branch_column * branch_pitch;
        placements = append_graphics(placements, branch_cluster_placements(
            branches[index], row, spacing, column_offset
        ));
    }
    for index in 0..hubs.len {
        placements.push(#{ path: hubs[index], x: CAUSAL_HUB_X, y: BUS_Y + index * CAUSAL_HUB_ROW_SPACING });
    }
    let satellite_columns = branch_column_count(satellites.len);
    let first_satellite_row = (branches.len + SIGNAL_BRANCHES_PER_ROW - 1)
        / SIGNAL_BRANCHES_PER_ROW;
    for index in 0..satellites.len {
        placements.push(satellite_placement(
            satellites[index], index, satellite_columns, first_satellite_row, spacing
        ));
    }
    #{ available: true, placements: placements }
}

fn generic_group_layout(net, group) {
    let clustered = clustered_signal_layout(net, group.members);
    if clustered.available { return clustered.placements; }
    let islands = physical_islands(group.members, group.acausal_connections);
    if physical_island_rows(islands) {
        return physical_island_placements(islands);
    }
    let local_net = #{};
    local_net.connections = group.connections;
    generic_member_layout(local_net, group.members)
}

fn physical_island_placements(islands) {
    let placements = [];
    for row in 0..islands.len {
        let row_placements = island_row_placements(islands[row], row);
        placements = append_graphics(placements, row_placements);
    }
    placements
}

fn island_row_placements(island, row) {
    let placements = [];
    for column in 0..island.len {
        placements.push(#{path: island[column], x: column * 320, y: row * 260});
    }
    placements
}

fn physical_islands(members, connections) {
    if connections.is_empty() { return []; }
    let links = [];
    for edge in connections {
        links.push(#{source_path: edge.left_path, target_path: edge.right_path});
    }
    let graph = #{causal_links: links};
    let labels = causal_component_labels(graph, members, member_index(members), #{});
    let groups = [];
    let indices = #{};
    for path in members {
        let label = labels[path];
        if !indices.contains(label) {
            indices[label] = groups.len;
            groups.push([]);
        }
        groups[indices[label]].push(path);
    }
    groups
}

fn physical_island_rows(islands) {
    if islands.len < 2 { return false; }
    for island in islands {
        if island.len < 2 || island.len > 6 { return false; }
    }
    true
}

fn common_member_prefix(members) {
    let prefix = path_leaf(members[0]);
    for path in members {
        let name = path_leaf(path);
        let count = 0;
        while count < prefix.len && count < name.len {
            if prefix[count] != name[count] { break; }
            count += 1;
        }
        prefix = prefix.sub_string(0, count);
    }
    if prefix.len < 4 { return ""; }
    prefix
}

fn member_display_labels(net, members) {
    let labels = #{};
    let counts = #{};
    for path in members.keys() {
        let name=path_leaf(path);
        if !counts.contains(name) {counts[name]=0;}
        counts[name]+=1;
    }
    for path in members.keys() {
        let name=path_leaf(path);
        if counts[name]>1 {
            let parts=path.split("/");
            name=parts[parts.len-2]+" / "+name;
        }
        labels[path]=name;
    }
    for island in physical_islands(members.keys(), net.connections) {
        if island.len < 2 { continue; }
        let prefix = common_member_prefix(island);
        if prefix == "" { continue; }
        for path in island {
            let name = path_leaf(path);
            let suffix = name.sub_string(prefix.len);
            if suffix != "" { labels[path] = suffix; }
        }
    }
    labels
}

fn shifted_placement(item, shift) {
    let result = #{};
    result.path = item.path;
    result.x = item.x + shift;
    result.y = item.y;
    result
}

fn domain_member_layout(net, members, components) {
    let groups = member_domain_groups(net, members, components);
    if groups.len == 0 { throw "generated Modelica unit has no members"; }

    let placements = [];
    let previous_right = 0;
    let first = true;
    for group in groups {
        let local = if group.type == "LunCo.Electrical.Pin" {
            connector_group_layout(group, components)
        } else {
            generic_group_layout(net, group)
        };
        let bounds = placement_x_bounds(local);
        let shift = if first { 0 } else { previous_right + DOMAIN_GROUP_GAP - bounds.min };
        for item in local {
            placements.push(shifted_placement(item, shift));
        }
        previous_right = bounds.max + shift;
        first = false;
    }
    placements
}

fn is_electrical_pin_network(net, members, components) {
    let membership = member_index(members);
    let found = false;
    for edge in net.connections {
        if !membership.contains(edge.left_path) || !membership.contains(edge.right_path) {
            continue;
        }
        found = true;
        if connection_type(edge, components) != "LunCo.Electrical.Pin" { return false; }
    }
    found
}

fn domain_outline_color(type_name) {
    if type_name == "LunCo.Electrical.Pin" { return "{225,185,85}"; }
    if type_name == "LunCo.Thermal.HeatPort" { return "{230,125,105}"; }
    if type_name == "LunCo.Propulsion.FluidPort" { return "{100,185,220}"; }
    "{165,150,215}"
}

fn subsystem_graphics(net, members, components, positions) {
    let graphics = [];
    for group in member_domain_groups(net, members, components) {
        let min_x = 0;
        let max_x = 0;
        let min_y = 0;
        let max_y = 0;
        let first = true;
        for path in group.members {
            let p = position(positions, path);
            if first {
                min_x = p.x - COMPONENT_HALF_WIDTH;
                max_x = p.x + COMPONENT_HALF_WIDTH;
                min_y = p.y - COMPONENT_HALF_HEIGHT;
                max_y = p.y + COMPONENT_HALF_HEIGHT;
                first = false;
            } else {
                min_x = min(min_x, p.x - COMPONENT_HALF_WIDTH);
                max_x = max(max_x, p.x + COMPONENT_HALF_WIDTH);
                min_y = min(min_y, p.y - COMPONENT_HALF_HEIGHT);
                max_y = max(max_y, p.y + COMPONENT_HALF_HEIGHT);
            }
        }
        let label_y = min_y - 95;
        let label_center_x = (min_x + max_x) / 2;
        let label_half_width = max((max_x - min_x) / 2, 120);
        let label_text = connector_domain_label(group.type);
        let outline = "Rectangle(extent=" + rect(min_x - 100, min_y - 120, max_x + 100, max_y + 110);
        outline += ", lineColor=" + domain_outline_color(group.type);
        outline += ", fillPattern=FillPattern.None, radius=16)";
        graphics.push(outline);
        if group.type != "" && group_has_signal_links(net, group.members) {
            label_text += " + Signals and I/O";
        }
        let label = "Text(extent=" + rect(
                label_center_x - label_half_width,
                label_y,
                label_center_x + label_half_width,
                label_y + 35
            )
            + ", textString=" + string_literal(label_text)
            + ", textColor=" + domain_outline_color(group.type)
            + ", fontSize=30)";
        graphics.push(label);
    }
    graphics
}

fn unit_summary_label(net, members) {
    let physical = 0;
    for edge in net.connections {
        if has_member(members, edge.left_path) && has_member(members, edge.right_path) {
            physical += 1;
        }
    }
    let signals = 0;
    for link in net.causal_links {
        if has_member(members, link.source_path) && has_member(members, link.target_path) {
            signals += 1;
        }
    }
    let boundary = 0;
    for link in net.boundary_links {
        if has_member(members, link.target_path) { boundary += 1; }
    }
    let label = path_leaf(net.root) + " | " + members.keys().len.to_string() + " components";
    label += " | " + physical.to_string() + " physical connections";
    label += " | " + signals.to_string() + " signal links";
    label += " | " + boundary.to_string() + " external inputs";
    label
}

fn member_name_graphics(net, members, positions, components) {
    let graphics = [];
    let labels = member_display_labels(net, members);
    for path in members.keys() {
        let p = position(positions, path);
        let label = labels[path];
        let text = "Text(extent=";
        text += rect(p.x - 130, p.y + COMPONENT_HALF_HEIGHT + 7,
            p.x + 130, p.y + COMPONENT_HALF_HEIGHT + 41);
        text += ", textString=" + string_literal(label);
        text += ", textColor=" + MEMBER_NAME_COLOR + ", fontSize=" + DIAGRAM_MEMBER_FONT_SIZE_TEXT + ")";
        graphics.push(text);
        let class_segments = component_for(components, path).class.split(".");
        let class_label = class_segments[class_segments.len - 1];
        let detail = "Text(extent=" + rect(p.x - 130, p.y - 70, p.x + 130, p.y - 42);
        detail += ", textString=" + string_literal(class_label);
        detail += ", textColor={165,185,210}, fontSize=20)";
        graphics.push(detail);
    }
    graphics
}

fn branch_frame_graphics(branch, positions, title) {
    let first = position(positions, branch[0]);
    let left = first.x;
    let right = first.x;
    for path in branch {
        let p = position(positions, path);
        left = min(left, p.x);
        right = max(right, p.x);
    }
    let frame = "Rectangle(extent=" + rect(left - 140, first.y - 88, right + 140, first.y + 88);
    frame += ", lineColor={65,95,135}, fillColor={24,30,43}, fillPattern=FillPattern.Solid, radius=12)";
    let caption = "Text(extent=" + rect(left - 100, first.y + 96, right + 100, first.y + 130);
    caption += ", textString=" + string_literal(title);
    caption += ", textColor={145,190,235}, fontSize=24)";
    [frame, caption]
}

fn causal_branch_graphics(net, members, positions) {
    let layout = clustered_signal_layout(net, members.keys());
    if !layout.available { return []; }
    let groups = causal_layout_groups(net, members.keys());
    let graphics = [];
    for branch in groups.branches {
        let title = path_leaf(branch[0]) + " · " + branch.len.to_string() + " components";
        graphics = append_graphics(graphics, branch_frame_graphics(branch, positions, title));
    }
    graphics
}

fn physical_island_graphics(net, members, components, positions) {
    let graphics = [];
    for group in member_domain_groups(net, members.keys(), components) {
        if group.type == "LunCo.Electrical.Pin" { continue; }
        let clustered = clustered_signal_layout(net, group.members);
        if clustered.available { continue; }
        let islands = physical_islands(group.members, group.acausal_connections);
        if !physical_island_rows(islands) { continue; }
        let frames = island_frames(islands, positions);
        graphics = append_graphics(graphics, frames);
    }
    graphics
}

fn island_frames(islands, positions) {
    let graphics = [];
    for island in islands {
        let title = island_title(island);
        let frame = branch_frame_graphics(island, positions, title);
        graphics = append_graphics(graphics, frame);
    }
    graphics
}

fn island_title(island) {
    let title = common_member_prefix(island);
    if title == "" { title = path_leaf(island[0]); }
    title + " · " + island.len.to_string() + " components"
}

fn member_summary_graphics(net, members, size) {
    let text = "Text(extent=";
    text += rect(size.left + 24, size.top - 56, size.right - 24, size.top - 16);
    text += ", textString=" + string_literal(unit_summary_label(net, members));
    text += ", textColor=" + MEMBER_NAME_COLOR + ", fontSize=" + DIAGRAM_SUMMARY_FONT_SIZE_TEXT + ")";
    [text]
}

fn generated_unit_position(index, count) {
    // Keep the generated-layout hook portable across the embedded Rhai
    // runtimes.  `ceil()` is not part of the minimal scripting surface used
    // by the headless Modelica synthesizer, so compute the integer ceiling of
    // sqrt(count) explicitly.  This is presentation-only and remains
    // deterministic for every connected-component count.
    let columns = 1;
    while columns * columns < count {
        columns += 1;
    }
    if columns < 1 { columns = 1; }
    let rows = ((count + columns - 1) / columns).max(1);
    let origin_x = -((columns - 1) * GENERATED_UNIT_COLUMN_SPACING) / 2;
    let origin_y = ((rows - 1) * GENERATED_UNIT_ROW_SPACING) / 2;
    #{
        x: origin_x + (index % columns) * GENERATED_UNIT_COLUMN_SPACING,
        y: origin_y - (index / columns) * GENERATED_UNIT_ROW_SPACING
    }
}

fn has_member(index, path) {
    index.contains(path)
}

fn component_for(index, path) {
    if index.contains(path) { return index[path]; }
    throw "generated Modelica facts are missing component: " + path
}

fn path_leaf(path) {
    let leaf = path;
    for segment in path.split("/") {
        if segment != "" { leaf = segment; }
    }
    leaf
}

fn connection_degree_index(net, members, membership) {
    let degrees = #{};
    for path in members { degrees[path] = 0; }
    for edge in net.connections {
        if !membership.contains(edge.left_path) || !membership.contains(edge.right_path) {
            continue;
        }
        degrees[edge.left_path] = degrees[edge.left_path] + 1;
        degrees[edge.right_path] = degrees[edge.right_path] + 1;
    }
    degrees
}

// Acausal flow has no compile-time source or sink: Pin.i can reverse sign at
// runtime. The topology role is therefore an explicit USD presentation fact,
// not an inference from a connector name or a Modelica class string. It only
// chooses a readable side of the visual bus; runtime Pin.i sign controls
// animated direction, never the diagram's geometry.
fn topology_role(components, path) {
    let component = component_for(components, path);
    if component.contains("topology_role") {
        component.topology_role
    } else {
        "neutral"
    }
}

fn stable_path_precedes(path, best) {
    if best == "" { return true; }
    path < best
}

fn should_replace_hub(path, degree, role_priority, best, best_degree, best_role_priority) {
    if degree != best_degree {
        return degree > best_degree;
    }
    if role_priority != best_role_priority {
        return role_priority > best_role_priority;
    }
    stable_path_precedes(path, best)
}

fn topology_hub(members, degrees, roles) {
    let best = "";
    let best_degree = -1;
    let best_role_priority = -1;
    for path in members {
        let degree = degrees[path];
        let role_priority = if roles[path] == "storage" { 1 } else { 0 };
        let replace = should_replace_hub(
            path, degree, role_priority, best, best_degree, best_role_priority
        );
        if replace {
            best = path;
            best_degree = degree;
            best_role_priority = role_priority;
        }
    }
    if best == "" {
        throw "generated acausal unit has no topology members"
    }
    best
}

fn network_hub(members, degrees) {
    let best = "";
    let best_degree = -1;
    for path in members {
        let degree = degrees[path];
        if degree > best_degree || (degree == best_degree && stable_path_precedes(path, best)) {
            best = path;
            best_degree = degree;
        }
    }
    if best == "" { throw "generated connector group has no members"; }
    best
}

// Use a deterministic near-square bank. This keeps repeated components of
// the same type readable without giving the diagram a height proportional to
// the number of motors, thrusters, or other common-pin members.
fn branch_column_count(count) {
    if count <= 1 { 1 }
    else if count <= 4 { 2 }
    else if count <= 9 { 3 }
    else if count <= 16 { 4 }
    else { 5 }
}

fn layout_branch_spacing(members) {
    let spacing = BRANCH_COLUMN_SPACING;
    for path in members {
        let preferred = path_leaf(path).len * 10 + 120;
        if preferred > spacing { spacing = preferred; }
    }
    spacing
}

fn branch_placement(path, side, index, count, spacing) {
    let bank_spacing = max(spacing, 520);
    let columns = branch_column_count(count);
    let column = index % columns;
    let row = index / columns;
    let x = if side == "left" {
        BRANCH_LEFT_X - column * bank_spacing
    } else {
        BRANCH_FIRST_X + column * bank_spacing
    };
    let y = if side == "left" {
        BRANCH_LEFT_FIRST_ROW_Y - row * BRANCH_ROW_SPACING
    } else {
        BRANCH_RIGHT_FIRST_ROW_Y + row * BRANCH_ROW_SPACING
    };
    #{
        path: path,
        x: x,
        y: y
    }
}

fn layout_groups(net, units) {
    let groups = [];
    if units.len == 0 {
        let all = [];
        for component in net.components { all.push(component.path); }
        groups.push(all);
    } else {
        for unit in units { groups.push(unit.components); }
    }
    groups
}

fn classify_members(roles, members, hub) {
    let result = #{ sources: [], loads: [], neutral: [] };
    for path in members {
        if path == hub { continue; }
        let role = roles[path];
        if role == "source" {
            result.sources.push(path);
        } else if role == "load" {
            result.loads.push(path);
        } else {
            result.neutral.push(path);
        }
    }
    result
}

fn group_member_layout(net, members, components) {
    let placements = [];
    let branch_spacing = layout_branch_spacing(members);
    let membership = member_index(members);
    let degrees = connection_degree_index(net, members, membership);
    let roles = #{};
    for path in members { roles[path] = topology_role(components, path); }
    let hub = topology_hub(members, degrees, roles);
    placements.push(#{ path: hub, x: HUB_X, y: BUS_Y });
    let classified = classify_members(roles, members, hub);
    // Explicit source/load roles form opposite banks of the bus. This is a
    // semantic layout policy, not an electrical direction assumption: the
    // generated equations remain the authored acausal connect graph.
    // Keep neutral members on one selected bank. Alternating them between
    // banks would make a repeated component grid grow in two directions and
    // make its total height proportional to the number of members.
    let left_count = 0;
    let right_count = 0;
    let neutral_on_left = classified.sources.len <= classified.loads.len;
    let left_total = classified.sources.len;
    let right_total = classified.loads.len;
    if neutral_on_left {
        left_total += classified.neutral.len;
    } else {
        right_total += classified.neutral.len;
    }
    for path in classified.sources {
        placements.push(branch_placement(path, "left", left_count, left_total, branch_spacing));
        left_count += 1;
    }
    for path in classified.loads {
        placements.push(branch_placement(path, "right", right_count, right_total, branch_spacing));
        right_count += 1;
    }
    // Components without a side preference remain fully supported. Pack the
    // complete neutral bank onto the shorter explicit bank, with a
    // deterministic left tie, so repeated components form one compact grid.
    for path in classified.neutral {
        if neutral_on_left {
            placements.push(branch_placement(path, "left", left_count, left_total, branch_spacing));
            left_count += 1;
        } else {
            placements.push(branch_placement(path, "right", right_count, right_total, branch_spacing));
            right_count += 1;
        }
    }
    placements
}

fn generic_member_layout(net, members) {
    let placements = [];
    let branch_spacing = layout_branch_spacing(members);
    let membership = member_index(members);
    let degrees = connection_degree_index(net, members, membership);
    let hub = network_hub(members, degrees);
    placements.push(#{ path: hub, x: HUB_X, y: BUS_Y });
    let branch_index = 0;
    let branch_count = members.len - 1;
    for path in members {
        if path == hub { continue; }
        placements.push(branch_placement(path, "right", branch_index, branch_count, branch_spacing));
        branch_index += 1;
    }
    placements
}

fn visual_layout(net, units) {
    visual_layout_with_components(net, units, component_index(net))
}

fn visual_layout_with_components(net, units, components) {
    let layout = #{ units: [], members: [] };
    let index = 0;
    for unit in units {
        // `name` is the readable Modelica class name presented to the user.
        let item = generated_unit_position(index, units.len);
        layout.units.push(#{
            name: unit.name,
            x: item.x * UNIT_POSITION_SCALE,
            y: item.y * UNIT_POSITION_SCALE
        });
        index += 1;
    }
    // Unit positions belong to the generated root diagram. Member positions
    // belong to each unit's own diagram, so every group gets the same local
    // coordinate system; root-level spacing never leaks into a child class.
    for members in layout_groups(net, units) {
        for placement in domain_member_layout(net, members, components) {
            layout.members.push(placement);
        }
    }
    layout
}

fn member_extent(positions, members, net) {
    let first = position(positions, members[0]);
    let left = first.x - 160;
    let right = first.x + 160;
    let bottom = first.y - 180;
    let top = first.y + MEMBER_EXTENT_MARGIN_Y;
    for path in members {
        let p = position(positions, path);
        left = min(left, p.x - 160);
        right = max(right, p.x + 160);
        bottom = min(bottom, p.y - 180);
        top = max(top, p.y + MEMBER_EXTENT_MARGIN_Y);
    }
    for link in net.boundary_links {
        if !has_member(members, link.target_path) { continue; }
        let target = position(positions, link.target_path);
        let label_left = target.x - COMPONENT_HALF_WIDTH - BOUNDARY_STUB_LENGTH - 276;
        left = min(left, label_left - 24);
    }
    #{left: left, right: right, bottom: bottom, top: top}
}

fn unit_extent(positions, units) {
    let half_width = UNIT_MIN_HALF_WIDTH;
    let half_height = UNIT_MIN_HALF_HEIGHT;
    for unit in units {
        let p = unit_position(positions, unit.name);
        if p.x + UNIT_EXTENT_MARGIN_X > half_width {
            half_width = p.x + UNIT_EXTENT_MARGIN_X;
        }
        if -p.x + UNIT_EXTENT_MARGIN_X > half_width {
            half_width = -p.x + UNIT_EXTENT_MARGIN_X;
        }
        if p.y + UNIT_EXTENT_MARGIN_Y > half_height {
            half_height = p.y + UNIT_EXTENT_MARGIN_Y;
        }
        if -p.y + UNIT_EXTENT_MARGIN_Y > half_height {
            half_height = -p.y + UNIT_EXTENT_MARGIN_Y;
        }
    }
    let result = #{};
    result.width = half_width;
    result.height = half_height;
    result
}

fn compact_icon(line_color, fill_color) {
    let rectangle = "Rectangle(extent=" + rect(
            -ICON_BOX_HALF_WIDTH,
            -ICON_BOX_HALF_HEIGHT,
            ICON_BOX_HALF_WIDTH,
            ICON_BOX_HALF_HEIGHT
        )
        + ", lineColor=" + line_color
        + ", fillColor=" + fill_color
        + ", fillPattern=FillPattern.Solid, radius=" + ICON_CORNER_RADIUS_TEXT + ")";
    let bus = "Line(points={{-52,0},{52,0}}, color={245,250,255}, thickness=2)";
    let left_node = "Ellipse(extent={{-68,-12},{-44,12}}, lineColor="
        + line_color + ", fillColor=" + ICON_TEXT_COLOR + ", fillPattern=FillPattern.Solid)";
    let right_node = "Ellipse(extent={{44,-12},{68,12}}, lineColor="
        + line_color + ", fillColor=" + ICON_TEXT_COLOR + ", fillPattern=FillPattern.Solid)";
    "Icon(coordinateSystem(extent=" + rect(
            -ICON_COORDINATE_HALF_WIDTH,
            -ICON_COORDINATE_HALF_HEIGHT,
            ICON_COORDINATE_HALF_WIDTH,
            ICON_COORDINATE_HALF_HEIGHT
        )
        + "), graphics={" + rectangle + "," + bus + "," + left_node + "," + right_node + "})"
}

// Causal signals remain scalar Modelica equations. These standard Diagram
// graphics label and route those equations between the generated member boxes.
fn signal_endpoint(instance, variable) {
    let result = instance;
    result += ".";
    result += variable;
    result
}

fn signal_label(source, target) {
    port_caption(source)
}

fn port_caption(name) {
    let segments = name.split(".");
    let caption = segments[segments.len - 1];
    caption.replace("_", " ")
}

fn causal_wire_graphics(x1, y1, x2, y2, label, index, show_label) {
    let lane_offset = ((index % 5) - 2) * 18;
    let lane_x = (x1 + x2) / 2 + lane_offset;
    let line = "Line(points={{";
    line += x1.to_string() + "," + y1.to_string();
    line += "},{" + lane_x.to_string() + "," + y1.to_string();
    line += "},{" + lane_x.to_string() + "," + y2.to_string();
    line += "},{" + x2.to_string() + "," + y2.to_string();
    line += "}}, color=" + CAUSAL_WIRE_COLOR
        + ", thickness=1.5, arrow={Arrow.None,Arrow.Filled})";
    let text_y = if y1 == y2 { y1 + COMPONENT_HALF_HEIGHT + 8 } else { (y1 + y2) / 2 + 6 };
    let text = "Text(extent=";
    text += rect(lane_x - 40, text_y, lane_x + 40, text_y + 20);
    text += ", textString=" + string_literal(label);
    text += ", textColor=" + CAUSAL_WIRE_TEXT_COLOR + ", fontSize=12)";
    if show_label { return [line, text]; }
    [line]
}

fn member_causal_wire_graphics(x1, y1, x2, y2, index) {
    let line = "Line(points={{";
    line += x1.to_string() + "," + y1.to_string();
    if y1 == y2 {
        line += "},{" + x2.to_string() + "," + y2.to_string();
    } else {
        let lane_x = (x1 + x2) / 2 + ((index % 5) - 2) * 12;
        line += "},{" + lane_x.to_string() + "," + y1.to_string();
        line += "},{" + lane_x.to_string() + "," + y2.to_string();
        line += "},{" + x2.to_string() + "," + y2.to_string();
    }
    line += "}}, color=" + CAUSAL_WIRE_COLOR
        + ", thickness=1.5, arrow={Arrow.None,Arrow.Filled})";
    [line]
}

fn shared_signal_bus_graphics(net, members, positions, source_path, lane_index) {
    let targets = [];
    for link in net.causal_links {
        if link.source_path == source_path
            && has_member(members, link.target_path)
            && !targets.contains(link.target_path) {
            targets.push(link.target_path);
        }
    }
    if targets.is_empty() { return []; }

    let source = position(positions, source_path);
    let target_center_x = 0;
    for path in targets {
        target_center_x += position(positions, path).x;
    }
    let direction = if target_center_x / targets.len < source.x { -1 } else { 1 };
    let source_edge = source.x + direction * COMPONENT_HALF_WIDTH;
    let trunk_x = source_edge + direction * (SHARED_SIGNAL_STUB + lane_index * 22);
    let min_y = position(positions, targets[0]).y;
    let max_y = min_y;
    for path in targets {
        let y = position(positions, path).y;
        min_y = min(min_y, y);
        max_y = max(max_y, y);
    }
    let graphics = [];
    let source_line = "Line(points={{";
    source_line += source_edge.to_string() + "," + source.y.to_string();
    source_line += "},{" + trunk_x.to_string() + "," + source.y.to_string();
    source_line += "}}, color=" + CAUSAL_WIRE_COLOR + ", thickness=2)";
    graphics.push(source_line);
    let trunk_line = "Line(points={{";
    trunk_line += trunk_x.to_string() + "," + min_y.to_string();
    trunk_line += "},{" + trunk_x.to_string() + "," + max_y.to_string();
    trunk_line += "}}, color=" + CAUSAL_WIRE_COLOR + ", thickness=2)";
    graphics.push(trunk_line);
    for index in 0..targets.len {
        let target = position(positions, targets[index]);
        let offset = ((index % 3) - 1) * 12;
        let tap_y = target.y + offset;
        let target_edge = target.x - direction * COMPONENT_HALF_WIDTH;
        let tap = member_causal_wire_graphics(trunk_x, tap_y, target_edge, tap_y, index);
        graphics = append_graphics(graphics, tap);
    }
    let bus_label = "shared signal bus: ";
    bus_label += targets.len.to_string() + " targets";
    let text = "Text(extent=";
    text += rect(trunk_x - 180, source.y - 48, trunk_x + 180, source.y - 12);
    text += ", textString=" + string_literal(bus_label);
    text += ", textColor=" + CAUSAL_WIRE_TEXT_COLOR + ", fontSize=" + DIAGRAM_DETAIL_FONT_SIZE_TEXT + ")";
    graphics.push(text);
    graphics
}

fn append_graphics(target, additions) {
    let result = target;
    for graphic in additions { result.push(graphic); }
    result
}

fn graphics_source(graphics) {
    let result = "";
    for index in 0..graphics.len {
        if index > 0 { result += ","; }
        result += graphics[index];
    }
    result
}

fn member_signal_graphics(net, members, positions) {
    let graphics = [];
    let handled = #{};
    let member_paths = members.keys();
    let hub_facts = causal_hub_index(net, member_paths, members);
    let hubs = hub_facts.hubs;
    let bus_index = 0;
    for path in member_paths {
        if hubs.contains(path) {
            let bus = shared_signal_bus_graphics(net, members, positions, path, bus_index);
            graphics = append_graphics(graphics, bus);
            bus_index += 1;
        }
    }
    let index = 0;
    for link in net.causal_links {
        if !has_member(members, link.source_path) || !has_member(members, link.target_path) {
            continue;
        }
        if hubs.contains(link.source_path) { continue; }
        let key = link.source_path + " -> " + link.target_path;
        if handled.contains(key) { continue; }
        handled[key] = true;
        let source = position(positions, link.source_path);
        let target = position(positions, link.target_path);
        let source_left = source.x <= target.x;
        let source_offset = if source_left { COMPONENT_HALF_WIDTH } else { -COMPONENT_HALF_WIDTH };
        let target_offset = if source_left { -COMPONENT_HALF_WIDTH } else { COMPONENT_HALF_WIDTH };
        let x1 = source.x + source_offset;
        let x2 = target.x + target_offset;
        let wire = member_causal_wire_graphics(x1, source.y, x2, target.y, index);
        graphics = append_graphics(graphics, wire);
        index += 1;
    }
    graphics
}

fn shared_boundary_inputs(net, members) {
    let candidates = [];
    for link in net.boundary_links {
        if !has_member(members, link.target_path) { continue; }
        if !candidates.contains(link.input) { candidates.push(link.input); }
    }
    let shared_names = [];
    for input in candidates {
        let targets = boundary_bus_targets(net, members, input);
        if targets.len >= SHARED_BOUNDARY_MIN_TARGETS { shared_names.push(input); }
    }
    shared_names
}

fn boundary_bus_targets(net, members, input) {
    let targets = [];
    for link in net.boundary_links {
        if link.input != input { continue; }
        if !has_member(members, link.target_path) { continue; }
        if !targets.contains(link.target_path) { targets.push(link.target_path); }
    }
    targets
}

fn boundary_input_bus_graphics(net, members, positions, input, lane_index) {
    let targets = boundary_bus_targets(net, members, input);
    if targets.is_empty() { return []; }
    let min_x = position(positions, targets[0]).x;
    let min_y = position(positions, targets[0]).y;
    let max_y = min_y;
    for path in targets {
        let target = position(positions, path);
        min_x = min(min_x, target.x);
        min_y = min(min_y, target.y);
        max_y = max(max_y, target.y);
    }
    let trunk_x = min_x - COMPONENT_HALF_WIDTH - SHARED_SIGNAL_STUB - lane_index * 22;
    let graphics = [];
    let trunk = "Line(points={{";
    trunk += trunk_x.to_string() + "," + min_y.to_string();
    trunk += "},{" + trunk_x.to_string() + "," + max_y.to_string();
    trunk += "}}, color=" + CAUSAL_WIRE_COLOR + ", thickness=2)";
    graphics.push(trunk);
    for index in 0..targets.len {
        let target = position(positions, targets[index]);
        let tap_y = target.y + ((index % 3) - 1) * 12;
        let target_edge = target.x - COMPONENT_HALF_WIDTH;
        let tap = member_causal_wire_graphics(trunk_x, tap_y, target_edge, tap_y, index);
        graphics = append_graphics(graphics, tap);
    }
    let label_text = port_caption(input) + " · external input";
    let label = "Text(extent=";
    label += rect(trunk_x - 460, min_y - 48, trunk_x - 20, min_y - 12);
    label += ", textString=" + string_literal(label_text);
    label += ", textColor=" + CAUSAL_WIRE_TEXT_COLOR + ", fontSize=" + DIAGRAM_DETAIL_FONT_SIZE_TEXT + ")";
    graphics.push(label);
    graphics
}

fn boundary_signal_graphics(net, members, positions) {
    let graphics = [];
    let handled = #{};
    let shared_inputs = shared_boundary_inputs(net, members);
    let shared_input_index = #{};
    for index in 0..shared_inputs.len {
        shared_input_index[shared_inputs[index]] = true;
        let bus = boundary_input_bus_graphics(
            net, members, positions, shared_inputs[index], index
        );
        graphics = append_graphics(graphics, bus);
    }
    for link in net.boundary_links {
        if !has_member(members, link.target_path) { continue; }
        if shared_input_index.contains(link.input) { continue; }
        let key = link.target_path + " -> " + link.target_input;
        if handled.contains(key) { continue; }
        handled[key] = true;
        let bundle = [];
        let bundle_inputs = #{};
        for candidate in net.boundary_links {
            if candidate.target_path != link.target_path { continue; }
            if shared_input_index.contains(candidate.input) { continue; }
            let candidate_input = candidate.target_input;
            if bundle_inputs.contains(candidate_input) { continue; }
            bundle.push(candidate);
            bundle_inputs[candidate_input] = true;
        }
        let target = position(positions, link.target_path);
        let target_edge = target.x - COMPONENT_HALF_WIDTH;
        let first_offset = (bundle.len - 1) * 15;
        for index in 0..bundle.len {
            let input = bundle[index];
            let y = target.y - first_offset + index * 30;
            let start_x = target_edge - BOUNDARY_STUB_LENGTH;
            let line = "Line(points={{";
            line += start_x.to_string() + "," + y.to_string();
            line += "},{" + target_edge.to_string() + "," + y.to_string();
            line += "}}, color=" + CAUSAL_WIRE_COLOR;
            line += ", thickness=1.5, arrow={Arrow.None,Arrow.Filled})";
            graphics.push(line);
            let label = "Text(extent=";
            label += rect(start_x - 276, y - 14, start_x - 8, y + 14);
            label += ", textString=" + string_literal(port_caption(input.target_input));
            label += ", textColor=" + CAUSAL_WIRE_TEXT_COLOR + ", fontSize=" + DIAGRAM_DETAIL_FONT_SIZE_TEXT + ")";
            graphics.push(label);
        }
    }
    graphics
}

fn root_unit_signal_graphics(unit, unit_position, bounds, index) {
    let graphics = [];
    for input in unit.inputs {
        let y1 = unit_position.y + ((index % 3) - 1) * 16;
        let label = signal_label(input, signal_endpoint(unit.instance, input));
        graphics = append_graphics(graphics, causal_wire_graphics(
            -bounds.width, y1, unit_position.x - UNIT_HALF_WIDTH, unit_position.y,
            label, index, true
        ));
        index += 1;
    }
    for output in unit.outputs {
        let y2 = unit_position.y + ((index % 3) - 1) * 16;
        let label = signal_label(signal_endpoint(unit.instance, output), output);
        graphics = append_graphics(graphics, causal_wire_graphics(
            unit_position.x + UNIT_HALF_WIDTH, unit_position.y, bounds.width, y2,
            label, index, false
        ));
        index += 1;
    }
    #{ graphics: graphics, index: index }
}

fn root_signal_graphics(units, unit_positions) {
    let graphics = [];
    let bounds = unit_extent(unit_positions, units);
    let index = 0;
    for unit in units {
        let unit_position = unit_position(unit_positions, unit.name);
        let emitted = root_unit_signal_graphics(unit, unit_position, bounds, index);
        graphics = append_graphics(graphics, emitted.graphics);
        index = emitted.index;
    }
    graphics
}

fn connect_equation(edge) {
    let line = "  connect(";
    line += edge.left_instance;
    line += ".";
    line += edge.left_connector;
    line += ", ";
    line += edge.right_instance;
    line += ".";
    line += edge.right_connector;
    line += ")";
    line
}

fn branch_feeder_x(x, rail_x) {
    if x < rail_x {
        x + COMPONENT_HALF_WIDTH + 18
    } else {
        x - COMPONENT_HALF_WIDTH - 18
    }
}

fn append_point(line, x, y) {
    line += ",{"
        + x.to_string()
        + ","
        + y.to_string()
        + "}";
    line
}

// Keep common-pin electrical wiring legible. The authored topology remains
// the connect() equation; these are standard Modelica Line waypoints only.
// Every branch visits its own feeder lane and then the central bus, so a
// SolarPanel-to-motor equation cannot render as a direct wire or cut through
// another repeated component.
fn power_bus_annotation(edge, positions) {
    let left = position(positions, edge.left_path);
    let right = position(positions, edge.right_path);
    let rail_x = power_rail_x();
    // Include the horizontal icon stubs as well as the rail crossing. A
    // two-point vertical annotation leaves the renderer's default straight
    // wire visible, making a panel appear directly connected to a motor.
    let left_port_x = if left.x < rail_x {
        left.x + COMPONENT_HALF_WIDTH
    } else {
        left.x - COMPONENT_HALF_WIDTH
    };
    let right_port_x = if right.x < rail_x {
        right.x + COMPONENT_HALF_WIDTH
    } else {
        right.x - COMPONENT_HALF_WIDTH
    };
    let line = " annotation(Line(points={{";
    line += left_port_x.to_string();
    line += ",";
    line += left.y.to_string();
    line += "}";
    if left.x == HUB_X && left.y == BUS_Y {
        line = append_point(line, rail_x, left.y);
    } else {
        let feeder_x = branch_feeder_x(left.x, rail_x);
        line = append_point(line, feeder_x, left.y);
        line = append_point(line, feeder_x, BUS_Y);
        line = append_point(line, rail_x, BUS_Y);
    }
    if right.x == HUB_X && right.y == BUS_Y {
        line = append_point(line, rail_x, right.y);
    } else {
        let feeder_x = branch_feeder_x(right.x, rail_x);
        line = append_point(line, rail_x, BUS_Y);
        line = append_point(line, feeder_x, BUS_Y);
        line = append_point(line, feeder_x, right.y);
    }
    line = append_point(line, right_port_x, right.y);
    // Leave connection colour to the Modelica connector projection. The
    // generated route is only geometry; a single neutral policy colour would
    // hide the electrical/mechanical/thermal distinction carried by each
    // connector's authored Icon.
    line += "}, thickness=";
    line += power_line_thickness_text();
    line += "))";
    line
}

fn bus_bounds(positions, members) {
    let min_y = 0;
    let max_y = 0;
    let first = true;
    for path in members {
        let p = position(positions, path);
        if first {
            min_y = p.y;
            max_y = p.y;
            first = false;
        } else {
            if p.y < min_y { min_y = p.y; }
            if p.y > max_y { max_y = p.y; }
        }
    }
    let result = #{};
    result.min_y = min_y - BUS_RAIL_MARGIN;
    result.max_y = max_y + BUS_RAIL_MARGIN;
    result
}

fn unit_bus_graphics(positions, members) {
    let bounds = bus_bounds(positions, members);
    let rail = "Line(points={{";
    rail += power_rail_x().to_string();
    rail += ",";
    rail += bounds.min_y.to_string();
    rail += "},{";
    rail += power_rail_x().to_string();
    rail += ",";
    rail += bounds.max_y.to_string();
    rail += "}}, color=";
    rail += power_rail_color();
    rail += ", thickness=";
    rail += power_line_thickness_text();
    rail += ")";
    let label_y = bounds.max_y + 22;
    let label = "Text(extent=" + rect(
            power_rail_x() - 75,
            label_y,
            power_rail_x() + 75,
            label_y + 20
        )
        + ", textString=\"POWER BUS\", textColor=" + power_rail_color() + ", fontSize=" + DIAGRAM_BUS_FONT_SIZE_TEXT + ")";
    [rail, label]
}

fn policy_units(net) {
    let result = [];
    for unit in net.units {
        let instance = unit.name;
        if net.units.len == 1 { instance = "network_system"; }
        // Generated unit classes share Rumoca's source-library namespace, so
        // include the unique root model identity even for a single-unit graph.
        let class_name = net.model_name + "_" + unit.name;
        if net.units.len == 1 { class_name = net.model_name + "_NetworkSystem"; }
        let rewritten = #{
            name: class_name,
            instance: instance,
            components: unit.components,
            inputs: unit.inputs,
            outputs: unit.outputs
        };
        result.push(rewritten);
    }
    result
}

fn output_connection(output) {
    let line = "  ";
    line += output.name;
    line += " = ";
    line += output.source_instance;
    line += ".";
    line += output.source_output;
    line += ";\n";
    line
}

fn member_output_connection(output, instance) {
    let line = "  ";
    line += output.alias;
    line += " = ";
    line += instance;
    line += ".";
    line += output.alias;
    line += ";\n";
    line
}

fn component_binding(name, value) {
    let binding = #{};
    binding.name = name;
    binding.value = value;
    binding
}

fn component_declaration(component, positions, net, members) {
    let bindings = [];
    for modification in component.modifications {
        let value = modification.value.to_string();
        let binding = #{name: modification.name, value: value};
        bindings.push(binding);
    }
    for link in net.boundary_links {
        if link.target_path == component.path {
            bindings.push(component_binding(link.target_input, link.input));
        }
    }
    for link in net.causal_links {
        if link.target_path == component.path && has_member(members, link.source_path) {
            let value = signal_endpoint(link.source_instance, link.source_output);
            bindings.push(component_binding(link.target_input, value));
        }
    }
    let line = "  " + instance_declaration(component.class, component.instance, bindings);
    let p = position(positions, component.path);
    let component_extent = rect(
        -COMPONENT_HALF_WIDTH,
        -COMPONENT_HALF_HEIGHT,
        COMPONENT_HALF_WIDTH,
        COMPONENT_HALF_HEIGHT
    );
    let origin = p.x.to_string() + ", " + p.y.to_string();
    let placement = " annotation(Placement(transformation(origin = {"
        + origin + "}, extent = " + component_extent + ")));\n";
    line += placement;
    line
}

// Shared source emission for runtime and initialization composites.
fn instance_declaration(class, name, bindings) {
    let result = class + " " + name;
    if bindings.len > 0 {
        result += "(";
        for index in 0..bindings.len {
            if index > 0 { result += ", "; }
            result += bindings[index].name + " = " + bindings[index].value;
        }
        result += ")";
    }
    result
}

// Checked dimensions and scalar order come from the compiler owner.
fn initial_array_value(dimensions, values, depth, offset) {
    if depth == dimensions.len {
        if offset >= values.len { throw("initial input array has too few scalar bindings"); }
        return #{source: values[offset], next: offset + 1};
    }
    let source = "{";
    let next = offset;
    for index in 0..dimensions[depth] {
        if index > 0 { source += ", "; }
        let child = initial_array_value(dimensions, values, depth + 1, next);
        source += child.source;
        next = child.next;
    }
    #{source: source + "}", next: next}
}

fn initial_bindings(groups) {
    let bindings = [];
    for group in groups {
        let value = initial_array_value(group.dimensions, group.values, 0, 0);
        if value.next != group.values.len { throw("initial input array has excess scalar bindings"); }
        bindings.push(#{name: group.name, value: value.source});
    }
    bindings
}

// The importer supplies the exact initial causal graph and source snapshot.
fn initialize(plan) {
    if !valid_synthesis_context(runtime_context) {
        throw("synth.coupled-initialization requires its Twin lifecycle preparation context");
    }
    let text = "model " + plan.model_name + "\n";
    for member in plan.members {
        text += "  " + instance_declaration(member.class, member.instance, initial_bindings(member.bindings)) + ";\n";
    }
    for probe in plan.probes { text += "  output Real " + probe.name + ";\n"; }
    text += "equation\n";
    for probe in plan.probes { text += "  " + probe.name + " = " + probe.value + ";\n"; }
    text += "end " + plan.model_name + ";\n";
    #{source: text}
}

fn emit_unit(net, unit, components, members, positions) {
    let source = "model " + unit.name + "\n";
    let mutable = source;
    for input in unit.inputs { mutable += "  input Real " + input + ";\n"; }
    for output in unit.outputs { mutable += "  output Real " + output + ";\n"; }
    for member_output in net.member_outputs {
        if has_member(members, member_output.member_path) {
            mutable += "  output Real " + member_output.alias + ";\n";
        }
    }
    for path in unit.components {
        mutable += component_declaration(component_for(components, path), positions, net, members);
    }
    mutable += "equation\n";
    let electrical_bus = is_electrical_pin_network(net, unit.components, components);
    for edge in net.connections {
        if has_member(members, edge.left_path) && has_member(members, edge.right_path) {
            mutable += connect_equation(edge);
            if electrical_bus {
                mutable += power_bus_annotation(edge, positions);
            }
            mutable += ";\n";
        }
    }
    for boundary in net.boundary_outputs {
        if has_member(members, boundary.source_path) {
            mutable += output_connection(boundary);
        }
    }
    for member_output in net.member_outputs {
        if has_member(members, member_output.member_path) {
            mutable += "  " + member_output.alias + " = "
                + member_output.member_instance + "." + member_output.output + ";\n";
        }
    }
    let size = member_extent(positions, unit.components, net);
    let graphics = if electrical_bus {
        unit_bus_graphics(positions, unit.components)
    } else {
        let result = [];
        append_graphics(result, subsystem_graphics(net, unit.components, components, positions))
    };
    let decorated = append_graphics(graphics, physical_island_graphics(net, members, components, positions));
    decorated = append_graphics(decorated, causal_branch_graphics(net, members, positions));
    decorated = append_graphics(decorated, member_signal_graphics(net, members, positions));
    decorated = append_graphics(decorated, boundary_signal_graphics(net, members, positions));
    decorated = append_graphics(decorated, member_name_graphics(net, members, positions, components));
    decorated = append_graphics(decorated, member_summary_graphics(net, members, size));
    mutable += "annotation(" + compact_icon(UNIT_ICON_LINE_COLOR, UNIT_ICON_FILL_COLOR) + ","
        + "Diagram(coordinateSystem(extent=" + rect(size.left, size.bottom, size.right, size.top)
        + "), graphics={" + graphics_source(decorated) + "})" + ");\n";
    mutable += "end " + unit.name + ";\n\n";
    mutable
}

fn valid_synthesis_context(context) {
    context.scope == "twin"
        && context.cycle == "lifecycle"
        && context.phase == "preparation"
        && context.clock == "none"
        && context.generation != ()
        && context.time_seconds == ()
        && context.delta_seconds == ()
        && context.sequence == ()
        && context.producer == ()
}

fn synthesize(net) {
    if !valid_synthesis_context(runtime_context) {
        throw("synth.acausal-network requires its Twin lifecycle preparation context");
    }

    let components = component_index(net);
    let units = policy_units(net);
    let layout = visual_layout_with_components(net, units, components);
    let positions = member_position_index(layout);
    let unit_positions = unit_position_index(layout);
    let unit_members = #{};
    for unit in units { unit_members[unit.name] = member_index(unit.components); }
    let source = "model " + net.model_name + "\n";
    let mutable = source;
    for input in net.inputs { mutable += "  input Real " + input + ";\n"; }
    for boundary in net.boundary_outputs { mutable += "  output Real " + boundary.name + ";\n"; }
    for member_output in net.member_outputs { mutable += "  output Real " + member_output.alias + ";\n"; }
    for unit in units {
        let p = unit_position(unit_positions, unit.name);
        let unit_extent = rect(
            -UNIT_HALF_WIDTH,
            -UNIT_HALF_HEIGHT,
            UNIT_HALF_WIDTH,
            UNIT_HALF_HEIGHT
        );
        let origin = p.x.to_string() + ", " + p.y.to_string();
        let placement = " annotation(Placement(transformation(origin = {"
            + origin + "}, extent = " + unit_extent + ")));\n";
        mutable += "  " + unit.name + " " + unit.instance + placement;
    }
    mutable += "equation\n";
    for unit in units {
        for input in unit.inputs {
            mutable += "  " + unit.instance + "." + input + " = " + input + ";\n";
        }
        for output in unit.outputs {
            mutable += "  " + output + " = " + unit.instance + "." + output + ";\n";
        }
    }
    for member_output in net.member_outputs {
        for unit in units {
            let members = unit_members[unit.name];
            if has_member(members, member_output.member_path) {
                mutable += member_output_connection(member_output, unit.instance);
            }
        }
    }
    let size = unit_extent(unit_positions, units);
    let title = path_leaf(net.root) + " network | " + units.len.to_string() + " unit(s)";
    let root_graphics = [
        "Text(extent=" + rect(
            -size.width + DIAGRAM_TITLE_INSET_X,
            size.height - DIAGRAM_TITLE_TOP_INSET,
            size.width - DIAGRAM_TITLE_INSET_X,
            size.height - DIAGRAM_TITLE_BOTTOM_INSET
        ) + ", textString=" + string_literal(title)
            + ", textColor=" + DIAGRAM_TITLE_COLOR
            + ", fontSize=" + DIAGRAM_TITLE_FONT_SIZE_TEXT + ")"
    ];
    let graphics = append_graphics(root_graphics,
        root_signal_graphics(units, unit_positions));
    mutable += "annotation(" + compact_icon(ROOT_ICON_LINE_COLOR, ROOT_ICON_FILL_COLOR) + ","
        + "Diagram(coordinateSystem(extent=" + rect(-size.width, -size.height, size.width, size.height)
        + "), graphics={" + graphics_source(graphics) + "})" + ");\n";
    mutable += "end " + net.model_name + ";\n\n";
    for unit in units {
        mutable += emit_unit(net, unit, components, unit_members[unit.name], positions);
    }
    #{
        source: mutable,
        units: units,
        layout: layout,
        source_roots: net.source_roots,
        // Promotion is policy-owned. The default policy exposes every
        // declared member output; a scenario policy can omit or rename these
        // entries without a Rust change.
        member_output_aliases: net.member_outputs,
    }
}
