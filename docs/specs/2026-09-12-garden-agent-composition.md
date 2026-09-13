# Garden Agent Composition and Automation Orbits

## Status

Implemented contract for the Garden agent composition. This refines the
hierarchical cell model and supersedes older five-tray content groupings where
they conflict.

## Problem

Multiple routines attached to one agent were assigned the same horizontal
offset and successive vertical offsets. At realistic density this made an
agent the origin of a text list rather than a participant in a spatial map.
Failed stages also drew a large error ring over the agent.

Inside the expanded cell, Conversations and Inbox were nested below
Automations, Tools was a footer disclosure below Skills, and a wide
Workspace/Teams/Agents tray repeated relationships already expressed by the
district map. Large type and padding left too little useful collection area.

## Contract

### Automation geography

- Single-agent routines occupy deterministic expanding rings around that
  agent. The first ring has eight slots; later rings grow in capacity and
  radius. Stable ordering keeps anchors from orbiting during refresh.
- Routes with several participants keep their execution path and fan repeated
  routes to alternating sides of the participant midpoint.
- At dense attachment points, ordinary labels remain folded until zoom creates
  enough screen circumference. Selection always reveals the selected label.
- Canonical selection, keyboard entry, route fading and reverse zoom remain
  unchanged.
- Failed stages retain a compact local × and status label but do not draw an
  attention ring around the agent. Awaiting approval may retain its distinct
  local ring.

### Agent organelles

Every readable agent uses five direct organelles:

1. Identity
2. Skills
3. Memory
4. Automations
5. Conversations

Workspace, team and peer relationships belong to district geography and are
not repeated as an agent organelle. Tools and Inbox are not projected as
secondary drawers in the agent cell. Conversations is a direct, independently
scrollable collection.

The fixed 900px internal coordinate system remains stable. Type, padding and
object spacing are reduced; sparse agents stay compact while dense Skills,
Memory, Automations and Conversations scroll without changing the geography.
Memory search and canonical object anchors remain available at existing
thresholds.

## Validation

- Pure layout tests cover twenty deterministic single-agent attachment slots.
- Layer tests cover dense label disclosure, selected-label override, reverse
  thresholds and the absence of a failed-stage ring.
- Agent tests cover the five direct regions and absence of the removed drawers.
- Browser coverage exercises twenty assigned automations, 300 memories and 60
  conversations, including dark theme, narrow viewport, keyboard entry,
  collection scrolling and reverse navigation.
