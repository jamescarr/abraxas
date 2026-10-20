//! IRC: a level's network of channels, chatter and fserv bots. Generated from the level seed;
//! the Irc screen drives it and the run records what was taken (`RunState::irc`).

use rand::RngExt;
use rand::seq::{IndexedRandom, SliceRandom};

use crate::game::danger::{self, IceGrade};
use crate::game::host::{DUMP_SHOCK_EGO, IceKindAt};
use crate::game::programs::Program;
use crate::game::state::Place;
use crate::pack::model::{FileDef, HostOs, Pool};
use crate::rng::{Rng, substream};
use crate::text::{Subst, fill_placeholders, strip_pipe_codes, to_cp437_safe};

/// Channels on every network.
pub const CHANNELS: usize = 3;
/// Price of a file on an open fserv.
pub const FILE_KB: u32 = 8;
/// Trace every `/hack` attempt costs before the ICE even starts.
pub const HACK_TRACE: f32 = 5.0;
/// Trace a TEARDROP crash costs.
pub const TEARDROP_TRACE: f32 = 15.0;
/// EGO lost to a K-line (the IRC dump shock).
pub const KLINE_EGO: i32 = DUMP_SHOCK_EGO;

/// Used when the pool has no server names (validation makes that impossible for loaded packs).
const FALLBACK_SERVER: &str = "irc.abraxas.net";

/// One level's network.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IrcNet {
    pub server: String,
    /// Always [`CHANNELS`] of them; the first carries the level's tagline as its topic.
    pub channels: Vec<Channel>,
    /// Every chatterer on the network, each in exactly one channel.
    pub nicks: Vec<String>,
    pub fservs: Vec<Fserv>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Channel {
    pub name: String,
    pub topic: String,
    /// Indices into [`IrcNet::nicks`].
    pub nicks: Vec<usize>,
    /// Indices into [`IrcNet::fservs`].
    pub fservs: Vec<usize>,
}

/// A bot that sells files and programs for RATIO.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fserv {
    pub bot: String,
    /// Index into [`IrcNet::channels`].
    pub channel: usize,
    /// `None`: open to everyone. `Some`: sells nothing until cracked.
    pub lock: Option<Lock>,
    pub stock: Vec<Good>,
}

/// The ICE that guards a locked fserv.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lock {
    pub ice: IceKindAt,
    pub grade: IceGrade,
    /// The console's OS when `ice` is DAEMON.
    pub os: HostOs,
}

/// One thing an fserv sells.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Good {
    Program(Program),
    File(FileDef),
}

impl Good {
    /// The DOS file name.
    pub fn name(&self) -> &str {
        match self {
            Good::Program(p) => p.file_name(),
            Good::File(f) => &f.name,
        }
    }

    /// The listing line: name, price and what it does.
    pub fn label(&self) -> String {
        match self {
            Good::Program(p) => format!("{}  {}K  {}", p.file_name(), p.cost_kb(), p.effect()),
            Good::File(f) => format!("{}  {}K  {}", f.name, FILE_KB, f.description),
        }
    }
}

impl Fserv {
    /// Key in `IrcProgress::cracked`.
    pub fn key(&self, depth: u32) -> String {
        format!("{depth}:{}", self.bot)
    }

    /// `open`, `locked` or `cracked`.
    pub fn state(&self, cracked: bool) -> &'static str {
        match (&self.lock, cracked) {
            (None, _) => "open",
            (Some(_), true) => "cracked",
            (Some(_), false) => "locked",
        }
    }

    /// True while a lock still stands between the player and the stock.
    pub fn is_locked(&self, cracked: bool) -> bool {
        self.lock.is_some() && !cracked
    }

    /// `open`, `cracked`, or `locked (BREACH soft)` naming the ICE and its grade.
    pub fn status(&self, cracked: bool) -> String {
        match &self.lock {
            Some(lock) if !cracked => {
                format!("locked ({} {})", lock.ice.label(), lock.grade.label())
            }
            _ => self.state(cracked).to_string(),
        }
    }

    /// What the bot announces in its channel.
    pub fn advert(&self, cracked: bool) -> String {
        format!(
            "fserv online ─ /fserv {} ─ {} files ─ {}",
            self.bot,
            self.stock.len(),
            self.status(cracked)
        )
    }
}

