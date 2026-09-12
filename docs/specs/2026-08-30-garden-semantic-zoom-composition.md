# Garden Semantic Zoom Composition

- **Status:** Design direction; implementation linked, verification pending
- **Date:** 2026-08-30
- **Related:** `2026-06-02-malleable-garden.md`,
  `2026-07-30-garden-metric-map.md`,
  `2026-08-10-garden-file-terrain.md`, and
  `2026-08-23-agent-memory.md`

The [September 8 continuous-zoom contract](./2026-09-08-garden-continuous-zoom.md)
is the latest implementation direction and takes precedence for camera travel,
world-anchored composition and records, scrolling, and acceptance. Integration
and validation are pending. The [September 7 implementation record](./2026-09-07-garden-composition-implementation.md)
preserves historical decisions and validation; its DOM cutaway and omission of
continuous reverse-wheel navigation no longer override this design's intent.
Its canonical data and evidence rules remain applicable unless revised by the
latest contract. The context below describes the pre-implementation baseline.

## Context

The current Garden is a useful operational map, but its presentation is still
mostly utilitarian. Agents are compact status orbs, automation blueprints are
independent units, and workspace contents are rendered as filesystem terrain.
Zoom reveals more skill glyphs and subdivides more filesystem cells, but it does
not yet create the feeling of moving between meaningful scales.

Two inspirations define the intended experience:

1. Playful agent worlds in which agents read as inhabitants of a place rather
   than rows in an administration tool.
2. Infinite-zoom experiences in which moving closer changes the object of
   attention: a habitat becomes a team, a team becomes an agent, and an agent
   opens into the memories, capabilities, and active systems that compose it.

The design problem is therefore not just visual polish. Garden needs a
composition hierarchy that says what exists at each scale, how the user crosses
between scales, and which relationships are expressed as placement, attachment,
terrain, or flow.

Two parts of the current model need particular revision:

- Automation blueprints float as independent units even when the important
  question is which agents participate in a deployment or run. Their spatial
  association is consequently difficult to interpret.
- Filesystem ground is effective at giving each team a stable territory, but a
  complete directory treemap is rarely the operational question. The useful
  signal is usually which files agents are changing now or changed recently.

## Product Intent

Garden is a living, spatial explanation of Wardian's current activity and
composition. It should answer progressively deeper questions without becoming a
second source of truth:

- Where is work happening?
- Who is participating?
- What are they doing?
- What is an agent, workspace, or run made of?
- What evidence supports what the map is claiming?

The playful ecological and cellular metaphors are visual and interaction
languages. User-facing labels remain operational: **Memory**, **Skills**,
**Active work**, **Workspace**, and **Run**, rather than invented biological
names that users must translate.

## Decision Principles

### Zoom changes meaning, not only magnification

Every semantic zoom band has one primary noun and one primary question. Moving
closer reveals composition or evidence, not merely smaller labels.

### Scope is a lens, not a universal containment tree

Teams, workspaces, agents, skills, memories, and automations overlap. A team can
span workspaces, a skill can be carried by several agents, and a memory can be
agent-wide or workspace-bound. Garden may project one canonical object through
several lenses, but it must not invent exclusive ownership to make the picture
nest cleanly.

### Addressability does not require permanent placement

An entity may have canonical identity and an independent lifecycle without
needing a permanent location in Garden. Permanent placement is reserved for
objects whose location answers a useful spatial question.

This revises the earlier heuristic that every independent lifecycle earns a
unit. Automation blueprints remain independently addressable Library objects,
but their deployment and execution relationships are usually more informative
than a free-floating blueprint position.

### Stable context, changing signal

Team territories, workspace zones, and authored placement remain stable.
Telemetry, file activity, run state, and recency change paint, emphasis, trails,
and visible detail without moving those landmarks.

## Semantic Zoom Hierarchy

| Band | Primary object | Visible composition | Question |
| --- | --- | --- | --- |
| **Habitat** | The whole Wardian ecology | Team or workstream territories, Commons, aggregate health and activity | Where is work happening? |
| **Workstream** | One team or district | Agent inhabitants, workspace ground, active routines, recent activity | Who is working together, and on what? |
| **Focus** | An agent, workspace, or automation run | Connections to nearby context and an affordance to enter the object | What is this connected to? |
| **Composition** | The focused object's internal systems | Agent regions, workspace activity groups, or run stages | What is this made of? |
| **Record** | One memory, skill, file, stage, output, or run event | Content, scope, provenance, revisions, and evidence | What exactly is this, and can I trust it? |

