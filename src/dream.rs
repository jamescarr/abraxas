//! Headless `pack dream`: runs the real director through consecutive levels and prints what it
//! made. Offline runs with a fixed seed print identical output every time.

use std::io::Write;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use rand::RngExt;

use crate::cli::{Choose, GlobalOpts};
use crate::config::{Config, Paths};
use crate::director::{Director, DirectorEvent};
use crate::game::seed;
use crate::game::state::{GenSource, RunState};
use crate::llm::{Llm, OpenAiCompat};
use crate::oracle;
use crate::pack::{self, model::*};
use crate::rng::substream;

/// What the driver can wait for.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Place,
    Passage,
    Interlude,
    Epilogue,
}

impl Part {
    fn name(self) -> &'static str {
        match self {
            Part::Place => "place",
            Part::Passage => "passage",
            Part::Interlude => "interlude",
            Part::Epilogue => "epilogue",
        }
    }
}

struct Dreamer {
    run: RunState,
    pack: Arc<StoryPack>,
    director: Director,
    rx: Receiver<DirectorEvent>,
    /// How long to wait for any one part before giving up.
    timeout: Duration,
    epilogue: Option<(String, GenSource)>,
}

impl Dreamer {
    /// Store a director event into the run, like the game does.
    fn apply(&mut self, ev: DirectorEvent) {
        self.director.on_event(&ev);
        match ev {
            DirectorEvent::PlaceReady {
                depth,
                place,
                source,
            } => {
                if let Some(level) = self.run.levels.get_mut(&depth) {
                    let keys = place.pool_keys();
                    level.place = Some(place);
                    level.place_source = Some(source);
                    self.run.used_pool.extend(keys);
                }
            }
            DirectorEvent::PassageReady {
                depth,
                passage,
                source,
            } => {
                if let Some(level) = self.run.levels.get_mut(&depth) {
                    level.passage = Some(passage);
                    level.passage_source = Some(source);
                }
            }
            DirectorEvent::InterludeDone {
                depth,
                text,
                source,
            } => {
                if let Some(level) = self.run.levels.get_mut(&depth) {
                    level.interlude = Some(text);
                    level.interlude_source = Some(source);
                }
            }
            DirectorEvent::EpilogueDone { text, source } => self.epilogue = Some((text, source)),
            _ => {}
        }
    }

    fn ready(&self, part: Part, depth: u32) -> bool {
        match (part, self.run.levels.get(&depth)) {
            (Part::Epilogue, _) => self.epilogue.is_some(),
            (_, None) => false,
            (Part::Place, Some(l)) => l.place.is_some(),
            (Part::Passage, Some(l)) => l.passage.is_some(),
            (Part::Interlude, Some(l)) => l.interlude.is_some(),
        }
    }

    fn wait(&mut self, part: Part, depth: u32) -> anyhow::Result<()> {
        let deadline = Instant::now() + self.timeout;
        while !self.ready(part, depth) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(ev) => self.apply(ev),
                Err(RecvTimeoutError::Timeout) => {
                    bail!("timed out waiting for the {} of level {depth}", part.name())
                }
                Err(RecvTimeoutError::Disconnected) => {
                    bail!(
                        "the director stopped before the {} of level {depth}",
                        part.name()
                    )
                }
            }
        }
        Ok(())
    }
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn tag(source: Option<&GenSource>) -> String {
    source.map_or_else(|| "missing".to_string(), GenSource::tag)
}

fn objective_word(kind: ObjectiveKind) -> &'static str {
    match kind {
        ObjectiveKind::Retrieve => "retrieve",
        ObjectiveKind::Plant => "plant",
        ObjectiveKind::Crash => "crash",
    }
}

fn os_word(os: HostOs) -> &'static str {
    match os {
        HostOs::Unix => "unix",
        HostOs::Vax => "vax",
        HostOs::Mainframe => "mainframe",
        HostOs::Dos => "dos",
    }
}