/// Everything `generate` reads.
pub struct GenInput<'a> {
    pub place: &'a Place,
    pub depth: u32,
    pub run_danger: u8,
    pub pool: &'a Pool,
    pub seed: u64,
    pub handle: &'a str,
    pub shadow: &'a str,
    pub real_name: &'a str,
}

/// The text filter for pool lines: placeholders filled, CP437-safe, pipe codes stripped.
pub fn filler<'a>(
    handle: &'a str,
    shadow: &'a str,
    real_name: &'a str,
) -> impl Fn(&str) -> String + 'a {
    move |text| {
        let subst = Subst {
            handle,
            shadow,
            real_name,
            motif: None,
            exit: None,
        };
        strip_pipe_codes(&to_cp437_safe(&fill_placeholders(text, &subst)))
    }
}

/// `list` without repeats (ignoring case), first occurrence wins. Pools are not validated for
/// uniqueness, and a network needs unique channel and bot names.
fn distinct(list: &[String]) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    list.iter()
        .filter(|s| seen.insert(s.to_ascii_lowercase()))
        .cloned()
        .collect()
}

/// The password with the first half of its letters shown: `RAVENS` becomes `RAV___`.
fn mask(password: &str) -> String {
    let shown = password.chars().count() / 2;
    password
        .chars()
        .enumerate()
        .map(|(i, c)| if i < shown { c } else { '_' })
        .collect()
}

fn roll_lock(run_danger: u8, rng: &mut Rng) -> Lock {
    const ICE: [IceKindAt; 3] = [IceKindAt::Breach, IceKindAt::Phreak, IceKindAt::Daemon];
    const OS: [HostOs; 4] = [HostOs::Unix, HostOs::Vax, HostOs::Mainframe, HostOs::Dos];
    Lock {
        ice: ICE[rng.random_range(0..ICE.len())],
        grade: danger::roll_grade(run_danger, rng),
        os: OS[rng.random_range(0..OS.len())],
    }
}

