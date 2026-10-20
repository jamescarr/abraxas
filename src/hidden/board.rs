//! A read-only snapshot of one board, as the hidden line sees it.
//!
//! The socket session never touches the run state: the app builds a [`BoardSnapshot`] from the
//! current run and publishes it to the listener. Everything here is deterministic from the run
//! seed, so the same board always offers the same relay tones and the same zine files.

use std::fs;
use std::path::Path;

use rand::RngExt;
use rand::seq::SliceRandom;

use crate::ans;
use crate::game::danger::{params, run_danger};
use crate::game::host::HostSession;
use crate::game::irc;
use crate::game::programs::{self, Program};
use crate::game::state::{Backdoor, RunState};
use crate::ice::phreak::KEYS;
use crate::pack::model::{HostOs, StoryPack};
use crate::rng::substream;

/// `IrcFile.bot` of files downloaded over the hidden line.
pub const HIDDEN_BOT: &str = "hidden";
/// Art pieces offered by one board when the ANS directory has files.
pub const ART_PER_BOARD: usize = 3;
/// The SAUCE description is cut to this many characters.
const DESCRIPTION_MAX: usize = 40;
/// The shortest title kept when a long credit crowds it.
const MIN_TITLE: usize = 8;
/// Artpack files over this size are left out of the directory.
pub const MAX_ANS_BYTES: u64 = 256 * 1024;

/// One board's hidden line.
#[derive(Debug, Clone)]
pub struct BoardSnapshot {
    pub depth: u32,
    pub handle: String,
    pub bbs_name: String,
    pub title: String,
    pub sysop: String,
    pub host_name: String,
    pub os: HostOs,
    /// The PHREAK-guarded node of this board's host, when it has one.
    pub relay: Option<Relay>,
    pub backdoor: Backdoor,
    /// Art first, then the board's article.
    pub files: Vec<ZineEntry>,
}

/// The relay the hidden line taps: a host node guarded by PHREAK.
#[derive(Debug, Clone)]
pub struct Relay {
    pub id: String,
    pub display: String,
    pub grade: crate::game::danger::IceGrade,
    /// The DTMF sequence the board plants the backdoor with.
    pub tones: Vec<char>,
}

/// How a zine entry is shown.
#[derive(Debug, Clone)]
pub enum ZineKind {
    /// ANSI art: an artpack file decoded to text, or the pool's inline art converted to ANSI.
    Ans { text: String },
    /// An article; reading it teaches `attack`.
    Article { body: String, attack: Program },
}

/// One file in a board's zine directory.
#[derive(Debug, Clone)]
pub struct ZineEntry {
    pub name: String,
    pub description: String,
    pub kind: ZineKind,
    /// Already in the run's files (from this board's hidden line).
    pub owned: bool,
    /// The article's attack is already in the deck.
    pub learned: bool,
}

/// The hidden line of the run's current board, or `None` before the level has a place.
pub fn snapshot(run: &RunState, pack: &StoryPack, ans_dir: &Path) -> Option<BoardSnapshot> {
    let level = run.level()?;
    let place = level.place.as_ref()?;

    let exit = run.exit_taken.as_ref().map(|e| e.kind);
    let danger = run_danger(level.seed.base_danger, exit);
    let host = HostSession::generate(
        level,
        danger,
        exit,
        run.door_attempts,
        run.seed,
        &pack.pool.daemons,
    );

    let relay = host.phreak_node().map(|node| {
        let graded = params(danger).graded(node.grade);
        let len = graded.phreak_start + graded.phreak_rounds - 1;
        let mut rng = substream(run.seed, run.depth, "backdoor", 0);
        let tones = (0..len)
            .map(|_| KEYS[rng.random_range(0..KEYS.len())])
            .collect();
        Relay {
            id: node.id.clone(),
            display: node.display.clone(),
            grade: node.grade,
            tones,
        }
    });

    let backdoor = run.backdoors.get(&run.depth).copied().unwrap_or_default();
    let mut rng = substream(run.seed, run.depth, "hidden", 0);
    let mut files: Vec<ZineEntry> = Vec::new();

    let mut paths = ans::scan(ans_dir);
    if paths.is_empty() {
        // No artpack files: the pool's inline art, converted so it plays like any other ANSI.
        let mut art = pack.pool.zine_art.clone();
        art.shuffle(&mut rng);
        for file in art.into_iter().take(ART_PER_BOARD) {
            let kind = ZineKind::Ans {
                text: ans::from_pipe(&file.body),
            };
            files.push(entry(run, file.name.to_uppercase(), file.description, kind));
        }
    } else {
        paths.shuffle(&mut rng);
        for path in paths {
            if files.len() == ART_PER_BOARD {
                break;
            }
            let Some((name, description, text)) = read_art(&path) else {
                continue;
            };
            if files.iter().any(|f| f.name == name) {
                continue;
            }
            files.push(entry(run, name, description, ZineKind::Ans { text }));
        }
    }

    let articles = &pack.pool.zine_article;
    let n = articles.len();
    if n > 0 {
        let index = (usize::try_from(run.seed % n as u64).unwrap_or(0) + run.depth as usize) % n;
        let article = &articles[index];
        let shadow = run.shadow();
        let fill = irc::filler(&run.handle, &shadow, &run.real_name);
        files.push(entry(
            run,
            article.name.to_uppercase(),
            fill(&article.description),
            ZineKind::Article {
                body: fill(&article.body),
                attack: article.attack,
            },
        ));
    }

    Some(BoardSnapshot {
        depth: run.depth,
        handle: run.handle.clone(),
        bbs_name: place.bbs_name.clone(),
        title: place.title.clone(),
        sysop: place.sysop.handle.clone(),
        host_name: place.host.name.clone(),
        os: place.host.os,
        relay,
        backdoor,
        files,
    })
}