/// The level block, from the header through the exits.
fn level_block(d: &Dreamer, depth: u32) -> anyhow::Result<String> {
    let run = &d.run;
    let level = run.levels.get(&depth).context("level missing")?;
    let place = level.place.as_ref().context("place missing")?;
    let passage = level.passage.as_ref().context("passage missing")?;
    let s = &level.seed;
    let act = d.pack.act_for_depth(depth).map_or("", |a| a.name);
    let shadow = run.shadow();
    let mut out = format!(
        "=== LEVEL {depth} │ {act} │ CLASS {} │ omen {} │ motifs: {} ===\n",
        s.base_danger,
        oracle::title(s.omen),
        s.motifs.join("; ")
    );
    out.push_str(&format!(
        "place   [{}] \"{}\" @ {} (sysop: {})\n",
        tag(level.place_source.as_ref()),
        place.title,
        place.bbs_name,
        place.sysop.handle
    ));
    out.push_str(&format!("card    {}\n", one_line(&place.level_card)));
    out.push_str(&format!(
        "host    {} ({}) │ {} {} │ \"{}\"\n",
        place.host.name,
        os_word(place.host.os),
        objective_word(s.objective_kind),
        match (s.objective_kind, &s.target_daemon) {
            (ObjectiveKind::Crash, Some(daemon)) => daemon.as_str(),
            _ => place.host.target_file.as_str(),
        },
        one_line(&place.host.objective)
    ));
    out.push_str(&format!(
        "words   {} × {} letters: {}\n",
        place.words.len(),
        s.wordlock_len,
        place.words.join(", ")
    ));
    let has_shadow = place.posts.iter().any(|p| p.from == shadow);
    let has_clue = place
        .posts
        .iter()
        .any(|p| p.subject.eq_ignore_ascii_case("re: front door"));
    let yes_no = |b: bool| if b { "yes" } else { "no" };
    out.push_str(&format!(
        "board   posts {} (shadow: {}, clue: {}) │ files {} │ host files {}\n",
        place.posts.len(),
        yes_no(has_shadow),
        yes_no(has_clue),
        place.text_files.len(),
        place.host_files.len()
    ));
    out.push_str(&format!(
        "passage [{}]\n",
        tag(level.passage_source.as_ref())
    ));
    out.push_str(&format!("beat    {}\n", one_line(&passage.beat)));
    let options: Vec<String> = passage
        .choice
        .options
        .iter()
        .enumerate()
        .map(|(i, o)| format!("{} [{}] {}", i + 1, o.stance.as_str(), one_line(&o.text)))
        .collect();
    out.push_str(&format!(
        "choice  {} │ {}\n",
        one_line(&passage.choice.prompt),
        options.join(" │ ")
    ));
    let exits: Vec<String> = passage
        .exits
        .iter()
        .map(|e| format!("{}: {}", e.kind.as_str(), one_line(&e.text)))
        .collect();
    out.push_str(&format!("exits   {}\n", exits.join(" │ ")));
    Ok(out)
}

fn pick_option(passage_options: &[OptionDef], choose: Choose, seed: u64, depth: u32) -> usize {
    let wanted = match choose {
        Choose::Integrate => Stance::Integrate,
        Choose::Repress => Stance::Repress,
        Choose::Neutral => Stance::Neutral,
        Choose::Random => {
            let mut rng = substream(seed, depth, "dream", 0);
            return rng.random_range(0..passage_options.len().max(1));
        }
    };
    passage_options
        .iter()
        .position(|o| o.stance == wanted)
        .unwrap_or(0)
}

fn emit(text: &str) -> anyhow::Result<()> {
    let mut out = std::io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()?;
    Ok(())
}