The hierarchy branches at **Focus** rather than forcing every object through one
interior model:

- **Agent:** identity, capabilities, memory, active work, and connections.
- **Workspace:** active directory groups, changed files, attribution, and diffs.
- **Automation run:** stages, assigned agents, temporary providers, outputs,
  failures, and memory commits.

The same canonical record may appear in several branches. For example, a
workspace-bound memory can appear in an agent's Memory region and in that
workspace's activity context. Selecting either projection resolves to the same
record.

**Focus is a transition state rather than a separate population layer.** It
previews the interior of the selected object, suppresses irrelevant detail, and
preserves the surrounding workstream as return context. Double-click or
continued zoom carries the user through Focus into Composition.

Semantic bands are determined by the focused object's screen-space extent, not
by one global camera-scale number. A scale value that makes a small district
readable may still leave a large district at Habitat density. Transitions use
hysteresis so a label, region, or activity cluster does not flicker when the
camera rests on a threshold.

## Rendering Direction

### Visual character

Garden uses a **soft vector micro-ecology**: calm organic silhouettes, restrained
surface texture, precise labels, and sparing state motion. The playful agent
world references inform liveliness and spatial composition, but Wardian does
not adopt literal pixel-art characters as the default. Pixel avatars are
charming in small scenes and become repetitive, difficult to theme, and harder
to scan in a dense operational map.

The visual system follows these rules:

- Stable entities use neutral themed surfaces. Live status colour appears as a
  core, aura, boundary, or route state rather than recolouring the whole object.
- Shape carries object kind: territories enclose, agents inhabit, workspace
  activity forms ground patches, automations connect, and records provide a
  reading plane.
- Identity remains consistent as representation changes. An agent's monogram or
  future user-assigned sigil survives from inhabitant scale into its Identity
  core; a workspace keeps the same root label and ground texture at every scale.
- Labels use Wardian's canvas typography and theme tokens. Text does not shrink
  below a readable size; it appears, shortens, groups, or disappears according
  to available screen space.
- Texture is low-contrast and decorative only. Status, selection, change kind,
  evidence, and focus never depend on texture.
- Motion means transition or live activity. Processing may breathe gently,
  active routes may carry a slow directional pulse, and semantic dives may
  morph. Idle objects remain still.
- Selection receives a high-contrast outline and a second non-colour cue.
  Status and change kinds retain text or shape equivalents so colour is never
  the sole discriminant.

### Representation progression

Objects progress through four visual roles as the camera moves closer:

1. **Signal:** the smallest truthful mark, such as a status dot or activity
   tint.
2. **Identity:** a labelled inhabitant, patch, attachment, or route that can be
   distinguished from its peers.
3. **Container:** an opened object whose internal regions or stages become the
   current world.
4. **Record:** a readable object with provenance, history, and canonical
   actions.

This progression is a morph of one object, not four unrelated components.

## Objects by Semantic Band

### Habitat

Habitat scale prioritizes territories and aggregate signal.

- **Team or workstream:** a softly bounded territory with its name, agent count,
  aggregate status, and current activity level. The silhouette is stable and
  low saturation; active areas receive sparse change paint rather than filling
  the whole territory.
- **Commons:** a named central territory using the same visual grammar, not an
  empty gap or special dashboard card.
- **Agent:** a small status dot placed inside its territory. The visible mark is
  minimal, but the hit target is larger when dots do not overlap. Dense groups
  collapse into a status-distributed population cluster rather than creating a
  pile of inaccessible hit targets.
- **Workspace:** a broad, quiet subdivision only when a team spans several
  roots. Its label appears when it is necessary to explain the territory.
- **File activity:** aggregate heat and change-kind ticks only. No folder or file
  names are shown.
- **Automation:** individual routines and routes are hidden. A team label may
  carry a compact running or action-required count; a selected cross-team run
  may reveal a single restrained bridge.
- **Skill and memory:** not individually rendered.

### Workstream

Workstream scale makes participants and current work identifiable.

- **Team or workstream:** the territory becomes the stable environmental frame.
  Its title moves to the edge so it does not compete with inhabitants.
