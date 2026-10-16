//! Content rules for story packs (`abraxas pack check`). `load` only parses; everything an
//! author can get wrong beyond TOML syntax is reported here.

use std::collections::BTreeMap;

use super::PackIssue;
use super::model::{
    ActDef, Anchor, AnchorEntry, Archetype, Character, ChoiceDef, ExitDef, ExitKind, FileDef,
    ObjectiveKind, PostDef, Stance, StoryPack,
};
use crate::game::danger;
use crate::text::{
    check_placeholders, contains_words, is_83_name, is_cp437, pipe_size, strip_pipe_codes,
};

/// Placeholders allowed in every authored text.
const BASE: [&str; 3] = ["handle", "shadow", "real_name"];
/// Pool level cards may also use `{motif}`.
const LEVEL_CARD: [&str; 4] = ["handle", "shadow", "real_name", "motif"];
/// Pool interludes may also use `{exit}`.
const INTERLUDE: [&str; 4] = ["handle", "shadow", "real_name", "exit"];

const ANY: usize = usize::MAX;
const ART_MAX: (usize, usize) = (36, 16);
const LOGO_MAX: (usize, usize) = (78, 8);
const LOGO: &str = "abraxas-logo.txt";

/// Every content-rule violation in `pack`. Empty when the pack is valid.
pub fn validate(pack: &StoryPack) -> Vec<PackIssue> {
    let mut v = Validator { issues: Vec::new() };
    v.meta(pack);
    v.acts(pack);
    v.cast(pack);
    v.canon(pack);
    v.anchors(pack);
    v.pool(pack);
    v.art(pack);
    v.issues
}

struct Validator {
    issues: Vec<PackIssue>,
}

fn is_id(s: &str, max: usize, dash: bool) -> bool {
    (1..=max).contains(&s.len())
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' && !dash || b == b'-' && dash
        })
}