/// The level's network, deterministic from `substream(seed, depth, "irc")`.
///
/// Fserv 0 is always open and sells TEARDROP.SH; fserv 1 is always locked and sells the `.NFO`
/// that names half the front-door password; fserv 2 exists from run danger 3 and is locked half
/// the time. Every fserv also sells one zine and one program the BBS sells; no program is sold
/// twice.
pub fn generate(input: &GenInput) -> IrcNet {
    let pool = input.pool;
    let fill = filler(input.handle, input.shadow, input.real_name);
    let mut rng = substream(input.seed, input.depth, "irc", 0);

    let server = pool
        .irc_servers
        .choose(&mut rng)
        .cloned()
        .unwrap_or_else(|| FALLBACK_SERVER.to_string());

    let mut names = distinct(&pool.irc_channels);
    names.shuffle(&mut rng);
    names.truncate(CHANNELS);
    while names.len() < CHANNELS {
        names.push(format!("#chan{}", names.len() + 1));
    }

    let mut spare: Vec<String> = distinct(&pool.irc_nicks)
        .into_iter()
        .filter(|n| !n.eq_ignore_ascii_case(input.handle))
        .collect();
    spare.shuffle(&mut rng);
    let mut spare = spare.into_iter();
    let mut guests = 0u32;
    let mut nicks: Vec<String> = Vec::new();
    let mut channels: Vec<Channel> = Vec::with_capacity(CHANNELS);
    for (i, name) in names.into_iter().enumerate() {
        let topic = if i == 0 {
            input.place.tagline.clone()
        } else {
            pool.irc_chatter
                .choose(&mut rng)
                .map_or_else(String::new, |line| fill(line))
        };
        let mut members = Vec::new();
        for _ in 0..rng.random_range(4..=7) {
            let nick = spare.next().unwrap_or_else(|| {
                guests += 1;
                format!("guest{guests:02}")
            });
            members.push(nicks.len());
            nicks.push(nick);
        }
        channels.push(Channel {
            name,
            topic,
            nicks: members,
            fservs: Vec::new(),
        });
    }

    let count = 2 + usize::from(input.run_danger >= 3);
    let mut bots = distinct(&pool.irc_bots);
    bots.shuffle(&mut rng);
    let mut bots = bots.into_iter();
    let mut sold: Vec<Program> = Program::ALL
        .into_iter()
        .filter(|p| p.sold_on_bbs())
        .collect();
    sold.shuffle(&mut rng);
    let mut zines = pool.irc_zines.clone();
    zines.shuffle(&mut rng);
    let nfo = pool.irc_nfo.choose(&mut rng).map(|f| {
        let mut file = fill_file(f, &fill);
        file.body = format!(
            "{}\n\nfront door at {}: {}",
            file.body,
            input.place.host.name,
            mask(&input.place.password)
        );
        file
    });

    let mut fservs: Vec<Fserv> = Vec::with_capacity(count);
    for i in 0..count {
        let bot = bots.next().unwrap_or_else(|| format!("fserv{}", i + 1));
        let channel = i % CHANNELS;
        let lock = match i {
            0 => None,
            1 => Some(roll_lock(input.run_danger, &mut rng)),
            _ => {
                let locked = rng.random_range(0..2) == 0;
                locked.then(|| roll_lock(input.run_danger, &mut rng))
            }
        };
        let mut stock: Vec<Good> = Vec::new();
        if i == 0 {
            stock.push(Good::Program(Program::Teardrop));
        }
        stock.extend(sold.get(i).copied().map(Good::Program));
        if i == 1 {
            stock.extend(nfo.clone().map(Good::File));
        }
        if !zines.is_empty() {
            stock.push(Good::File(fill_file(&zines[i % zines.len()], &fill)));
        }
        channels[channel].fservs.push(i);
        fservs.push(Fserv {
            bot,
            channel,
            lock,
            stock,
        });
    }

    IrcNet {
        server,
        channels,
        nicks,
        fservs,
    }
}

/// A pool file with its name upper-cased and its description and body passed through `fill`.
pub fn fill_file(f: &FileDef, fill: &impl Fn(&str) -> String) -> FileDef {
    FileDef {
        name: f.name.to_ascii_uppercase(),
        description: fill(&f.description),
        body: fill(&f.body),
    }
}