- **Agent:** a compact cell-like inhabitant with a neutral body, status core or
  aura, consistent monogram or sigil, and a readable name. Class and provider
  remain supporting detail on selection or hover. Up to three distinctive skill
  marks may form a restrained crown; overflow is summarized rather than wrapped
  around the whole agent.
- **Workspace:** a labelled ground bed. It is visually quieter than agents and
  uses a stable root texture rather than a recursive treemap.
- **Activity cluster:** an organic patch representing active or recent files
  under a meaningful common ancestor. It shows the ancestor name, file count,
  dominant change state, and collaborator count.
- **File:** not rendered separately unless the cluster contains only one active
  file and enough space exists; otherwise it remains aggregated.
- **Single-agent automation:** a small routine attachment docked to the agent's
  perimeter, carrying schedule or run state without becoming a second unit.
- **Multi-agent automation:** a directed route between assigned agents. The
  route label is compact; stage markers appear only for active, blocked, or
  selected runs. Concurrent runs of one deployment aggregate into one route
  with a run count at this scale.
- **Skill:** the most distinctive deployed skills appear as skill marks on their
  carrier. They never receive independent map positions.
- **Memory:** not individually rendered. A selected agent may show a quiet
  memory-presence indicator, but counts do not ring every inhabitant.

### Focus transition

Focus explains what will open before the surrounding context recedes.

- The selected object grows toward a stable target extent while neighbours
  desaturate and stop revealing secondary labels.
- Canonical relationships to agents, workspace activity, routines, or ports
  strengthen; unrelated routes and paint fade.
- An **Enter Agent**, **Inspect activity**, or **Inspect run** affordance appears
  only when the next semantic level exists.
- The object's container boundary and coarse internal regions become visible,
  but individual records remain hidden until Composition.

### Composition

Composition scale turns the selected object into the current spatial container.

- **Agent container:** occupies most of the viewport while the originating team
  remains as a quiet outer context. Identity, Capabilities, Memory, Active work,
  and Ports use a stable internal layout. The live agent status moves from its
  external aura to the boundary or membrane.
- **Workspace container:** the stable bed becomes an activity hierarchy. Active
  ancestor groups subdivide into folders and files; unchanged siblings appear
  only when needed to preserve ancestry or when **Show full tree** is enabled.
- **Automation container:** the route expands into its directed stage flow.
  Assigned agents anchor the relevant stages, temporary providers use transient
  silhouettes, and outputs or failures remain attached to the stage that
  produced them.
- **Skill:** a capability organelle with icon or monogram, name, provenance,
  deployment scope, and sync state. Related skills may cluster without moving
  the Capability region itself.
- **Memory:** a rounded memory record grouped first by Stable or Current and then
  by agent-wide or workspace-bound scope. The visible excerpt is short; evidence
  and revision depth are secondary marks rather than paragraphs on the canvas.
- **File:** a labelled leaf with change kind, recency, attribution count, and
  evidence state. Source content remains deferred to Record scale.
- **Automation stage or output:** a selectable part of the expanded run flow,
  not a new global unit.

### Record

Record scale prioritizes reading and provenance over spatial density.

- The selected memory, skill, file, stage, output, or run event expands into a
  calm reading plane inside the current container.
- An agent identity Record exposes **Open agent session** as its canonical
  operational action. This leaves the spatial hierarchy and opens the same
  agent in the Workbench **Agent Session** surface's terminal presentation; it
  does not introduce a second chat or terminal mode inside Garden.
- The primary content, scope, source, revision or run history, and evidence are
  readable without hover.
- The parent container remains visible as a narrow contextual frame and the
  breadcrumb remains available. Unrelated inhabitants and terrain are omitted.
- Canonical actions such as **Open file**, **Open in Library**, **View agent**,
  **Edit schedule**, or **Inspect run evidence** are explicit buttons. The
  spatial projection never silently becomes the authority for editing.

### Same-object morphs

