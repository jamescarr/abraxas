//! Deterministic host generation: the node graph a door run is played on.
//!
//! Pure logic. The shell (`game::shell`) drives a [`HostSession`]; the UI renders it through the
//! accessors here and interprets the shell's effects.

use std::collections::VecDeque;
use std::time::Duration;

use rand::RngExt;
use rand::seq::{IndexedRandom, SliceRandom};

use crate::game::danger::{IceGrade, params, roll_grade};
use crate::game::programs::Deck;
use crate::game::shell::ShellEffect;
use crate::game::state::Level;
use crate::ice::IceOutcome;
use crate::ice::duel::BlackClass;
use crate::pack::model::{ExitKind, HostOs, ObjectiveIce, ObjectiveKind};
use crate::rng::{Rng, substream};

/// EGO lost to a DUMP SHOCK.
pub const DUMP_SHOCK_EGO: i32 = -20;
/// KB for the first `get` of a non-objective file.
pub const FILE_REWARD_KB: u32 = 8;
/// Trace removed by a solved ROUTER bounce.
pub const BOUNCE_TRACE_RELIEF: f32 = 30.0;
/// Trace added by retreating from a duel.
pub const RETREAT_TRACE: f32 = 10.0;
/// Fallback decoy process names when the pool offers too few.
const FALLBACK_DAEMONS: [&str; 5] = ["init", "cron", "syslogd", "lpd", "inetd"];
/// Kinds handed to the nodes that are neither portal nor objective, in order.
const KIND_CYCLE: [NodeKind; 6] = [
    NodeKind::Archive,
    NodeKind::Gate,
    NodeKind::Relay,
    NodeKind::Gate,
    NodeKind::Archive,
    NodeKind::Gate,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeKind {
    Portal,
    Gate,
    Relay,
    Archive,
    Vault,
    Core,
    Mirror,
}

impl NodeKind {
    /// Lowercase prefix of command ids.
    pub fn label(self) -> &'static str {
        match self {
            NodeKind::Portal => "portal",
            NodeKind::Gate => "gate",
            NodeKind::Relay => "relay",
            NodeKind::Archive => "archive",
            NodeKind::Vault => "vault",
            NodeKind::Core => "core",
            NodeKind::Mirror => "mirror",
        }
    }
}

/// What guards a node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IceKindAt {
    Wordlock,
    Breach,
    Phreak,
    Daemon,
    Black,
    Shadow,
}