/// A random nick of `channel` and a filled chatter line from the pool. Falls back to `anon` and
/// `...` when the channel or the pool is empty.
pub fn chatter(
    net: &IrcNet,
    channel: usize,
    pool: &Pool,
    rng: &mut Rng,
    fill: impl Fn(&str) -> String,
) -> (String, String) {
    let nick = net
        .channels
        .get(channel)
        .and_then(|c| c.nicks.choose(rng))
        .and_then(|&i| net.nicks.get(i))
        .cloned()
        .unwrap_or_else(|| "anon".to_string());
    let line = pool
        .irc_chatter
        .choose(rng)
        .map_or_else(|| "...".to_string(), |l| fill(l));
    (nick, line)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::pack;
    use crate::ui::app::sample_place;

    struct Fixture {
        pool: Pool,
        place: Place,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                pool: pack::embedded().unwrap().pool,
                place: sample_place("TEST"),
            }
        }

        fn net(&self, seed: u64, run_danger: u8, handle: &str) -> IrcNet {
            self.net_with(&self.pool, seed, run_danger, handle)
        }

        fn net_with(&self, pool: &Pool, seed: u64, run_danger: u8, handle: &str) -> IrcNet {
            generate(&GenInput {
                place: &self.place,
                depth: 3,
                run_danger,
                pool,
                seed,
                handle,
                shadow: "REDSET",
                real_name: "Test Person",
            })
        }
    }

    fn programs(net: &IrcNet) -> Vec<Program> {
        net.fservs
            .iter()
            .flat_map(|fs| &fs.stock)
            .filter_map(|g| match g {
                Good::Program(p) => Some(*p),
                Good::File(_) => None,
            })
            .collect()
    }

    #[test]
    fn generation_is_deterministic_and_seed_sensitive() {
        let f = Fixture::new();
        assert_eq!(f.net(42, 3, "Tester"), f.net(42, 3, "Tester"));
        assert_ne!(f.net(42, 3, "Tester"), f.net(43, 3, "Tester"));
    }

    #[test]
    fn every_network_has_three_channels_an_open_and_a_locked_fserv() {
        let f = Fixture::new();
        for danger in 1..=5u8 {
            for seed in 0..30 {
                let net = f.net(seed, danger, "Tester");
                assert_eq!(net.channels.len(), CHANNELS);
                assert_eq!(net.fservs.len(), if danger >= 3 { 3 } else { 2 });
                assert!(net.fservs[0].lock.is_none());
                assert!(net.fservs[1].lock.is_some());
                for (i, fs) in net.fservs.iter().enumerate() {
                    assert!(net.channels[fs.channel].fservs.contains(&i));
                }
                let mut seated: Vec<usize> = Vec::new();
                for c in &net.channels {
                    assert!((4..=7).contains(&c.nicks.len()), "{c:?}");
                    seated.extend(&c.nicks);
                }
                seated.sort_unstable();
                let everyone: Vec<usize> = (0..net.nicks.len()).collect();
                assert_eq!(seated, everyone, "each nick sits in exactly one channel");
            }
        }
    }

    #[test]
    fn the_third_fserv_is_locked_about_half_the_time() {
        let f = Fixture::new();
        let locked = (0..200)
            .filter(|&seed| f.net(seed, 3, "Tester").fservs[2].lock.is_some())
            .count();
        assert!((60..=140).contains(&locked), "{locked} of 200");
    }

    #[test]
    fn the_nfo_names_half_the_front_door_password() {
        let f = Fixture::new();
        let net = f.net(7, 3, "Tester");
        let nfo = net.fservs[1]
            .stock
            .iter()
            .find_map(|g| match g {
                Good::File(file) if file.name.ends_with(".NFO") => Some(file),
                _ => None,
            })
            .unwrap();
        assert!(
            nfo.body
                .ends_with("\n\nfront door at ODS TEST EXCHANGE: RAV___"),
            "{}",
            nfo.body
        );
        assert_eq!(mask("RAVENS"), "RAV___");
        assert_eq!(mask("OWLET"), "OW___");
        assert_eq!(mask("MIDNIGHT"), "MIDN____");
        assert_eq!(mask(""), "");
    }

    #[test]
    fn nicks_never_match_the_handle_in_any_case() {
        let f = Fixture::new();
        assert!(f.pool.irc_nicks.iter().any(|n| n == "phr34k"));
        for seed in 0..50 {
            let net = f.net(seed, 2, "PHR34K");
            assert!(
                net.nicks.iter().all(|n| !n.eq_ignore_ascii_case("phr34k")),
                "seed {seed}"
            );
        }
    }

    #[test]
    fn programs_are_never_sold_twice_and_only_fserv_zero_sells_teardrop() {
        let f = Fixture::new();
        for danger in 1..=5u8 {
            for seed in 0..40 {
                let net = f.net(seed, danger, "Tester");
                let all = programs(&net);
                let unique: BTreeSet<Program> = all.iter().copied().collect();
                assert_eq!(unique.len(), all.len(), "{all:?}");
                assert!(
                    net.fservs[0]
                        .stock
                        .contains(&Good::Program(Program::Teardrop))
                );
                let elsewhere = programs(&IrcNet {
                    fservs: net.fservs[1..].to_vec(),
                    ..net.clone()
                });
                assert!(elsewhere.iter().all(|p| p.sold_on_bbs()), "{elsewhere:?}");
            }
        }
    }

    #[test]
    fn empty_pools_are_padded_instead_of_failing() {
        let f = Fixture::new();
        let net = f.net_with(&Pool::default(), 1, 1, "Tester");
        assert_eq!(net.server, FALLBACK_SERVER);
        let names: Vec<&str> = net.channels.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["#chan1", "#chan2", "#chan3"]);
        assert!(net.nicks.iter().all(|n| n.starts_with("guest")));
        assert!(net.nicks.contains(&"guest01".to_string()));
        let bots: Vec<&str> = net.fservs.iter().map(|fs| fs.bot.as_str()).collect();
        assert_eq!(bots, ["fserv1", "fserv2"]);
        assert!(
            net.fservs
                .iter()
                .flat_map(|fs| &fs.stock)
                .all(|g| matches!(g, Good::Program(_)))
        );
    }

    #[test]
    fn repeated_pool_entries_never_repeat_a_channel_or_bot_name() {
        let f = Fixture::new();
        let pool = Pool {
            irc_channels: ["#a", "#A", "#b", "#b"].map(String::from).to_vec(),
            irc_bots: ["Bot", "bot", "BOT"].map(String::from).to_vec(),
            ..Pool::default()
        };
        for seed in 0..20 {
            let net = f.net_with(&pool, seed, 3, "Tester");
            let channels: BTreeSet<String> = net
                .channels
                .iter()
                .map(|c| c.name.to_ascii_lowercase())
                .collect();
            assert_eq!(channels.len(), CHANNELS, "{:?}", net.channels);
            let bots: BTreeSet<String> = net
                .fservs
                .iter()
                .map(|fs| fs.bot.to_ascii_lowercase())
                .collect();
            assert_eq!(bots.len(), net.fservs.len(), "{:?}", net.fservs);
        }
    }

    #[test]
    fn chatter_comes_from_the_channel_with_placeholders_filled() {
        let f = Fixture::new();
        let net = f.net(1, 1, "Tester");
        let pool = Pool {
            irc_chatter: vec!["{handle} hi".to_string()],
            ..Pool::default()
        };
        let fill = filler("Tester", "REDSET", "Test Person");
        let mut rng = substream(1, 0, "chatter-test", 0);
        for _ in 0..10 {
            let (nick, line) = chatter(&net, 1, &pool, &mut rng, &fill);
            assert!(net.channels[1].nicks.iter().any(|&i| net.nicks[i] == nick));
            assert_eq!(line, "Tester hi");
        }
        let none = chatter(&net, 9, &Pool::default(), &mut rng, &fill);
        assert_eq!(none, ("anon".to_string(), "...".to_string()));
    }

    #[test]
    fn fserv_status_follows_the_lock() {
        let open = Fserv {
            bot: "warezbot".into(),
            channel: 0,
            lock: None,
            stock: Vec::new(),
        };
        assert_eq!(open.status(false), "open");
        assert!(!open.is_locked(false));
        assert_eq!(
            open.advert(false),
            "fserv online ─ /fserv warezbot ─ 0 files ─ open"
        );
        let locked = Fserv {
            lock: Some(Lock {
                ice: IceKindAt::Breach,
                grade: IceGrade::Soft,
                os: HostOs::Unix,
            }),
            ..open
        };
        assert_eq!(locked.status(false), "locked (BREACH soft)");
        assert_eq!(locked.state(false), "locked");
        assert!(locked.is_locked(false));
        assert_eq!(locked.status(true), "cracked");
        assert!(!locked.is_locked(true));
        assert_eq!(locked.key(3), "3:warezbot");
    }

    #[test]
    fn teardrop_text_names_its_trace_cost() {
        let effect = Program::Teardrop.effect();
        assert!(
            effect.contains(&format!("+{TEARDROP_TRACE:.0} trace")),
            "{effect}"
        );
    }
}