| Object | Habitat signal | Workstream identity | Composition container | Record |
| --- | --- | --- | --- | --- |
| Agent | Status dot | Cell-like inhabitant with name and sigil | Agent cutaway with stable regions | Identity, configuration, or selected internal record |
| Team | Territory silhouette and aggregate | Environmental frame | Quiet outer context and ports | Team metadata when explicitly opened |
| Workspace | Optional broad subdivision | Labelled ground bed | Active folder and file hierarchy | File, diff, snapshot, or workspace evidence |
| File activity | Aggregate heat | Ancestor activity patch | Active folders and file leaves | File content and attribution evidence |
| Automation | Aggregate team state only | Attached routine or participant route | Directed stages, assignments, and outputs | Schedule, run, stage, or output evidence |
| Skill | Hidden | Distinctive carrier mark | Capability organelle | Skill definition, provenance, and deployments |
| Memory | Hidden | Presence only on selected agent | Scoped memory record | Full text, evidence, revisions, and history |

## Agent Cutaway

At close range, an agent changes from an inhabitant into a stable cutaway. The
surrounding workstream remains visible but recedes, preserving where the user
came from.

The internal regions are consistent across agents so operators can build spatial
memory:

| Visual role | Operational contents |
| --- | --- |
| Boundary or membrane | Live status, permissions, communication boundaries, and connection ports |
| Identity core | Agent identity, class, provider configuration, prompt, and instructions |
| Capabilities | Skills and tools, including direct, class-inherited, and global provenance |
| Memory | Stable and current memories, grouped by agent-wide and workspace-bound scope |
| Active work | Current conversation, queued work, active routines, and recent outputs |
| Ports | Teams, workspaces, agents, and automation relationships that cross the boundary |

Object constancy should carry the transition. The external status aura becomes
the boundary, existing skill-crown glyphs migrate into Capabilities, relevant
workspace terrain resolves into ports, and memory structures emerge inside the
agent. The agent is transformed rather than replaced by an unrelated detail
screen.

Internal regions use a stable template rather than a force-directed layout.
Items may be ordered or clustered within a region, but Memory should not move to
a different side of every agent merely because its contents differ.

## Automation as Behavior

### Problem

An automation blueprint can name an agent directly, state only a role or class
requirement, receive an agent through a schedule, or use a temporary provider.
A permanent blueprint unit cannot communicate all of those relationships with
position alone. When it floats between agents, the map appears to assert an
association without explaining whether that association is required, deployed,
or currently running.

### Considered directions

| Direction | Model | Strength | Risk |
| --- | --- | --- | --- |
| Infrastructure object | A blueprint is a building or machine in a team territory | Persistent and playful | Still needs ambiguous tethers to every possible participant |
| Agent attachment | An automation is a satellite or routine attached to one agent | Very clear for single-agent schedules | Misrepresents multi-agent runs and temporary providers |
| Behavioral path | The blueprint stays in Library; deployments and runs appear as paths through actual participants | Makes the operational relationship primary | Requires a good dormant or scheduled representation |

### Working direction

Garden should represent automations primarily as **behavioral paths**, with
representation based on their current operational state:

- An undeployed blueprint is not placed. Like an undeployed skill, it remains a
  generic Library element until a schedule or run gives it situated meaning.
- A scheduled single-agent automation appears as a compact attached routine on
  that agent, not as a second free-floating unit.
- A scheduled automation with no durable agent assignment is anchored to its
  configured workspace. This covers pure script automations and
  temporary-provider schedules without inventing an agent owner.
- An active multi-agent run appears as a route or circuit connecting the agents
  actually assigned to its stages. Stage markers show progression along the
  route.
- A temporary provider appears as an ephemeral participant on the route, with a
  distinct transient silhouette rather than a durable agent position.
- A completed run leaves a fading recent trail when it remains relevant to the
  selected recency window. Its durable evidence remains available at Record
  scale after the trail disappears.
- A failed or action-required stage interrupts the route at the failing point so
  responsibility and blockage are visible without opening the run.

Dormant and paused routines remain visually quieter than live work. A dormant
schedule uses a thin hollow attachment or route with no motion; a paused
schedule adds a broken stroke. A running route becomes solid and carries a slow
directional pulse. Awaiting-action and failed stages interrupt the route with a
labelled Wardian status mark rather than changing the entire path to one alarm
colour.

Concurrent runs of one deployment aggregate into one route with a count at
Workstream scale. Opening Automation Composition separates them into parallel
run lanes ordered by start time, preserving participant anchors and allowing a
blocked run to be inspected without drawing duplicate routes across the map.