/// `^[A-Za-z0-9_-]{2,16}$`
fn is_handle(s: &str) -> bool {
    (2..=16).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// `^[A-Z]{len}$` when `len` is `Some`, else any non-empty `^[A-Z]+$`.
fn is_caps_word(s: &str, len: Option<usize>) -> bool {
    !s.is_empty() && len.is_none_or(|l| s.len() == l) && s.bytes().all(|b| b.is_ascii_uppercase())
}

/// `^[a-z][a-z0-9_]{0,31}$`
fn is_flag(s: &str) -> bool {
    s.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn words(s: &str) -> usize {
    s.split_whitespace().count()
}

fn parse_clock(s: &str) -> bool {
    let parts: Vec<&str> = s.split(':').collect();
    let [h, m, sec] = parts.as_slice() else {
        return false;
    };
    let num = |p: &str, max: u32| {
        p.len() == 2
            && p.bytes().all(|b| b.is_ascii_digit())
            && p.parse::<u32>().is_ok_and(|n| n <= max)
    };
    num(h, 23) && num(m, 59) && num(sec, 59)
}

impl Validator {
    fn add(&mut self, file: &str, field: &str, message: impl Into<String>) {
        self.issues.push(PackIssue::new(file, field, message));
    }

    /// CP437 check, optional length limit (characters after stripping pipe codes) and
    /// placeholder check.
    fn text(&mut self, file: &str, field: &str, s: &str, max: usize, allowed: Option<&[&str]>) {
        if let Some(c) = s.chars().find(|&c| !is_cp437(c)) {
            self.add(
                file,
                field,
                format!("character {c:?} (U+{:04X}) is outside CP437", u32::from(c)),
            );
        }
        if max != ANY {
            let n = strip_pipe_codes(s).chars().count();
            if n > max {
                self.add(file, field, format!("{n} characters; the limit is {max}"));
            }
        }
        if let Some(allowed) = allowed
            && let Err(e) = check_placeholders(s, allowed)
        {
            self.add(file, field, e);
        }
    }

    fn opt_text(
        &mut self,
        file: &str,
        field: &str,
        s: Option<&String>,
        max: usize,
        allowed: Option<&[&str]>,
    ) {
        if let Some(s) = s {
            self.text(file, field, s, max, allowed);
        }
    }

    fn post(&mut self, file: &str, field: &str, p: &PostDef, allowed: &[&str]) {
        self.text(file, &format!("{field}.from"), &p.from, 16, Some(allowed));
        self.text(
            file,
            &format!("{field}.subject"),
            &p.subject,
            40,
            Some(allowed),
        );
        self.text(file, &format!("{field}.body"), &p.body, 600, Some(allowed));
        if p.body.trim().is_empty() {
            self.add(file, &format!("{field}.body"), "must not be empty");
        }
    }

    fn file_def(&mut self, file: &str, field: &str, f: &FileDef, allowed: &[&str]) {
        if !is_83_name(&f.name) {
            self.add(
                file,
                &format!("{field}.name"),
                format!(
                    "'{}' is not an 8.3 name ([A-Z0-9_-]{{1,8}}.[A-Z0-9]{{1,3}})",
                    f.name
                ),
            );
        }
        self.text(
            file,
            &format!("{field}.description"),
            &f.description,
            40,
            Some(allowed),
        );
        self.text(file, &format!("{field}.body"), &f.body, 900, Some(allowed));
    }

    fn exit_def(&mut self, file: &str, field: &str, e: &ExitDef, allowed: &[&str]) {
        self.text(file, &format!("{field}.text"), &e.text, 60, Some(allowed));
    }

    fn choice(&mut self, file: &str, field: &str, c: &ChoiceDef, allowed: &[&str]) {
        self.text(
            file,
            &format!("{field}.prompt"),
            &c.prompt,
            160,
            Some(allowed),
        );
        for (i, o) in c.options.iter().enumerate() {
            let f = format!("{field}.option[{}]", i + 1);
            self.text(file, &format!("{f}.text"), &o.text, 70, Some(allowed));
            if o.flags.len() > 2 {
                self.add(file, &format!("{f}.flags"), "at most 2 flags");
            }
            for flag in &o.flags {
                if !is_flag(flag) {
                    self.add(
                        file,
                        &format!("{f}.flags"),
                        format!("'{flag}' is not snake_case (^[a-z][a-z0-9_]{{0,31}}$)"),
                    );
                }
            }
        }
    }

    // ---- pack.toml -----------------------------------------------------------------------

    fn meta(&mut self, pack: &StoryPack) {
        const F: &str = "pack.toml";
        let m = &pack.meta;
        if !is_id(&m.id, 32, true) {
            self.add(
                F,
                "id",
                format!("'{}' does not match ^[a-z0-9-]{{1,32}}$", m.id),
            );
        }
        if m.title.trim().is_empty() {
            self.add(F, "title", "must not be empty");
        }
        self.text(F, "title", &m.title, ANY, None);
        self.text(F, "author", &m.author, ANY, None);
        self.text(F, "style", &m.style, ANY, None);
        if words(&m.style) > 120 {
            self.add(
                F,
                "style",
                format!("{} words; the limit is 120", words(&m.style)),
            );
        }
        if m.motifs.len() < 8 {
            self.add(
                F,
                "motifs",
                format!("{} entries; at least 8 are required", m.motifs.len()),
            );
        }
        for (i, s) in m.motifs.iter().enumerate() {
            self.text(F, &format!("motifs[{}]", i + 1), s, ANY, None);
        }
        if m.room_kinds.len() < 8 {
            self.add(
                F,
                "room_kinds",
                format!("{} entries; at least 8 are required", m.room_kinds.len()),
            );
        }
        for (i, s) in m.room_kinds.iter().enumerate() {
            self.text(F, &format!("room_kinds[{}]", i + 1), s, ANY, None);
        }
        if !parse_clock(&m.start_clock) {
            self.add(
                F,
                "start_clock",
                format!("'{}' is not HH:MM:SS", m.start_clock),
            );
        }
        if m.minutes_per_level < 1 {
            self.add(F, "minutes_per_level", "must be at least 1");
        }
    }

    fn acts(&mut self, pack: &StoryPack) {
        const F: &str = "pack.toml";
        let acts: &[ActDef] = &pack.meta.act;
        if acts.is_empty() {
            self.add(F, "act", "at least one [[act]] is required");
        }
        let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
        let mut expected = 0u32;
        for (i, a) in acts.iter().enumerate() {
            let field = format!("act[{}]", i + 1);
            if !is_id(&a.id, 24, false) {
                self.add(
                    F,
                    &format!("{field}.id"),
                    format!("'{}' does not match ^[a-z0-9_]{{1,24}}$", a.id),
                );
            }
            if let Some(first) = ids.insert(&a.id, i + 1) {
                self.add(
                    F,
                    &format!("{field}.id"),
                    format!("'{}' already used by act {first}", a.id),
                );
            }
            let [from, to] = a.depths;
            if from > to {
                self.add(
                    F,
                    &format!("{field}.depths"),
                    format!("from {from} is after to {to}"),
                );
            } else if from > expected {
                self.add(
                    F,
                    &format!("{field}.depths"),
                    format!("gap: depths {expected}..{} belong to no act", from - 1),
                );
            } else if from < expected {
                self.add(
                    F,
                    &format!("{field}.depths"),
                    format!("overlaps the previous act (expected to start at depth {expected})"),
                );
            }
            expected = expected.max(to.saturating_add(1));
            self.text(F, &format!("{field}.name"), &a.name, ANY, None);
            if a.name.trim().is_empty() {
                self.add(F, &format!("{field}.name"), "must not be empty");
            }
            self.text(F, &format!("{field}.goal"), &a.goal, 160, None);
            self.text(F, &format!("{field}.direction"), &a.direction, ANY, None);
            if words(&a.direction) > 120 {
                self.add(
                    F,
                    &format!("{field}.direction"),
                    format!("{} words; the limit is 120", words(&a.direction)),
                );
            }
            if a.default_vision == super::model::VisionKind::Noise {
                self.add(
                    F,
                    &format!("{field}.default_vision"),
                    "noise cannot be an act's default vision",
                );
            }
        }
        if let Some(e) = pack.meta.endless.as_ref().filter(|e| e.enabled) {
            for (field, value) in [
                ("name", &e.name),
                ("goal", &e.goal),
                ("direction", &e.direction),
            ] {
                if value.trim().is_empty() {
                    self.add(
                        F,
                        &format!("endless.{field}"),
                        "required when endless is enabled",
                    );
                }
            }
            self.text(F, "endless.name", &e.name, ANY, None);
            self.text(F, "endless.goal", &e.goal, ANY, None);
            self.text(F, "endless.direction", &e.direction, ANY, None);
            if e.default_vision == super::model::VisionKind::Noise {
                self.add(
                    F,
                    "endless.default_vision",
                    "noise cannot be the default vision",
                );
            }
        }
    }

    // ---- cast.toml -----------------------------------------------------------------------

    fn cast(&mut self, pack: &StoryPack) {
        const F: &str = "cast.toml";
        let mut ids: BTreeMap<&str, usize> = BTreeMap::new();
        let mut shadows = 0;
        for (i, c) in pack.cast.iter().enumerate() {
            let field = format!("character[{}]", i + 1);
            self.character(pack, c, &field);
            if let Some(first) = ids.insert(&c.id, i + 1) {
                self.add(
                    F,
                    &format!("{field}.id"),
                    format!("'{}' already used by character {first}", c.id),
                );
            }
            if c.archetype == Archetype::Shadow {
                shadows += 1;
                if shadows == 2 {
                    self.add(
                        F,
                        &format!("{field}.archetype"),
                        "at most one shadow character is allowed",
                    );
                }
            }
        }
    }

    fn character(&mut self, pack: &StoryPack, c: &Character, field: &str) {
        const F: &str = "cast.toml";
        if !is_id(&c.id, 24, false) {
            self.add(
                F,
                &format!("{field}.id"),
                format!("'{}' does not match ^[a-z0-9_]{{1,24}}$", c.id),
            );
        }
        if c.handle.contains("{shadow}")
            && (c.archetype != Archetype::Shadow || c.handle != "{shadow}")
        {
            self.add(
                F,
                &format!("{field}.handle"),
                "{shadow} is only allowed as the whole handle of the shadow archetype",
            );
        } else if c.handle != "{shadow}" {
            self.text(F, &format!("{field}.handle"), &c.handle, 16, Some(&[]));
        }
        if c.handle.trim().is_empty() {
            self.add(F, &format!("{field}.handle"), "must not be empty");
        }
        self.text(
            F,
            &format!("{field}.description"),
            &c.description,
            ANY,
            Some(&BASE),
        );
        if words(&c.description) > 60 {
            self.add(
                F,
                &format!("{field}.description"),
                format!("{} words; the limit is 60", words(&c.description)),
            );
        }
        self.text(F, &format!("{field}.voice"), &c.voice, ANY, Some(&BASE));
        if words(&c.voice) > 40 {
            self.add(
                F,
                &format!("{field}.voice"),
                format!("{} words; the limit is 40", words(&c.voice)),
            );
        }
        if let Some(art) = &c.art
            && !pack.art.contains_key(art)
        {
            self.add(
                F,
                &format!("{field}.art"),
                format!("art/{art} does not exist"),
            );
        }
        if c.fallback.len() < 3 {
            self.add(
                F,
                &format!("{field}.fallback"),
                format!("{} lines; at least 3 are required", c.fallback.len()),
            );
        }
        for (i, line) in c.fallback.iter().enumerate() {
            self.text(
                F,
                &format!("{field}.fallback[{}]", i + 1),
                line,
                ANY,
                Some(&BASE),
            );
        }
        for (i, g) in c.eliza.iter().enumerate() {
            let gf = format!("{field}.eliza[{}]", i + 1);
            if g.keywords.is_empty() {
                self.add(F, &format!("{gf}.keywords"), "must not be empty");
            }
            if g.replies.is_empty() {
                self.add(F, &format!("{gf}.replies"), "must not be empty");
            }
            for r in &g.replies {
                self.text(F, &format!("{gf}.replies"), r, ANY, Some(&BASE));
            }
            for k in &g.keywords {
                self.text(F, &format!("{gf}.keywords"), k, ANY, None);
            }
        }
    }

    fn canon(&mut self, pack: &StoryPack) {
        const F: &str = "canon.md";
        self.text(F, "text", &pack.canon, ANY, None);
        let n = words(&pack.canon);
        if n > 1500 {
            self.add(F, "text", format!("{n} words; the limit is 1500"));
        }
    }

    // ---- anchors -------------------------------------------------------------------------

    fn anchors(&mut self, pack: &StoryPack) {
        let mut by_depth: BTreeMap<u32, &str> = BTreeMap::new();
        for entry in &pack.anchors {
            let file = entry.file.as_str();
            let d = entry.anchor.depth;
            if let Some(other) = by_depth.get(&d) {
                self.add(file, "depth", format!("depth {d} already used by {other}"));
            } else {
                by_depth.insert(d, file);
            }
            if pack.act_for_depth(d).is_none() {
                self.add(
                    file,
                    "depth",
                    format!(
                        "depth {d} is outside the acts (last depth {})",
                        pack.last_depth()
                    ),
                );
            }
            self.anchor(pack, entry);
        }
    }

    fn anchor(&mut self, pack: &StoryPack, entry: &AnchorEntry) {
        let file = entry.file.as_str();
        let a: &Anchor = &entry.anchor;
        let t = Some(&BASE[..]);
        self.opt_text(file, "title", a.title.as_ref(), 40, t);
        self.opt_text(file, "bbs_name", a.bbs_name.as_ref(), 24, t);
        self.opt_text(file, "tagline", a.tagline.as_ref(), 60, t);
        self.opt_text(file, "level_card", a.level_card.as_ref(), 320, t);
        self.opt_text(file, "beat", a.beat.as_ref(), 700, t);
        self.opt_text(file, "required", a.required.as_ref(), ANY, t);
        for (i, k) in a.keep.iter().enumerate() {
            let field = format!("keep[{}]", i + 1);
            self.text(file, &field, k, 200, t);
            if !k.chars().any(char::is_alphanumeric) {
                self.add(file, &field, "has no words");
            } else if !a.beat.as_deref().is_some_and(|b| contains_words(b, k)) {
                self.add(file, &field, format!("'{k}' is not in beat"));
            }
        }
        self.opt_text(file, "interlude", a.interlude.as_ref(), ANY, t);
        self.opt_text(file, "summary", a.summary.as_ref(), 200, t);

        if let Some(s) = &a.sysop
            && pack.character(s).is_none()
            && !is_handle(s)
        {
            self.add(
                file,
                "sysop",
                format!("'{s}' is neither a cast id nor a handle matching ^[A-Za-z0-9_-]{{2,16}}$"),
            );
        }
        if let Some(f) = &a.focus
            && pack.character(f).is_none()
        {
            self.add(file, "focus", format!("'{f}' is not a cast id"));
        }
        if let Some(d) = a.danger
            && !(1..=5).contains(&d)
        {
            self.add(file, "danger", format!("{d} is outside 1..5"));
        }
        let danger = a
            .danger
            .filter(|d| (1..=5).contains(d))
            .unwrap_or_else(|| danger::base_danger(a.depth));
        let wl = danger::params(danger).wordlock_len;
        if let Some(p) = &a.password
            && !is_caps_word(p, Some(wl))
        {
            self.add(
                file,
                "password",
                format!("'{p}' must be {wl} letters A-Z (WORDLOCK length for danger {danger})"),
            );
        }
        for w in &a.words {
            if !is_caps_word(w, Some(wl)) {
                self.add(
                    file,
                    "words",
                    format!("'{w}' must be {wl} letters A-Z (WORDLOCK length for danger {danger})"),
                );
            }
        }
        if a.captions.len() > 3 {
            self.add(
                file,
                "captions",
                format!("{} captions; at most 3", a.captions.len()),
            );
        }
        for (i, c) in a.captions.iter().enumerate() {
            self.text(file, &format!("captions[{}]", i + 1), c, 48, t);
        }
        if let Some(h) = &a.host {
            self.text_opt_host(file, h, &a.depth);
        }
        for (i, p) in a.post.iter().enumerate() {
            self.post(file, &format!("post[{}]", i + 1), p, &BASE);
        }
        for (i, f) in a.text_file.iter().enumerate() {
            self.file_def(file, &format!("text_file[{}]", i + 1), f, &BASE);
        }
        for (i, f) in a.host_file.iter().enumerate() {
            self.file_def(file, &format!("host_file[{}]", i + 1), f, &BASE);
        }
        if let Some(c) = &a.choice {
            self.choice(file, "choice", c, &BASE);
            if !(2..=3).contains(&c.options.len()) {
                self.add(
                    file,
                    "choice",
                    format!("{} options; 2 or 3 are required", c.options.len()),
                );
            }
        }
        self.exits(file, "exit", &a.exit, &BASE);
        for (i, e) in a.ending.iter().enumerate() {
            let f = format!("ending[{}]", i + 1);
            self.text(file, &format!("{f}.title"), &e.title, 40, t);
            self.text(file, &format!("{f}.text"), &e.text, ANY, t);
            if e.id.is_empty() {
                self.add(file, &format!("{f}.id"), "must not be empty");
            }
            if a.ending[..i].iter().any(|o| o.id == e.id) {
                self.add(
                    file,
                    &format!("{f}.id"),
                    format!("'{}' is used by an earlier ending", e.id),
                );
            }
            if let (Some(lo), Some(hi)) = (e.min_integration, e.max_integration)
                && lo > hi
            {
                self.add(
                    file,
                    &format!("{f}.min_integration"),
                    format!("{lo} is above max_integration {hi}"),
                );
            }
        }
        if !a.ending.is_empty()
            && let Some(c) = &a.choice
        {
            for (i, o) in c.options.iter().enumerate() {
                if !a.ending.iter().any(|e| e.when_stance == o.stance) {
                    self.add(
                        file,
                        "ending",
                        format!("option {} ({}) has no ending", i + 1, o.stance.as_str()),
                    );
                }
            }
        }
    }

    fn text_opt_host(&mut self, file: &str, h: &super::model::AnchorHost, _depth: &u32) {
        let t = Some(&BASE[..]);
        self.opt_text(file, "host.name", h.name.as_ref(), 40, t);
        self.opt_text(file, "host.banner", h.banner.as_ref(), 160, t);
        self.opt_text(file, "host.objective", h.objective.as_ref(), 100, t);
        if let Some(f) = &h.target_file
            && !is_83_name(f)
        {
            self.add(
                file,
                "host.target_file",
                format!("'{f}' is not an 8.3 name"),
            );
        }
        if let Some(n) = h.nodes
            && !(3..=8).contains(&n)
        {
            self.add(file, "host.nodes", format!("{n} is outside 3..8"));
        }
        if let Some(d) = &h.target_daemon {
            if h.objective_kind.is_some_and(|k| k != ObjectiveKind::Crash) {
                self.add(
                    file,
                    "host.target_daemon",
                    "only valid with objective_kind = \"crash\"",
                );
            }
            if d.is_empty()
                || d.chars()
                    .any(|c| c.is_ascii_uppercase() || c.is_whitespace())
            {
                self.add(
                    file,
                    "host.target_daemon",
                    format!("'{d}' must be a lowercase process name"),
                );
            }
            self.text(file, "host.target_daemon", d, ANY, None);
        }
        for (i, n) in h.node_names.iter().enumerate() {
            self.text(file, &format!("host.node_names[{}]", i + 1), n, 24, t);
        }
        for (i, n) in h.ice_names.iter().enumerate() {
            self.text(file, &format!("host.ice_names[{}]", i + 1), n, 20, t);
        }
    }

    /// `exits`, when non-empty, must be exactly calm, deep and mirror once each.
    fn exits(&mut self, file: &str, field: &str, exits: &[ExitDef], allowed: &[&str]) {
        if exits.is_empty() {
            return;
        }
        if exits.len() != 3 {
            self.add(
                file,
                field,
                format!(
                    "{} exits; exactly 3 are required (calm, deep, mirror)",
                    exits.len()
                ),
            );
        }
        for kind in ExitKind::ALL {
            let n = exits.iter().filter(|e| e.kind == kind).count();
            if n != 1 {
                self.add(
                    file,
                    field,
                    format!(
                        "exit kind '{}' appears {n} times; it must appear exactly once",
                        kind.as_str()
                    ),
                );
            }
        }
        for (i, e) in exits.iter().enumerate() {
            self.exit_def(file, &format!("{field}[{}]", i + 1), e, allowed);
        }
    }

    // ---- pool.toml -----------------------------------------------------------------------

    fn pool(&mut self, pack: &StoryPack) {
        const F: &str = "pool.toml";
        let p = &pack.pool;
        let lists: [(&str, usize); 23] = [
            ("titles", p.titles.len()),
            ("bbs_names", p.bbs_names.len()),
            ("taglines", p.taglines.len()),
            ("level_cards", p.level_cards.len()),
            ("sysops", p.sysops.len()),
            ("host_names", p.host_names.len()),
            ("node_names", p.node_names.len()),
            ("ice_names", p.ice_names.len()),
            ("daemons", p.daemons.len()),
            ("interludes", p.interludes.len()),
            ("beats", p.beats.len()),
            ("captions", p.captions.len()),
            ("post", p.post.len()),
            ("text_file", p.text_file.len()),
            ("host_file", p.host_file.len()),
            ("choice", p.choice.len()),
            ("exit", p.exit.len()),
            ("words_5", p.words_5.len()),
            ("words_6", p.words_6.len()),
            ("words_7", p.words_7.len()),
            ("words_8", p.words_8.len()),
            ("eliza", p.eliza.len()),
            ("eliza_fallback", p.eliza_fallback.len()),
        ];
        for (name, n) in lists {
            if n == 0 {
                self.add(F, name, "list must not be empty");
            }
        }
        let t = Some(&BASE[..]);
        let simple = |v: &mut Self, name: &str, items: &[String], max: usize, allowed: &[&str]| {
            for (i, s) in items.iter().enumerate() {
                v.text(F, &format!("{name}[{}]", i + 1), s, max, Some(allowed));
            }
        };
        simple(self, "titles", &p.titles, 40, &BASE);
        simple(self, "bbs_names", &p.bbs_names, 24, &BASE);
        simple(self, "taglines", &p.taglines, 60, &BASE);
        simple(self, "level_cards", &p.level_cards, 320, &LEVEL_CARD);
        simple(self, "host_names", &p.host_names, 40, &BASE);
        simple(self, "node_names", &p.node_names, 24, &BASE);
        simple(self, "ice_names", &p.ice_names, 20, &BASE);
        simple(self, "interludes", &p.interludes, ANY, &INTERLUDE);
        simple(self, "beats", &p.beats, 700, &BASE);
        simple(self, "captions", &p.captions, 48, &BASE);
        simple(self, "eliza_fallback", &p.eliza_fallback, ANY, &BASE);
        for (i, s) in p.sysops.iter().enumerate() {
            if !is_handle(s) {
                self.add(
                    F,
                    &format!("sysops[{}]", i + 1),
                    format!("'{s}' must match ^[A-Za-z0-9_-]{{2,16}}$"),
                );
            }
        }
        for (i, s) in p.daemons.iter().enumerate() {
            if s.is_empty()
                || s.chars()
                    .any(|c| c.is_ascii_uppercase() || c.is_whitespace())
            {
                self.add(
                    F,
                    &format!("daemons[{}]", i + 1),
                    format!("'{s}' must be a lowercase process name"),
                );
            }
            self.text(F, &format!("daemons[{}]", i + 1), s, ANY, None);
        }
        for (i, pd) in p.post.iter().enumerate() {
            self.post(F, &format!("post[{}]", i + 1), pd, &BASE);
        }
        for (i, f) in p.text_file.iter().enumerate() {
            self.file_def(F, &format!("text_file[{}]", i + 1), f, &BASE);
        }
        for (i, f) in p.host_file.iter().enumerate() {
            self.file_def(F, &format!("host_file[{}]", i + 1), f, &BASE);
        }
        for (i, c) in p.choice.iter().enumerate() {
            let f = format!("choice[{}]", i + 1);
            self.choice(F, &f, c, &BASE);
            for s in Stance::ALL {
                let n = c.options.iter().filter(|o| o.stance == s).count();
                if n != 1 {
                    self.add(F, &f, format!("stance '{}' appears {n} times; each choice needs exactly one option per stance", s.as_str()));
                }
            }
        }
        for (i, e) in p.exit.iter().enumerate() {
            self.exit_def(F, &format!("exit[{}]", i + 1), e, &BASE);
        }
        for kind in ExitKind::ALL {
            if p.exit.iter().filter(|e| e.kind == kind).count() == 0 {
                self.add(F, "exit", format!("no exits of kind '{}'", kind.as_str()));
            }
        }
        for (n, list) in [
            (5, &p.words_5),
            (6, &p.words_6),
            (7, &p.words_7),
            (8, &p.words_8),
        ] {
            for (i, w) in list.iter().enumerate() {
                if !is_caps_word(w, Some(n)) {
                    self.add(
                        F,
                        &format!("words_{n}[{}]", i + 1),
                        format!("'{w}' must match ^[A-Z]{{{n}}}$"),
                    );
                }
            }
        }
        for (i, g) in p.eliza.iter().enumerate() {
            let gf = format!("eliza[{}]", i + 1);
            if g.keywords.is_empty() || g.replies.is_empty() {
                self.add(F, &gf, "keywords and replies must not be empty");
            }
            for r in &g.replies {
                self.text(F, &format!("{gf}.replies"), r, ANY, t);
            }
        }
    }

    // ---- art -----------------------------------------------------------------------------

    fn art(&mut self, pack: &StoryPack) {
        for (name, text) in &pack.art {
            let file = format!("art/{name}");
            if let Some(c) = text.chars().find(|&c| !is_cp437(c)) {
                self.add(
                    &file,
                    "art",
                    format!("character {c:?} (U+{:04X}) is outside CP437", u32::from(c)),
                );
            }
            let (cols, rows) = pipe_size(text);
            let (max_cols, max_rows) = if name == LOGO { LOGO_MAX } else { ART_MAX };
            if cols > max_cols || rows > max_rows {
                self.add(
                    &file,
                    "art",
                    format!("{cols}x{rows} exceeds the {max_cols}x{max_rows} limit"),
                );
            }
        }
    }
}
