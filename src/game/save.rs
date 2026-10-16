//! Save files and the caller log.
//!
//! `saves/{lowercased handle}.json` holds one [`SaveFile`] per player; `callers.json` is the
//! newest-first list of recent callers the Login, Dialer and Users screens read. Every write goes
//! to a temp file in the same directory and is renamed into place.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::config::Paths;
use crate::game::state::{RunState, SAVE_FORMAT, SaveFile};

/// Most callers kept in `callers.json`.
pub const MAX_CALLERS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CallerStatus {
    Online,
    Lost,
    Ascended,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallerRecord {
    pub handle: String,
    pub depth: u32,
    pub clock: String,
    pub status: CallerStatus,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct CallersFile {
    #[serde(default)]
    callers: Vec<CallerRecord>,
}

#[derive(Debug, Clone)]
pub struct SaveStore {
    saves_dir: PathBuf,
    callers_file: PathBuf,
}

impl SaveStore {
    pub fn new(paths: &Paths) -> Self {
        Self {
            saves_dir: paths.saves_dir(),
            callers_file: paths.callers_file(),
        }
    }

    /// `saves/{handle}.json`, lowercased. Anything but `[A-Za-z0-9_-]` is rejected so a handle can
    /// never address a file outside `saves/`.
    fn save_path(&self, handle: &str) -> Result<PathBuf> {
        let ok = !handle.is_empty()
            && handle
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
        if !ok {
            bail!("invalid handle {handle:?} for a save file name");
        }
        Ok(self
            .saves_dir
            .join(format!("{}.json", handle.to_ascii_lowercase())))
    }

    pub fn load(&self, handle: &str) -> Result<Option<RunState>> {
        let path = self.save_path(handle)?;
        let text = match fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("reading {}", path.display())),
        };
        // Check the format first so a future schema reports as a version error, not a parse error.
        let value: serde_json::Value = serde_json::from_str(&text)
            .with_context(|| format!("{} is corrupt", path.display()))?;
        let format = value.get("format").and_then(serde_json::Value::as_u64);
        if format != Some(u64::from(SAVE_FORMAT)) {
            bail!(
                "{}: unsupported save format {} (expected {SAVE_FORMAT})",
                path.display(),
                format.map_or_else(|| "missing".to_string(), |f| f.to_string())
            );
        }
        let file: SaveFile = serde_json::from_value(value)
            .with_context(|| format!("{} is corrupt", path.display()))?;
        Ok(Some(file.run))
    }

    pub fn save(&self, run: &RunState) -> Result<()> {
        let path = self.save_path(&run.handle)?;
        #[derive(Serialize)]
        struct SaveRef<'a> {
            format: u32,
            run: &'a RunState,
        }
        let json = serde_json::to_vec_pretty(&SaveRef {
            format: SAVE_FORMAT,
            run,
        })
        .with_context(|| format!("serialising save for {}", run.handle))?;
        write_atomic(&path, &json)
    }

    pub fn delete(&self, handle: &str) -> Result<()> {
        let path = self.save_path(handle)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("deleting {}", path.display())),
        }
    }

    pub fn exists(&self, handle: &str) -> bool {
        self.save_path(handle).is_ok_and(|p| p.is_file())
    }

    /// True when any save or caller record exists (the Dialer then lists ABRAXAS).
    pub fn has_any_save_or_caller(&self) -> bool {
        let any_save = fs::read_dir(&self.saves_dir).is_ok_and(|mut rd| {
            rd.any(|e| e.is_ok_and(|e| e.path().extension().is_some_and(|x| x == "json")))
        });
        any_save || !self.callers().is_empty()
    }

    /// Newest first, at most [`MAX_CALLERS`]. A missing or corrupt file reads as empty.
    pub fn callers(&self) -> Vec<CallerRecord> {
        let mut list = fs::read_to_string(&self.callers_file)
            .ok()
            .and_then(|t| serde_json::from_str::<CallersFile>(&t).ok())
            .map(|f| f.callers)
            .unwrap_or_default();
        list.truncate(MAX_CALLERS);
        list
    }

    pub fn last_callers(&self, n: usize) -> Vec<CallerRecord> {
        let mut list = self.callers();
        list.truncate(n);
        list
    }

    /// Put `rec` at the front, replacing any entry with the same handle (case-insensitive).
    pub fn record_caller(&self, rec: CallerRecord) -> Result<()> {
        let mut callers = self.callers();
        callers.retain(|c| !c.handle.eq_ignore_ascii_case(&rec.handle));
        callers.insert(0, rec);
        callers.truncate(MAX_CALLERS);
        let json =
            serde_json::to_vec_pretty(&CallersFile { callers }).context("serialising callers")?;
        write_atomic(&self.callers_file, &json)
    }

    pub fn is_dead(&self, handle: &str) -> bool {
        self.callers()
            .iter()
            .any(|c| c.status == CallerStatus::Lost && c.handle.eq_ignore_ascii_case(handle))
    }

    /// Ego death: delete the save and record `Lost` at the run's depth.
    pub fn ego_death(&self, run: &RunState, clock: &str) -> Result<()> {
        self.finish(run, clock, CallerStatus::Lost)
    }

    /// A completed run: delete the save and record `Ascended`.
    pub fn ascend(&self, run: &RunState, clock: &str) -> Result<()> {
        self.finish(run, clock, CallerStatus::Ascended)
    }

    fn finish(&self, run: &RunState, clock: &str, status: CallerStatus) -> Result<()> {
        self.delete(&run.handle)?;
        self.record_caller(CallerRecord {
            handle: run.handle.clone(),
            depth: run.depth,
            clock: clock.to_string(),
            status,
        })
    }
}