Selecting a routine or route makes its assignments explicit. Entering it opens
the Automation Run branch, where the blueprint structure, stage state, outputs,
and evidence can be inspected. The blueprint is still canonical; Garden is
showing its situated execution rather than inventing a second automation object.

### Projection identity

The Garden projection is a **situated binding, deployment, or run instance**,
not the generic automation blueprint itself. This distinction is necessary
because two schedules of one blueprint can carry different role assignments,
workspaces, cadence, and state. Pooling them by blueprint id produces one
ambiguous object whose supposed location is the union of unrelated deployments.

- A blueprint containing a direct agent id has one situated binding even before
  it is scheduled. A `role:` or `class:` requirement does not; it becomes
  situated only when a schedule or run resolves the role to a concrete target.
- A persisted schedule is one durable Garden projection, keyed by schedule id.
- A manual run without a schedule is a temporary projection, keyed by run id,
  while it is active or inside the recent-activity window.
- An active scheduled run temporarily supplies live state to its schedule
  projection rather than creating a visually duplicate automation beside it.
- The blueprint remains the canonical definition linked from every projection.

### Placement ladder

The number of concrete agent associations changes the *kind* of projection,
not just the coordinates of one universal automation unit:

| Situated evidence | Garden representation | Placement rule |
| --- | --- | --- |
| No direct binding, schedule, or active or recent run | Not shown | The generic blueprint remains in Library |
| Schedule or run with no concrete agents, but with a workspace | Workspace routine | Dock to that workspace's stable ground or boundary |
| No concrete agents and no workspace | Not spatially shown | Keep it in Automation Monitor; Garden must not fabricate a location |
| Direct binding, schedule, or run with exactly one concrete agent | Agent routine | Attach to a stable slot on that agent's perimeter; the automation has no independent map position |
| Exactly two concrete agents | Directed route | Connect the participants in execution order; place the label and controls on the route rather than at an unrelated point |
| Three or more concrete agents | Routed network | The participant set is the location; connect role transitions and use one neutral route label, not a centroid unit |
| Temporary-provider assignment | Workspace-bound routine at rest; ephemeral participant while active | Use the assignment workspace, then show the transient participant only for the live or recent run |

A concrete agent assignment outranks workspace anchoring for the routine itself.
If the schedule workspace differs from that agent's workspace, the routine stays
attached to the agent and the workspace appears as a destination port or
activity relationship. This states both truths instead of moving the routine
away from the actor that performs it.

For multi-agent automations, asking where the automation “lands” is the wrong
spatial question. It does not have a residence separate from its assignees. Its
extent is the set of participating agents, and its geometry is the execution
path between them. The route label is a selection handle, not a claim that the
automation belongs at the geometric midpoint.

The route is derived from blueprint execution order with consecutive stages on
the same agent collapsed into one segment. A live run's actual assignments and
stage progress outrank the stored schedule projection. A dormant schedule uses
its saved role assignments to preview the same route without active motion.

This direction remains provisional until it is tested against concurrent runs
of one schedule, several schedules that overlap on the same agents, and
workflows with many temporary providers.

## Activity-Centered Ground

### Preserve the territory

The ground metaphor remains valuable. It gives each team a stable spatial zone,
makes cross-workspace teams visible, and lets operators remember where a body of
work lives. The revision is not to remove ground, but to change what the ground
chooses to reveal.

The default ground should not attempt to visualize the complete file tree.
Instead, it renders an **activity frontier** composed of:

- files being changed now;
- files changed recently within the selected activity window;
- the minimum ancestor folders needed to explain where those files live; and
- coarse workspace or repository beds that preserve the team's stable territory.

Unchanged filesystem content remains available through workspace focus or an
explicit **Show full tree** lens. It is context on demand, not default Garden
population.

### Activity hierarchy

At Habitat scale, a workspace bed shows aggregate activity and collaborator
count without individual paths. At Workstream scale, changed top-level regions
become landmarks such as `src/features` or `docs/specs`. At Workspace Focus,
those regions subdivide into active folders and files. Record scale opens the
file, diff, snapshots, attribution, and relevant conversation turns.

If many changed files share an ancestor, they first appear as one activity
cluster with a count and dominant change state. Zooming or selecting the cluster
reveals its files. The tree is therefore used to provide comprehensible ancestry
for activity, not displayed recursively for its own sake.