/// Dream `levels` levels (more than the authored story when the pack is endless), choosing per
/// `choose`. AI failures never fail the run: the director falls back to the offline generator.
pub fn run(
    global: &GlobalOpts,
    config: Config,
    _paths: &Paths,
    levels: u32,
    choose: Choose,
) -> anyhow::Result<()> {
    let pack =
        Arc::new(pack::open_for_run(global.pack.as_deref()).map_err(|e| anyhow::anyhow!("{e}"))?);
    let seed = match global.seed {
        Some(s) => s,
        None => {
            let s: u64 = rand::random();
            emit(&format!("seed {s}\n"))?;
            s
        }
    };
    let llm: Option<Arc<dyn Llm>> = config
        .ai
        .enabled
        .then(|| Arc::new(OpenAiCompat::new(&config.ai)) as Arc<dyn Llm>);
    let (tx, rx) = mpsc::channel();
    let timeout = Duration::from_secs(config.ai.timeout_secs.saturating_mul(3).max(1));
    let director = Director::new(llm, Arc::clone(&pack), config.ai.clone(), tx);
    director.warmup();
    let mut d = Dreamer {
        run: RunState::new(
            "DREAMER",
            "N/A",
            "a dream",
            seed,
            pack.meta.id.clone(),
            global.pack.clone(),
        ),
        pack: Arc::clone(&pack),
        director,
        rx,
        timeout,
        epilogue: None,
    };

    for depth in 0..levels {
        let last_authored = !pack.endless_enabled() && depth >= pack.last_depth();
        let is_last = depth + 1 >= levels || last_authored;

        seed::ensure_level(&mut d.run, &pack, depth);
        if !d.ready(Part::Place, depth) {
            d.director.request_place(&d.run, depth);
        }
        d.wait(Part::Place, depth)?;

        if !is_last {
            seed::ensure_level(&mut d.run, &pack, depth + 1);
            d.director.request_place(&d.run, depth + 1);
        }
        d.director.request_passage(&d.run, depth);
        d.wait(Part::Passage, depth)?;
        emit(&level_block(&d, depth)?)?;

        let (options, exits) = {
            let passage = d
                .run
                .levels
                .get(&depth)
                .and_then(|l| l.passage.as_ref())
                .context("passage missing")?;
            (passage.choice.options.clone(), passage.exits.clone())
        };
        let option_index = pick_option(&options, choose, seed, depth);
        let stance = options
            .get(option_index)
            .map_or(Stance::Neutral, |o| o.stance);
        if !d.run.apply_choice(&pack, option_index) {
            bail!("could not apply choice {option_index} at level {depth}");
        }
        let exit_kind = ExitKind::ALL[depth as usize % 3];
        let exit_index = exits.iter().position(|e| e.kind == exit_kind).unwrap_or(0);
        if is_last {
            emit(&format!(
                "-> chose {} ({})\n",
                option_index + 1,
                stance.as_str()
            ))?;
        } else {
            emit(&format!(
                "-> chose {} ({}), exit {}\n",
                option_index + 1,
                stance.as_str(),
                exit_kind.as_str()
            ))?;
        }

        // Endings: an anchor's matching ending, else the generic one at the end of a finite pack.
        let ending = pack
            .anchor_at(depth)
            .and_then(|e| {
                e.anchor
                    .ending
                    .iter()
                    .find(|x| x.matches(stance, d.run.stats.integration))
            })
            .cloned();
        if let Some(ending) = ending {
            d.epilogue = None;
            d.director.request_epilogue(&d.run, &ending);
            d.wait(Part::Epilogue, depth)?;
            if let Some((text, source)) = d.epilogue.take() {
                emit(&format!(
                    "ending  [{}] {}: {}\n",
                    source.tag(),
                    ending.id,
                    one_line(&text)
                ))?;
            }
            d.run.flags.insert(format!("ending_{}", ending.id));
            d.run.ending = Some(ending.id.clone());
        } else if last_authored {
            emit("ending  [authored] end_of_line: The board goes quiet. NO CARRIER.\n")?;
        }
        if last_authored {
            d.run.completed = true;
        }
        if is_last {
            break;
        }

        if !d.run.apply_exit(exit_index) {
            bail!("could not apply exit {exit_index} at level {depth}");
        }
        d.wait(Part::Place, depth + 1)?;
        d.director.request_interlude(&d.run, depth + 1);
        d.wait(Part::Interlude, depth + 1)?;
        if let Some(level) = d.run.levels.get(&(depth + 1)) {
            emit(&format!(
                "interlude [{}] {}\n\n",
                tag(level.interlude_source.as_ref()),
                one_line(level.interlude.as_deref().unwrap_or(""))
            ))?;
        }
        d.run.arrive(depth + 1);
    }
    Ok(())
}