/// Write `bytes` to a sibling temp file, then rename over `path`.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path
        .parent()
        .with_context(|| format!("{} has no parent directory", path.display()))?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let name = path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let tmp = dir.join(format!(".{name}.tmp"));
    let written = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("writing {}", path.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::chronicle::{ChosenOption, ChronicleEntry};
    use crate::game::programs::Program;
    use crate::game::state::{
        GenSource, HostInfo, Level, LevelSeed, Passage, Phase, Place, PlayerPost, Sysop,
    };
    use crate::pack::model::{
        ChoiceDef, ExitDef, ExitKind, FileDef, HostOs, ObjectiveKind, OptionDef, PostDef, Stance,
        VisionKind,
    };

    fn store() -> (tempfile::TempDir, SaveStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let s = SaveStore::new(&Paths::in_dir(dir.path().to_path_buf()));
        (dir, s)
    }

    fn rec(handle: &str, depth: u32, status: CallerStatus) -> CallerRecord {
        CallerRecord {
            handle: handle.into(),
            depth,
            clock: "23:10:00".into(),
            status,
        }
    }

    fn rich_run() -> RunState {
        let mut run = RunState::new("TesterOne", "Ada", "a payphone", 42, "rollover", None);
        run.depth = 3;
        run.stats.ego = 55;
        run.stats.integration = -4;
        run.deck.insert(Program::Decrypt, 2);
        run.deck.insert(Program::IcePick, 1);
        run.flags.insert("admitted_fear".into());
        run.used_pool.insert("title:Foo".into());
        run.player_posts.push(PlayerPost {
            depth: 1,
            subject: "hi".into(),
            body: "hello".into(),
        });
        let exit = ExitDef {
            kind: ExitKind::Deep,
            text: "down".into(),
        };
        run.exit_taken = Some(exit.clone());
        run.chronicle.upsert(ChronicleEntry {
            depth: 2,
            act_id: "nigredo".into(),
            act_name: "NIGREDO".into(),
            title: "THE HALL".into(),
            class: 1,
            omen: 29,
            level_card: "card".into(),
            summary: "sum".into(),
            choice: Some(ChosenOption {
                text: "go".into(),
                stance: Stance::Repress,
            }),
            exit: Some(exit.clone()),
        });
        let seed = LevelSeed {
            depth: 3,
            act_id: "nigredo".into(),
            anchor: Some("03-black-sun.toml".into()),
            base_danger: 2,
            omen: 63,
            motifs: vec!["a".into(), "b".into()],
            room: "room".into(),
            focus: Some("shadow".into()),
            objective_kind: ObjectiveKind::Crash,
            target_daemon: Some("pleroma".into()),
            wordlock_len: 6,
        };
        let mut level = Level::new(seed.clone(), vec![Program::Ghost, Program::Mirror]);
        level.place = Some(Place {
            title: "T".into(),
            bbs_name: "B".into(),
            tagline: "tag".into(),
            level_card: "lc".into(),
            sysop: Sysop {
                handle: "SHADOW".into(),
                cast_id: Some("shadow".into()),
            },
            host: HostInfo {
                name: "H".into(),
                os: HostOs::Vax,
                banner: "b".into(),
                objective: "o".into(),
                target_file: "X.LOG".into(),
                objective_ice: None,
                nodes: Some(5),
                node_names: vec!["n".into(); 8],
                ice_names: vec!["i".into(); 3],
            },
            posts: vec![PostDef {
                from: "a".into(),
                subject: "s".into(),
                body: "b".into(),
            }],
            text_files: vec![FileDef {
                name: "A.TXT".into(),
                description: "d".into(),
                body: "b".into(),
            }],
            host_files: vec![],
            words: vec!["RAVENS".into()],
            password: "RAVENS".into(),
        });
        level.place_source = Some(GenSource::Ai {
            model: "m".into(),
            millis: 1200,
        });
        level.passage = Some(Passage {
            beat: "beat".into(),
            choice: ChoiceDef {
                prompt: "p".into(),
                options: vec![OptionDef {
                    text: "t".into(),
                    stance: Stance::Neutral,
                    flags: vec!["f".into()],
                }],
            },
            exits: vec![exit],
            vision: VisionKind::Ouroboros,
            captions: vec!["1".into(), "2".into(), "3".into()],
            summary: "s".into(),
        });
        level.passage_source = Some(GenSource::Offline {
            reason: "timeout".into(),
        });
        level.interlude = Some("text".into());
        run.levels.insert(3, level.clone());
        run.levels
            .insert(4, Level::new(LevelSeed { depth: 4, ..seed }, vec![]));
        run.phase = Phase::Hub {
            door_done: true,
            choice: Some(1),
        };
        run
    }

    fn json(run: &RunState) -> serde_json::Value {
        serde_json::to_value(run).expect("json")
    }

    #[test]
    fn round_trip_rich_state_and_every_phase() {
        let (_d, s) = store();
        let mut run = rich_run();
        for phase in [
            Phase::Arrival,
            Phase::Hub {
                door_done: false,
                choice: None,
            },
            Phase::Descending { exit: 2 },
            Phase::Ending,
            Phase::Hub {
                door_done: true,
                choice: Some(1),
            },
        ] {
            run.phase = phase;
            s.save(&run).expect("save");
            // Handle lookup is case-insensitive.
            let back = s.load("TESTERONE").expect("load").expect("some");
            assert_eq!(json(&back), json(&run));
            assert_eq!(back.phase, phase);
        }
    }

    #[test]
    fn load_absent_is_none() {
        let (_d, s) = store();
        assert!(s.load("nobody").expect("ok").is_none());
        assert!(!s.exists("nobody"));
    }

    #[test]
    fn corrupt_and_wrong_format_name_the_file() {
        let (_d, s) = store();
        let path = s.save_path("bad").expect("path");
        fs::create_dir_all(&s.saves_dir).expect("mkdir");
        fs::write(&path, "{ not json").expect("write");
        let err = format!("{:#}", s.load("bad").expect_err("corrupt"));
        assert!(err.contains("bad.json"), "{err}");

        let mut v = json(&rich_run());
        v = serde_json::json!({ "format": 99, "run": v });
        fs::write(&path, v.to_string()).expect("write");
        let err = format!("{:#}", s.load("bad").expect_err("format"));
        assert!(err.contains("bad.json") && err.contains("99"), "{err}");

        fs::write(&path, r#"{"format":1,"run":{}}"#).expect("write");
        let err = format!("{:#}", s.load("bad").expect_err("shape"));
        assert!(err.contains("bad.json"), "{err}");
    }

    #[test]
    fn save_leaves_no_temp_and_ignores_stale_temp() {
        let (_d, s) = store();
        let run = rich_run();
        s.save(&run).expect("save");
        // A stale temp from an interrupted write must not affect the real file or later saves.
        let tmp = s.saves_dir.join(".testerone.json.tmp");
        fs::write(&tmp, "{ half a wri").expect("stale");
        assert!(s.load("testerone").expect("load").is_some());
        let mut run2 = run.clone();
        run2.depth = 9;
        s.save(&run2).expect("save over stale");
        assert!(!tmp.exists());
        let names: Vec<_> = fs::read_dir(&s.saves_dir)
            .expect("dir")
            .map(|e| e.expect("e").file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["testerone.json".to_string()]);
        assert_eq!(s.load("testerone").expect("l").expect("s").depth, 9);
    }

    #[test]
    fn callers_newest_first_capped_and_replaced() {
        let (_d, s) = store();
        assert!(s.callers().is_empty());
        for i in 0..25 {
            s.record_caller(rec(&format!("c{i}"), i, CallerStatus::Online))
                .expect("rec");
        }
        let all = s.callers();
        assert_eq!(all.len(), MAX_CALLERS);
        assert_eq!(all[0].handle, "c24");
        assert_eq!(all[19].handle, "c5");

        s.record_caller(rec("C10", 77, CallerStatus::Online))
            .expect("rec");
        let all = s.callers();
        assert_eq!(all.len(), MAX_CALLERS);
        assert_eq!(all[0].handle, "C10");
        assert_eq!(all[0].depth, 77);
        assert_eq!(
            all.iter()
                .filter(|c| c.handle.eq_ignore_ascii_case("c10"))
                .count(),
            1
        );
        assert_eq!(s.last_callers(3).len(), 3);
        assert_eq!(s.last_callers(99).len(), MAX_CALLERS);
    }

    #[test]
    fn callers_file_format_and_corruption() {
        let (_d, s) = store();
        s.record_caller(rec("a", 1, CallerStatus::Ascended))
            .expect("rec");
        let v: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&s.callers_file).expect("read")).expect("j");
        assert_eq!(v["callers"][0]["status"], "ascended");
        assert_eq!(v["callers"][0]["handle"], "a");
        fs::write(&s.callers_file, "garbage").expect("w");
        assert!(s.callers().is_empty());
        // A corrupt log is replaced by the next record rather than blocking it.
        s.record_caller(rec("b", 2, CallerStatus::Online))
            .expect("rec");
        assert_eq!(s.callers().len(), 1);
    }

    #[test]
    fn ego_death_deletes_save_and_records_lost() {
        let (_d, s) = store();
        let run = rich_run();
        s.save(&run).expect("save");
        s.record_caller(rec("TesterOne", 1, CallerStatus::Online))
            .expect("rec");
        assert!(!s.is_dead("testerone"));
        s.ego_death(&run, "23:41:07").expect("death");
        assert!(!s.exists("TesterOne"));
        assert!(s.is_dead("TESTERONE"));
        let c = &s.callers()[0];
        assert_eq!(
            (c.depth, c.clock.as_str(), c.status),
            (3, "23:41:07", CallerStatus::Lost)
        );
        assert_eq!(s.callers().len(), 1);
    }

    #[test]
    fn ascend_deletes_save_and_records_ascended() {
        let (_d, s) = store();
        let run = rich_run();
        s.save(&run).expect("save");
        s.ascend(&run, "00:00:01").expect("ascend");
        assert!(!s.exists(&run.handle));
        assert_eq!(s.callers()[0].status, CallerStatus::Ascended);
        assert!(!s.is_dead(&run.handle));
        s.delete(&run.handle)
            .expect("deleting a missing save is fine");
    }

    #[test]
    fn path_traversal_handles_are_rejected() {
        let (d, s) = store();
        for h in ["../x", "a/b", "a\\b", "", "..", "x.json"] {
            let mut run = rich_run();
            run.handle = h.into();
            assert!(s.save(&run).is_err(), "{h:?}");
            assert!(s.load(h).is_err(), "{h:?}");
            assert!(s.delete(h).is_err(), "{h:?}");
            assert!(!s.exists(h));
        }
        assert!(!d.path().join("x.json").exists());
    }

    #[test]
    fn has_any_save_or_caller_truth_table() {
        let (_d, s) = store();
        assert!(!s.has_any_save_or_caller());
        s.save(&rich_run()).expect("save");
        assert!(s.has_any_save_or_caller());
        s.delete("testerone").expect("del");
        assert!(!s.has_any_save_or_caller());
        s.record_caller(rec("x", 0, CallerStatus::Lost))
            .expect("rec");
        assert!(s.has_any_save_or_caller());
        s.save(&rich_run()).expect("save");
        assert!(s.has_any_save_or_caller());
    }
}