### Agent relationships

The primary file question is what agents are doing with files around them.
Garden expresses that relationship through selective attribution:

- Selecting an agent highlights every active or recent path attributed to it.
- Selecting a file or activity cluster highlights its participating agents.
- Attribution threads appear only for the current selection or focused run, not
  for every agent and file simultaneously.
- Solid and dashed evidence treatment continues to distinguish attributed from
  inferred activity.
- Files touched by several agents show collaboration at the file cluster rather
  than duplicating the file in several territories.

Activity does not move agents, team zones, or workspace beds. Active path
placement is deterministic within its stable workspace region so a file that
leaves and re-enters the recency window returns to the same place.

### Time lens

The default time lens should emphasize **Now** and **Recent**, with older work
fading rather than disappearing abruptly. A broader **Branch** or **History**
lens can reuse branch-point changes and durable evidence when the operator is
investigating accumulated work.

The implementation contract resolves the initial thresholds: **Now** uses the
newest two turns, **Recent** the newest sixteen, and **Branch** retains the
whole current workspace comparison. Unknown recency remains visible. Automation
run recency uses a separate 24-hour window, with active runs retained regardless
of age. See the implementation contract for comparison and evidence semantics.

## Interaction Grammar

### Universal pointer rule

Garden uses one learnable distinction at every scale:

- **Single click selects and explains.** It never moves the camera or opens a
  different workbench surface. The selection summary names the object, its
  current state, and the most relevant relationship; reciprocal objects
  highlight in place.
- **Double-click opens the object at its next meaningful semantic scale.** A
  team opens to Workstream, an agent opens to its cutaway, a route opens to its
  run flow, and a record opens its canonical detail when no deeper spatial scale
  remains.

Hover may preview the name and kind, but it reveals no information or action
that is unavailable after selection. Labels and shapes share one hit target so
users do not have to discover whether the text or the object is interactive.

### Object actions

| Object | Single click or Space | Double-click or Enter |
| --- | --- | --- |
| Team or workstream | Select the territory; show aggregate status, activity, workspaces, and automation attention | Enter Workstream scale and fit that territory |
| Agent | Show status and purpose; highlight attributed file activity, attached routines, routes, and direct relationships; expose **Open agent session** as a secondary action | Enter the agent cutaway |
| Workspace bed | Show root, participating agents, active clusters, and aggregate change state | Enter Workspace Composition |
| Activity cluster or folder | Show changed-file count, recency, evidence, and participating agents | Descend into that activity group without revealing unrelated tree branches |
| File | Show change kind, recency, attribution, and relevant run or conversation | Open the file Record plane; from Record, Enter invokes **Open file** |
| Single-agent routine | Show schedule, next or current run, owning role, and agent association | Open Automation Composition for that deployment or run |
| Multi-agent route | Select the whole route; emphasize participants, direction, stage state, and blockage | Open Automation Composition with the run flow fitted |
| Automation stage or output | Show stage owner, status, input or output summary, and evidence | Open the stage or output Record plane |
| Skill mark or organelle | Show name, provenance, sync state, and reverse-highlight every carrying agent | Open the skill Record plane; from Record, Enter invokes **Open in Library** |
| Memory record | Show scope, kind, evidence source, and revision state | Open the memory Record plane; from Record, Enter invokes the canonical memory detail |
| Port or relationship endpoint | Select the relationship and highlight both ends | Jump to the counterpart through Focus while preserving the return breadcrumb |

Selection is singular by default. Modifier-click may add objects for comparison
later, but multi-selection is not required for the first semantic-zoom slice.

### Background, drag, and context actions

- Clicking empty ground clears selection. Double-clicking empty ground performs
  one camera zoom step at the pointer but does not nominate a semantic target.
- Dragging empty ground pans the camera.
- Agents may be dragged only at Workstream scale, where placement within a
  territory is meaningful and already persisted.
- Automation routes, activity patches, workspace beds, and internal organelles
  are derived projections and are not draggable. Their context menu may offer
  canonical actions, but a drag must not imply an unsupported ownership or
  deployment mutation.
- Agent internals use the stable template and are not manually rearranged in
  this direction. Allowing it would weaken the cross-agent spatial memory the
  cutaway is meant to establish.
- Right-click opens the existing contextual action menu without changing the
  single-click or double-click contract.