/// Build an entry with `owned`/`learned` read off the run.
fn entry(run: &RunState, name: String, description: String, kind: ZineKind) -> ZineEntry {
    let owned = run.irc.has_file(run.depth, HIDDEN_BOT, &name);
    let learned = match &kind {
        ZineKind::Article { attack, .. } => programs::owns(&run.deck, *attack),
        _ => false,
    };
    ZineEntry {
        name,
        description,
        kind,
        owned,
        learned,
    }
}

fn is_ice(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("ice"))
}

/// `"{title} by {author} ({group})"` (the parts that exist). The credit comes first: the title is
/// cut to what [`DESCRIPTION_MAX`] leaves, and a credit that leaves the title fewer than
/// [`MIN_TITLE`] characters stands alone (cut only when it is too long by itself). `None` when
/// the SAUCE names no title, author or group.
fn sauce_description(sauce: &ans::Sauce) -> Option<String> {
    let mut credit = String::new();
    if !sauce.author.is_empty() {
        credit.push_str(&format!("by {}", sauce.author));
    }
    if !sauce.group.is_empty() {
        if !credit.is_empty() {
            credit.push(' ');
        }
        credit.push_str(&format!("({})", sauce.group));
    }
    let title = sauce.title.trim();
    if credit.is_empty() {
        return (!title.is_empty()).then(|| title.chars().take(DESCRIPTION_MAX).collect());
    }
    // One space separates the title from the credit.
    let room = DESCRIPTION_MAX.saturating_sub(credit.chars().count() + 1);
    if room < MIN_TITLE || title.is_empty() {
        return Some(credit.chars().take(DESCRIPTION_MAX).collect());
    }
    let cut: String = title.chars().take(room).collect();
    Some(format!("{} {credit}", cut.trim_end()))
}

/// The artpack directory's name, used when a file carries no SAUCE title.
fn parent_name(path: &Path) -> String {
    path.parent()
        .and_then(Path::file_name)
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string()
}