impl IceKindAt {
    pub fn label(self) -> &'static str {
        match self {
            IceKindAt::Wordlock => "WORDLOCK",
            IceKindAt::Breach => "BREACH",
            IceKindAt::Phreak => "PHREAK",
            IceKindAt::Daemon => "DAEMON",
            IceKindAt::Black => "BLACK ICE",
            IceKindAt::Shadow => "SHADOW",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostFile {
    pub name: String,
    pub description: String,
    pub body: String,
    /// KB for the first fetch (non-objective files only; 0 for the target).
    pub reward_kb: u32,
    pub fetched: bool,
    pub is_target: bool,
}

impl HostFile {
    /// Size shown by `ls`.
    pub fn size(&self) -> usize {
        self.body.len()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Command id: kind + index, e.g. `gate2`.
    pub id: String,
    /// Display name from the level's `node_names`.
    pub display: String,
    pub kind: NodeKind,
    pub ice: Option<IceKindAt>,
    /// Per-instance difficulty of the guard (Standard when the node has no ICE).
    pub grade: IceGrade,
    /// The black ICE creature; `Some` only when `ice` is `Black`.
    pub class: Option<BlackClass>,
    pub cleared: bool,
    pub files: Vec<HostFile>,
    pub neighbors: Vec<usize>,
}

impl Node {
    /// True while an ICE still guards the node.
    pub fn guarded(&self) -> bool {
        self.ice.is_some() && !self.cleared
    }

    /// `scan` / `connect` description of the guard: `None` without ICE, else
    /// `KIND grade`, `BLACK/CLASS grade`, or just `SHADOW`.
    pub fn guard_desc(&self) -> Option<String> {
        self.ice.map(|ice| match ice {
            IceKindAt::Shadow => "SHADOW".to_string(),
            IceKindAt::Black => format!(
                "BLACK/{} {}",
                self.class.unwrap_or(BlackClass::Wolf).label(),
                self.grade.label()
            ),
            other => format!("{} {}", other.label(), self.grade.label()),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveState {
    Pending,
    Done,
}

/// One row of a `ps` listing (crash objectives).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Process {
    pub pid: u32,
    pub name: String,
    pub is_target: bool,
}

/// Colour of the shell prompt; the UI maps these to DOS colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptColor {
    LightGreen,
    Yellow,
    WhiteOnBlue,
    LightGray,
}

/// What the shell needs to know about the player.
#[derive(Debug, Clone, Default)]
pub struct PlayerView {
    pub handle: String,
    pub shadow: String,
    pub deck: Deck,
    pub dev: bool,
    /// In-world date and time printed by `date`.
    pub clock: String,
}

/// A node placed for the `scan` map: column = `layer * 14`, row = `row * 3`.
#[derive(Debug, Clone, PartialEq)]
pub struct MapNode {
    pub index: usize,
    pub id: String,
    pub display: String,
    pub kind: NodeKind,
    pub ice: Option<IceKindAt>,
    pub cleared: bool,
    pub is_current: bool,
    /// BFS depth from node 0.
    pub layer: usize,
    /// Position among the nodes of the same layer (by node index).
    pub row: usize,
    pub neighbors: Vec<usize>,
}

#[derive(Debug, Clone)]
pub struct HostSession {
    pub(crate) os: HostOs,
    pub(crate) host_name: String,
    pub(crate) banner: String,
    pub(crate) objective_text: String,
    pub(crate) objective_kind: ObjectiveKind,
    pub(crate) objective_node: usize,
    pub(crate) objective: ObjectiveState,
    /// Retrieve: the file; plant: the payload file name; crash: empty.
    pub(crate) target_file: String,
    pub(crate) target_daemon: String,
    pub(crate) processes: Vec<Process>,
    pub(crate) nodes: Vec<Node>,
    pub(crate) current: usize,
    pub(crate) run_danger: u8,
    pub(crate) exit_kind: Option<ExitKind>,
    pub(crate) attempt: u32,
    pub(crate) trace: f32,
    pub(crate) ghost: bool,
    pub(crate) bounce_used: bool,
    pub(crate) dumped: bool,
    pub(crate) whoami_seen: bool,
    pub(crate) player: PlayerView,
}

impl HostSession {
    /// Deterministic from `substream(seed, level.seed.depth, "host", attempt)`.
    /// `daemons` are the pack pool's process names (decoys for `ps`).
    pub fn generate(
        level: &Level,
        run_danger: u8,
        exit_kind: Option<ExitKind>,
        attempt: u32,
        seed: u64,
        daemons: &[String],
    ) -> HostSession {
        let run_danger = run_danger.clamp(1, 5);
        let mut rng = substream(seed, level.seed.depth, "host", attempt);
        let place = level.place.as_ref();
        let host = place.map(|p| &p.host);
        let objective_kind = level.seed.objective_kind;

        let n = host
            .and_then(|h| h.nodes)
            .map_or(usize::from(params(run_danger).nodes), |n| {
                usize::from(n).clamp(3, 8)
            });

        // Graph: a random tree capped at degree 3, plus one extra edge for big hosts.
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for i in 1..n {
            let open: Vec<usize> = (0..i).filter(|&j| adj[j].len() < 3).collect();
            let parent = open
                .choose(&mut rng)
                .copied()
                .unwrap_or_else(|| rng.random_range(0..i));
            adj[i].push(parent);
            adj[parent].push(i);
        }
        if n >= 6 {
            let mut pairs = Vec::new();
            for a in 0..n {
                for b in (a + 1)..n {
                    if adj[a].len() < 3 && adj[b].len() < 3 && !adj[a].contains(&b) {
                        pairs.push((a, b));
                    }
                }
            }
            if let Some(&(a, b)) = pairs.choose(&mut rng) {
                adj[a].push(b);
                adj[b].push(a);
            }
        }

        // Objective: farthest from node 0, ties to the lowest index.
        let dist = bfs(&adj, 0);
        let mut objective_node = 1;
        for i in 1..n {
            if dist[i] > dist[objective_node] {
                objective_node = i;
            }
        }

        // Objective node kind and guard.
        let override_ice = host.and_then(|h| h.objective_ice);
        let ice = match override_ice {
            Some(ObjectiveIce::Breach) => IceKindAt::Breach,
            Some(ObjectiveIce::Black) => IceKindAt::Black,
            Some(ObjectiveIce::Shadow) => IceKindAt::Shadow,
            None => match objective_kind {
                ObjectiveKind::Retrieve if run_danger >= 3 => IceKindAt::Black,
                ObjectiveKind::Retrieve | ObjectiveKind::Plant => IceKindAt::Breach,
                ObjectiveKind::Crash if run_danger >= 2 => IceKindAt::Black,
                ObjectiveKind::Crash => IceKindAt::Breach,
            },
        };
        let objective_kind_node = match objective_kind {
            ObjectiveKind::Retrieve if ice == IceKindAt::Breach => NodeKind::Archive,
            ObjectiveKind::Retrieve => NodeKind::Vault,
            ObjectiveKind::Plant | ObjectiveKind::Crash => NodeKind::Core,
        };

        let mut kinds = vec![NodeKind::Portal; n];
        let mut ices: Vec<Option<IceKindAt>> = vec![None; n];
        kinds[0] = NodeKind::Portal;
        ices[0] = Some(IceKindAt::Wordlock);
        kinds[objective_node] = objective_kind_node;
        ices[objective_node] = Some(ice);
        let mut rest: Vec<usize> = (1..n).filter(|&i| i != objective_node).collect();
        rest.shuffle(&mut rng);
        for (k, &i) in rest.iter().enumerate() {
            let mut kind = KIND_CYCLE[k % KIND_CYCLE.len()];
            if kind == NodeKind::Relay && run_danger == 1 {
                kind = NodeKind::Gate;
            }
            kinds[i] = kind;
            ices[i] = match kind {
                NodeKind::Gate => Some(IceKindAt::Breach),
                NodeKind::Relay => Some(IceKindAt::Phreak),
                _ => None,
            };
        }

        // Mirror leaf on a mirror exit.
        let mut nodes_total = n;
        if exit_kind == Some(ExitKind::Mirror) {
            let open: Vec<usize> = (0..n)
                .filter(|&i| i != objective_node && adj[i].len() < 3)
                .collect();
            let parent = open.choose(&mut rng).copied().unwrap_or(0);
            adj.push(vec![parent]);
            adj[parent].push(n);
            kinds.push(NodeKind::Mirror);
            ices.push(Some(IceKindAt::Shadow));
            nodes_total += 1;
        }

        // Per-instance grades and black ICE classes, drawn from their own stream so the graph,
        // files and objective draws above are untouched. Two in five BREACH gates become a
        // DAEMON console instead.
        let mut grng = substream(seed, level.seed.depth, "ice-grade", attempt);
        for (i, ice) in ices.iter_mut().enumerate() {
            if kinds[i] == NodeKind::Gate
                && *ice == Some(IceKindAt::Breach)
                && grng.random_range(0..100u32) < 40
            {
                *ice = Some(IceKindAt::Daemon);
            }
        }
        let mut grades: Vec<IceGrade> = vec![IceGrade::Standard; nodes_total];
        let mut classes: Vec<Option<BlackClass>> = vec![None; nodes_total];
        for i in 0..nodes_total {
            match ices[i] {
                Some(IceKindAt::Shadow) => {}
                Some(_) => grades[i] = roll_grade(run_danger, &mut grng),
                None => {}
            }
            if ices[i] == Some(IceKindAt::Black) {
                classes[i] = Some(BlackClass::ALL[grng.random_range(0..3)]);
            }
        }

        // Display names.
        let names: &[String] = host.map_or(&[], |h| h.node_names.as_slice());
        let mut nodes: Vec<Node> = (0..nodes_total)
            .map(|i| {
                let id = format!("{}{}", kinds[i].label(), i);
                let display = if kinds[i] == NodeKind::Mirror {
                    "MIRROR".to_string()
                } else {
                    names
                        .get(i)
                        .filter(|s| !s.trim().is_empty())
                        .cloned()
                        .unwrap_or_else(|| id.to_uppercase())
                };
                Node {
                    id,
                    display,
                    kind: kinds[i],
                    ice: ices[i],
                    grade: grades[i],
                    class: classes[i],
                    cleared: false,
                    files: Vec::new(),
                    neighbors: adj[i].clone(),
                }
            })
            .collect();

        // Files.
        let host_target = host.map(|h| h.target_file.trim()).unwrap_or("");
        let payload_name = match objective_kind {
            ObjectiveKind::Plant if host_target.is_empty() => "PAYLOAD.PIF".to_string(),
            ObjectiveKind::Crash => String::new(),
            _ => host_target.to_string(),
        };
        let authored: &[crate::pack::model::FileDef] =
            place.map_or(&[], |p| p.host_files.as_slice());
        if objective_kind == ObjectiveKind::Retrieve && !payload_name.is_empty() {
            let found = authored
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(&payload_name));
            let (description, body) = match found {
                Some(f) => (f.description.clone(), f.body.clone()),
                None => (
                    "encrypted payload".to_string(),
                    format!("{payload_name}: encrypted payload. Use get to retrieve it."),
                ),
            };
            nodes[objective_node].files.push(HostFile {
                name: payload_name.clone(),
                description,
                body,
                reward_kb: 0,
                fetched: false,
                is_target: true,
            });
        }
        let mut homes: Vec<usize> = (0..n)
            .filter(|&i| kinds[i] == NodeKind::Archive && i != objective_node)
            .collect();
        if homes.is_empty() {
            homes = (1..n).filter(|&i| i != objective_node).collect();
        }
        if homes.is_empty() {
            homes.push(0);
        }
        let mut k = 0;
        for f in authored {
            if objective_kind == ObjectiveKind::Retrieve
                && f.name.eq_ignore_ascii_case(&payload_name)
            {
                continue;
            }
            nodes[homes[k % homes.len()]].files.push(HostFile {
                name: f.name.clone(),
                description: f.description.clone(),
                body: f.body.clone(),
                reward_kb: FILE_REWARD_KB,
                fetched: false,
                is_target: false,
            });
            k += 1;
        }

        // Crash: four processes, one of them the target.
        let target_daemon = level
            .seed
            .target_daemon
            .clone()
            .or_else(|| daemons.first().cloned())
            .unwrap_or_else(|| "daemon".to_string())
            .to_lowercase();
        let processes = if objective_kind == ObjectiveKind::Crash {
            build_processes(&mut rng, &target_daemon, daemons)
        } else {
            Vec::new()
        };

        HostSession {
            os: host.map_or(HostOs::Unix, |h| h.os),
            host_name: host.map_or_else(|| "UNKNOWN HOST".to_string(), |h| h.name.clone()),
            banner: host.map(|h| h.banner.clone()).unwrap_or_default(),
            objective_text: host.map(|h| h.objective.clone()).unwrap_or_default(),
            objective_kind,
            objective_node,
            objective: ObjectiveState::Pending,
            target_file: payload_name,
            target_daemon,
            processes,
            nodes,
            current: 0,
            run_danger,
            exit_kind,
            attempt,
            trace: 0.0,
            ghost: false,
            bounce_used: false,
            dumped: false,
            whoami_seen: false,
            player: PlayerView::default(),
        }
    }

    // ----- accessors -------------------------------------------------------------------

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn current(&self) -> usize {
        self.current
    }

    pub fn current_node(&self) -> &Node {
        &self.nodes[self.current]
    }

    pub fn objective_kind(&self) -> ObjectiveKind {
        self.objective_kind
    }

    /// Index of the node that holds the objective.
    pub fn objective_node(&self) -> usize {
        self.objective_node
    }

    pub fn objective_state(&self) -> ObjectiveState {
        self.objective
    }

    pub fn objective_done(&self) -> bool {
        self.objective == ObjectiveState::Done
    }

    /// The authored/AI sentence naming the job.
    pub fn objective_text(&self) -> &str {
        &self.objective_text
    }

    /// Retrieve: the file to fetch; plant: the payload to `put`; crash: empty.
    pub fn target_file(&self) -> &str {
        &self.target_file
    }

    /// Lowercase target process name (crash objectives).
    pub fn target_daemon(&self) -> &str {
        &self.target_daemon
    }

    /// Processes still listed by `ps` (crash objectives).
    pub fn processes(&self) -> &[Process] {
        &self.processes
    }

    pub fn os(&self) -> HostOs {
        self.os
    }

    pub fn host_name(&self) -> &str {
        &self.host_name
    }

    pub fn banner(&self) -> &str {
        &self.banner
    }

    pub fn run_danger(&self) -> u8 {
        self.run_danger
    }

    pub fn exit_kind(&self) -> Option<ExitKind> {
        self.exit_kind
    }

    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// Trace, 0..=100.
    pub fn trace(&self) -> f32 {
        self.trace
    }

    pub fn ghost_active(&self) -> bool {
        self.ghost
    }

    pub fn bounce_used(&self) -> bool {
        self.bounce_used
    }

    /// True once DUMP SHOCK has fired; the session accepts no further input.
    pub fn is_dumped(&self) -> bool {
        self.dumped
    }

    pub fn whoami_used(&self) -> bool {
        self.whoami_seen
    }

    pub fn player(&self) -> &PlayerView {
        &self.player
    }

    /// True until the portal WORDLOCK has been cleared (`apply_ice_outcome(0, Cleared)`).
    pub fn needs_login(&self) -> bool {
        self.nodes.first().is_some_and(Node::guarded)
    }

    pub fn set_player(&mut self, view: PlayerView) {
        self.player = view;
    }

    /// The shell's copy of the deck (the UI keeps the saved deck in sync via
    /// `ShellEffect::UseProgram`).
    pub fn deck(&self) -> &Deck {
        &self.player.deck
    }

    /// Prompt text and colour for the current node. Mainframes show `prompt_header()` on its own
    /// line first and an empty prompt.
    pub fn prompt(&self) -> (String, PromptColor) {
        let node = &self.current_node().id;
        match self.os {
            HostOs::Unix => (format!("{node}% "), PromptColor::LightGreen),
            HostOs::Vax => ("$ ".to_string(), PromptColor::Yellow),
            HostOs::Mainframe => (String::new(), PromptColor::WhiteOnBlue),
            HostOs::Dos => (
                format!("C:\\{}>", node.to_uppercase()),
                PromptColor::LightGray,
            ),
        }
    }

    /// Line printed before the input line (`READY` on mainframes).
    pub fn prompt_header(&self) -> Option<&'static str> {
        (self.os == HostOs::Mainframe).then_some("READY")
    }

    /// Edges as `(a, b)` with `a < b`.
    pub fn edges(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (a, node) in self.nodes.iter().enumerate() {
            for &b in &node.neighbors {
                if a < b {
                    out.push((a, b));
                }
            }
        }
        out
    }

    /// Layout data for the `scan` map: column = `layer * 14`, row = `row * 3`.
    pub fn map_layout(&self) -> Vec<MapNode> {
        let adj: Vec<Vec<usize>> = self.nodes.iter().map(|n| n.neighbors.clone()).collect();
        let dist = bfs(&adj, 0);
        let mut counts: Vec<usize> = Vec::new();
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| {
                let layer = if dist[i] == usize::MAX { 0 } else { dist[i] };
                if counts.len() <= layer {
                    counts.resize(layer + 1, 0);
                }
                let row = counts[layer];
                counts[layer] += 1;
                MapNode {
                    index: i,
                    id: n.id.clone(),
                    display: n.display.clone(),
                    kind: n.kind,
                    ice: n.ice,
                    cleared: n.cleared,
                    is_current: i == self.current,
                    layer,
                    row,
                    neighbors: n.neighbors.clone(),
                }
            })
            .collect()
    }

    /// Resolve a node by command id or display name (case-insensitive).
    pub fn find_node(&self, name: &str) -> Option<usize> {
        let name = name.trim();
        self.nodes
            .iter()
            .position(|n| n.id.eq_ignore_ascii_case(name))
            .or_else(|| {
                self.nodes
                    .iter()
                    .position(|n| n.display.eq_ignore_ascii_case(name))
            })
    }

    pub(crate) fn find_file(&self, node: usize, name: &str) -> Option<usize> {
        self.nodes[node]
            .files
            .iter()
            .position(|f| f.name.eq_ignore_ascii_case(name.trim()))
    }

    // ----- trace and time -------------------------------------------------------------

    /// Time-based trace per second at the current settings.
    fn time_rate(&self, in_minigame: bool) -> f32 {
        let mut rate = params(self.run_danger).trace_per_10s / 10.0;
        if in_minigame {
            rate *= 0.5;
        }
        if self.ghost {
            rate *= 0.6;
        }
        rate
    }

    /// Advance the trace clock. Returns DUMP SHOCK effects when it reaches 100.
    pub fn tick(&mut self, dt: Duration, in_minigame: bool) -> Vec<ShellEffect> {
        let mut out = Vec::new();
        if self.dumped {
            return out;
        }
        let delta = self.time_rate(in_minigame) * dt.as_secs_f32();
        self.bump(delta, false, &mut out);
        out
    }

    /// Add (or, when negative, remove) trace; emits DUMP SHOCK once at 100.
    /// Returns false when the session has dumped.
    pub(crate) fn bump(&mut self, delta: f32, report: bool, out: &mut Vec<ShellEffect>) -> bool {
        if self.dumped {
            return false;
        }
        let before = self.trace;
        self.trace = (self.trace + delta).clamp(0.0, 100.0);
        if report && (self.trace - before).abs() > f32::EPSILON {
            out.push(ShellEffect::TraceDelta(self.trace - before));
        }
        if self.trace >= 100.0 {
            self.dumped = true;
            out.push(ShellEffect::EgoDelta(DUMP_SHOCK_EGO));
            out.push(ShellEffect::DumpShock);
            return false;
        }
        true
    }

    /// Apply the result of the ICE mini-game guarding `node`. A cleared node becomes the current
    /// node (the pending `connect` completes).
    pub fn apply_ice_outcome(&mut self, node: usize, outcome: IceOutcome) -> Vec<ShellEffect> {
        let mut out = Vec::new();
        if node >= self.nodes.len() || self.dumped {
            return out;
        }
        match outcome {
            IceOutcome::Cleared => self.enter(node, &mut out),
            IceOutcome::Integrated => {
                out.push(ShellEffect::IntegrationDelta(2));
                self.enter(node, &mut out);
            }
            IceOutcome::Destroyed => {
                out.push(ShellEffect::IntegrationDelta(-1));
                out.push(ShellEffect::line("you cut it away. it grows back."));
                self.enter(node, &mut out);
            }
            IceOutcome::Failed { trace, ego } => {
                if ego != 0 {
                    out.push(ShellEffect::EgoDelta(-ego.abs()));
                }
                out.push(ShellEffect::dim("ICE holds."));
                self.bump(trace.max(0.0), true, &mut out);
            }
            IceOutcome::Aborted { trace } => {
                self.bump(trace.max(0.0), true, &mut out);
            }
            IceOutcome::Retreated => {
                out.push(ShellEffect::dim("you fall back. the ICE stays."));
                self.bump(RETREAT_TRACE, true, &mut out);
            }
        }
        out
    }

    fn enter(&mut self, node: usize, out: &mut Vec<ShellEffect>) {
        self.nodes[node].cleared = true;
        self.current = node;
        out.push(ShellEffect::Moved { node });
        out.push(ShellEffect::line(format!(
            "access granted: {} [{}]",
            self.nodes[node].id, self.nodes[node].display
        )));
    }

    /// Apply the result of the ROUTER `bounce` puzzle. `Cleared` removes 30 trace; `Failed` adds
    /// its trace (timeout +10); both consume the bounce. `Aborted` does not.
    pub fn apply_bounce_outcome(&mut self, outcome: IceOutcome) -> Vec<ShellEffect> {
        let mut out = Vec::new();
        if self.dumped {
            return out;
        }
        match outcome {
            IceOutcome::Cleared => {
                self.bounce_used = true;
                out.push(ShellEffect::line("route bounced. the trace loses you."));
                self.bump(-BOUNCE_TRACE_RELIEF, true, &mut out);
            }
            IceOutcome::Failed { trace, .. } => {
                self.bounce_used = true;
                out.push(ShellEffect::error("bounce failed. the line holds."));
                self.bump(trace.max(0.0), true, &mut out);
            }
            _ => out.push(ShellEffect::dim("bounce cancelled.")),
        }
        out
    }
}

fn bfs(adj: &[Vec<usize>], from: usize) -> Vec<usize> {
    let mut dist = vec![usize::MAX; adj.len()];
    let mut queue = VecDeque::new();
    dist[from] = 0;
    queue.push_back(from);
    while let Some(u) = queue.pop_front() {
        for &v in &adj[u] {
            if dist[v] == usize::MAX {
                dist[v] = dist[u] + 1;
                queue.push_back(v);
            }
        }
    }
    dist
}

fn build_processes(rng: &mut Rng, target: &str, daemons: &[String]) -> Vec<Process> {
    let mut decoys: Vec<String> = Vec::new();
    let mut pool: Vec<String> = daemons
        .iter()
        .map(|d| d.trim().to_lowercase())
        .filter(|d| !d.is_empty() && d != target)
        .collect();
    pool.sort();
    pool.dedup();
    pool.shuffle(rng);
    decoys.extend(pool.into_iter().take(3));
    for f in FALLBACK_DAEMONS {
        if decoys.len() >= 3 {
            break;
        }
        if f != target && !decoys.iter().any(|d| d == f) {
            decoys.push(f.to_string());
        }
    }
    let mut names: Vec<(String, bool)> = decoys.into_iter().map(|d| (d, false)).collect();
    names.push((target.to_string(), true));
    let mut pids: Vec<u32> = Vec::new();
    while pids.len() < names.len() {
        let pid = rng.random_range(100..1000);
        if !pids.contains(&pid) {
            pids.push(pid);
        }
    }
    pids.sort_unstable();
    names.shuffle(rng);
    pids.into_iter()
        .zip(names)
        .map(|(pid, (name, is_target))| Process {
            pid,
            name,
            is_target,
        })
        .collect()
}