### Keyboard and touch

- Visible semantic objects participate in a roving keyboard focus order. Space
  selects, Enter opens the next scale or canonical record action, Escape rises
  one level, and arrow keys continue to pan when the canvas itself has focus.
- The focused object receives the same high-contrast treatment as pointer
  selection and exposes its type, name, state, and available open action to
  assistive technology.
- Touch does not require double-tap. The first tap selects and exposes a clear
  **Enter** or **Open** action in the selection summary; a second tap on that
  explicit action performs the semantic dive.
- Minimum hit targets are based on screen pixels rather than world units. When
  far-scale agent targets would overlap, the population cluster becomes the
  target and opens the workstream before individual selection is offered.

### Navigation and transitions

- Zooming into empty space changes camera scale only; it never chooses a focus
  target implicitly.
- Selecting an entity nominates it as the semantic dive target. Continued zoom
  crosses into it; double-click or Enter is the explicit shortcut.
- Near a semantic threshold, the target previews its interior and names the
  action, such as **Enter Agent** or **Inspect activity**.
- Zooming out reverses the transformation. Escape rises one semantic level.
- A breadcrumb such as `Habitat > Team > Agent > Memory` supplies direct return
  paths and exposes the current lens to keyboard and assistive users.
- Cross-surface actions preserve the Garden surface rather than replacing it.
  **Open agent session** uses contextual Workbench navigation to focus or open
  the agent's canonical `agent-session` terminal surface. Closing that surface
  or invoking Workbench Back returns to the same Garden surface with its
  breadcrumb, camera, semantic level, focus target, and selection intact.
- Reduced-motion mode replaces morphing with a short crossfade while preserving
  focus and object position.
- If activity or memory data is loading, stale, unavailable, or permission
  restricted, the affected region remains spatially present and states the
  limitation. It does not collapse and reflow the surrounding scene.

The current Garden uses double-click to open an agent session, skill detail, or
file directly. Under semantic zoom, those destinations remain available as
explicit canonical actions. For agents, **Open agent session** appears in the
selection summary at every scale where the agent is selectable, in the agent
context menu, and on its identity Record; it opens the Workbench Agent Session
terminal surface described above. Double-click first means “open this object”
in the Garden's own hierarchy so the same gesture does not unexpectedly leave
the spatial experience for some object kinds and descend for others.

## Non-Goals

- Replacing Library as the blueprint, skill, prompt, or class authority.
- Replacing Explorer or the file viewer with a permanently expanded tree.
- Encoding every canonical relationship as a visible line.
- Making agent status, file activity, or run telemetry alter stable geometry.
- Requiring the biological metaphor to become user-facing terminology.
- Defining implementation phases or renderer changes before the hierarchy is
  validated through a storyboard or prototype.

## Success Criteria

The direction is successful when representative operators can:

1. Identify which team or workstream is active at Habitat scale.
2. Identify which agents are collaborating on a changed area without opening a
   sidebar list.
3. Explain which agents participate in a selected automation run and where it is
   blocked.
4. Enter an agent and distinguish identity, capability, memory, and current work
   without learning a unique layout for every agent.
5. Follow one memory, skill, file, or run output to provenance and evidence.
6. Return to the prior spatial context without becoming lost in a separate
   detail surface.

## Open Questions

- Do the initial turn-based file windows and separate 24-hour run window in the
  implementation contract support representative investigations?
- Do the implementation's initial screen-space extents and hysteresis remain
  usable across representative densities and viewport sizes?
- How many concurrent run lanes can Automation Composition show before older or
  lower-priority runs need a second level of aggregation?

## Consequences

- **Positive:** Semantic zoom gains a coherent purpose at every scale.
- **Positive:** Automation relationships are shown from real deployments and
  runs instead of inferred from a blueprint's floating position.
- **Positive:** Team ground remains a stable and memorable zoning system while
  file detail becomes activity-centered.
- **Positive:** Memory has a natural place inside the agent without hiding its
  workspace scope or provenance.
- **Negative:** Garden needs different close-range compositions for agents,
  workspaces, and automation runs.
- **Negative:** Automation display becomes more dependent on runtime and schedule
  evidence than the current blueprint-unit projection.
- **Negative:** Activity recency and aggregation require explicit product rules
  before implementation can be considered deterministic.