/// One artpack file as `(name, description, decoded text)`; `None` when it cannot be read or is
/// larger than [`MAX_ANS_BYTES`].
fn read_art(path: &Path) -> Option<(String, String, String)> {
    if !fs::metadata(path).is_ok_and(|m| m.len() <= MAX_ANS_BYTES) {
        tracing::debug!(
            "hidden line: skipping {} (unreadable or over {MAX_ANS_BYTES} bytes)",
            path.display()
        );
        return None;
    }
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) => {
            tracing::debug!("hidden line: cannot read {}: {e}", path.display());
            return None;
        }
    };
    let sauce = ans::sauce(&bytes);
    let ice = is_ice(path) || sauce.as_ref().is_some_and(|s| s.ice_colors);
    let name = ans::display_name(path, ice);
    let description = sauce
        .as_ref()
        .and_then(sauce_description)
        .unwrap_or_else(|| parent_name(path));
    Some((name, description, ans::decode(&bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::host::IceKindAt;
    use crate::game::state::IrcFile;
    use crate::pack::model::FileDef;
    use crate::ui::app::{install_test_run, test_ctx};
    use tempfile::TempDir;

    fn ctx_run(depth: u32) -> (crate::ui::app::Ctx, TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = test_ctx(dir.path());
        install_test_run(&mut ctx, depth);
        (ctx, dir)
    }

    fn write_ans(dir: &Path, name: &str, body: &[u8]) {
        fs::write(dir.join(name), body).unwrap();
    }

    fn sauce_record(title: &str, author: &str, group: &str, ice: bool) -> Vec<u8> {
        let mut record = vec![b' '; 128];
        record[..7].copy_from_slice(b"SAUCE00");
        record[7..7 + title.len()].copy_from_slice(title.as_bytes());
        record[42..42 + author.len()].copy_from_slice(author.as_bytes());
        record[62..62 + group.len()].copy_from_slice(group.as_bytes());
        record[105] = u8::from(ice);
        record
    }

    #[test]
    fn relay_matches_the_generated_phreak_node() {
        let (ctx, dir) = ctx_run(4);
        let run = ctx.run.as_ref().unwrap();
        let snap = snapshot(run, &ctx.pack, dir.path()).unwrap();
        let relay = snap.relay.expect("depth 4 has a relay");
        let level = run.level().unwrap();
        let exit = run.exit_taken.as_ref().map(|e| e.kind);
        let danger = run_danger(level.seed.base_danger, exit);
        let host = HostSession::generate(
            level,
            danger,
            exit,
            run.door_attempts,
            run.seed,
            &ctx.pack.pool.daemons,
        );
        let node = host
            .nodes()
            .iter()
            .find(|n| n.ice == Some(IceKindAt::Phreak))
            .unwrap();
        assert_eq!(relay.id, node.id);
        assert_eq!(relay.grade, node.grade);
        let graded = params(danger).graded(node.grade);
        assert_eq!(
            relay.tones.len(),
            graded.phreak_start + graded.phreak_rounds - 1
        );
        assert!(relay.tones.iter().all(|c| KEYS.contains(c)));
    }

    #[test]
    fn depth_zero_has_no_relay() {
        let (ctx, dir) = ctx_run(0);
        let snap = snapshot(ctx.run.as_ref().unwrap(), &ctx.pack, dir.path()).unwrap();
        assert!(snap.relay.is_none());
    }

    #[test]
    fn empty_art_dir_falls_back_to_the_pool_art_as_ansi() {
        let (ctx, dir) = ctx_run(1);
        let snap = snapshot(ctx.run.as_ref().unwrap(), &ctx.pack, dir.path()).unwrap();
        let art: Vec<&ZineEntry> = snap
            .files
            .iter()
            .filter(|f| matches!(&f.kind, ZineKind::Ans { .. }))
            .collect();
        let articles = snap
            .files
            .iter()
            .filter(|f| matches!(&f.kind, ZineKind::Article { .. }))
            .count();
        assert_eq!(art.len(), ART_PER_BOARD);
        assert_eq!(articles, 1);
        for f in art {
            let ZineKind::Ans { text } = &f.kind else {
                continue;
            };
            assert!(ctx.pack.pool.zine_art.iter().any(|p| p.name == f.name));
            assert!(
                text.starts_with("\u{1b}[0"),
                "{} is converted: {text:?}",
                f.name
            );
            assert!(text.ends_with("\u{1b}[0m"), "{} is reset: {text:?}", f.name);
        }
    }

    #[test]
    fn ans_dir_fills_the_directory_and_reads_sauce() {
        let (ctx, dir) = ctx_run(1);
        let ans_dir = dir.path().join("artpack");
        fs::create_dir_all(&ans_dir).unwrap();
        for name in ["a.ANS", "b.ans"] {
            write_ans(&ans_dir, name, b"\x1b[31mHI\r\n");
        }
        let mut saucy = b"\x1b[31mHI\r\n\x1a".to_vec();
        saucy.extend(sauce_record("THE COW", "J. DOE", "ACiD", true));
        write_ans(&ans_dir, "saucy.ans", &saucy);

        let snap = snapshot(ctx.run.as_ref().unwrap(), &ctx.pack, &ans_dir).unwrap();
        let ans = snap
            .files
            .iter()
            .filter(|f| matches!(&f.kind, ZineKind::Ans { .. }))
            .count();
        assert_eq!(ans, ART_PER_BOARD);
        // The SAUCE file wins its description from title/author/group.
        let described = snap
            .files
            .iter()
            .find(|f| f.description.starts_with("THE COW"))
            .expect("the SAUCE description is used");
        assert_eq!(described.description, "THE COW by J. DOE (ACiD)");
        assert_eq!(described.name, "SAUCY.ICE");
    }

    #[test]
    fn the_same_seed_and_dir_pick_the_same_files() {
        let (ctx, dir) = ctx_run(1);
        let ans_dir = dir.path().join("ans");
        fs::create_dir_all(&ans_dir).unwrap();
        for name in ["a.ANS", "b.ANS", "c.ANS", "d.ANS", "e.ANS"] {
            write_ans(&ans_dir, name, b"X");
        }
        let run = ctx.run.as_ref().unwrap();
        let names = |snap: BoardSnapshot| {
            snap.files
                .into_iter()
                .map(|f| f.name)
                .collect::<Vec<String>>()
        };
        let first = names(snapshot(run, &ctx.pack, &ans_dir).unwrap());
        let second = names(snapshot(run, &ctx.pack, &ans_dir).unwrap());
        assert_eq!(first, second);
    }

    #[test]
    fn owned_follows_the_run_files() {
        let (mut ctx, dir) = ctx_run(1);
        let ans_dir = dir.path().join("one");
        fs::create_dir_all(&ans_dir).unwrap();
        write_ans(&ans_dir, "only.ans", b"X");
        let name = snapshot(ctx.run.as_ref().unwrap(), &ctx.pack, &ans_dir)
            .unwrap()
            .files
            .first()
            .unwrap()
            .name
            .clone();
        let run = ctx.run.as_mut().unwrap();
        let depth = run.depth;
        run.irc.files.push(IrcFile {
            depth,
            bot: HIDDEN_BOT.to_string(),
            file: FileDef {
                name: name.clone(),
                description: String::new(),
                body: String::new(),
            },
        });
        let snap = snapshot(ctx.run.as_ref().unwrap(), &ctx.pack, &ans_dir).unwrap();
        let found = snap.files.iter().find(|f| f.name == name).unwrap();
        assert!(found.owned);
    }

    #[test]
    fn duplicate_names_are_listed_once_and_big_files_are_skipped() {
        let (ctx, dir) = ctx_run(1);
        let ans_dir = dir.path().join("packs");
        for pack in ["a", "b", "c"] {
            fs::create_dir_all(ans_dir.join(pack)).unwrap();
        }
        write_ans(&ans_dir.join("a"), "same.ans", b"A");
        write_ans(&ans_dir.join("b"), "same.ans", b"B");
        write_ans(&ans_dir.join("c"), "huge.ans", &vec![b'x'; 256 * 1024 + 1]);
        let snap = snapshot(ctx.run.as_ref().unwrap(), &ctx.pack, &ans_dir).unwrap();
        let names: Vec<&str> = snap
            .files
            .iter()
            .filter(|f| matches!(&f.kind, ZineKind::Ans { .. }))
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, vec!["SAME.ANS"]);
    }

    #[test]
    fn long_sauce_titles_are_cut_so_the_credit_stays() {
        let long = ans::Sauce {
            title: "A very long title that goes on and on".to_string(),
            author: "ZORK".to_string(),
            group: "ACiD".to_string(),
            ice_colors: false,
        };
        let text = sauce_description(&long).unwrap();
        assert!(text.ends_with(" by ZORK (ACiD)"), "{text}");
        assert!(text.chars().count() <= DESCRIPTION_MAX, "{text}");
        let bare = ans::Sauce {
            title: "ONLY A TITLE".to_string(),
            author: String::new(),
            group: String::new(),
            ice_colors: false,
        };
        assert_eq!(sauce_description(&bare).unwrap(), "ONLY A TITLE");
        let untitled = ans::Sauce {
            title: String::new(),
            ..bare
        };
        assert_eq!(sauce_description(&untitled), None);
        // No title but a credit: the credit is the description, not the directory name.
        let credit_only = ans::Sauce {
            author: "ZORK".to_string(),
            group: "ACiD".to_string(),
            ..untitled.clone()
        };
        assert_eq!(sauce_description(&credit_only).unwrap(), "by ZORK (ACiD)");
        // A credit that crowds the title out stands alone; one that is too long alone is cut.
        let crowded = ans::Sauce {
            title: "THE TITLE".to_string(),
            author: "A".repeat(17),
            group: "B".repeat(12),
            ..untitled.clone()
        };
        let text = sauce_description(&crowded).unwrap();
        assert_eq!(text, format!("by {} ({})", "A".repeat(17), "B".repeat(12)));
        let huge = ans::Sauce {
            author: "C".repeat(60),
            ..crowded
        };
        assert_eq!(
            sauce_description(&huge).unwrap().chars().count(),
            DESCRIPTION_MAX
        );
    }
}
