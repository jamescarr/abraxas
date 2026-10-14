//! `abraxas pack check` rules against the shipped packs and deliberately broken copies.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

use abraxas::pack::check::{check_source, run};
use abraxas::pack::model::{ExitKind, Stance};
use abraxas::pack::{PackIssue, PackSource, load};

fn root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("packs")
        .join(name)
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &dest);
        } else {
            fs::copy(entry.path(), dest).unwrap();
        }
    }
}

/// A scratch copy of `packs/<name>`.
fn scratch(name: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("pack");
    copy_dir(&root(name), &dir);
    (tmp, dir)
}

fn edit(dir: &Path, rel: &str, from: &str, to: &str) {
    let path = dir.join(rel);
    let text = fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "{rel} does not contain {from:?}");
    fs::write(path, text.replacen(from, to, 1)).unwrap();
}

fn issues(dir: &Path) -> Vec<String> {
    match check_source(&PackSource::Dir(dir.to_path_buf())) {
        Ok(s) => panic!("expected issues, got: {s}"),
        Err(v) => v.iter().map(PackIssue::to_string).collect(),
    }
}

#[test]
fn rollover_validates_with_expected_summary() {
    assert_eq!(
        check_source(&PackSource::Embedded).unwrap(),
        "pack rollover: OK (4 acts, 5 anchors, 7 characters, endless on)"
    );
    assert_eq!(
        check_source(&PackSource::Dir(root("rollover"))).unwrap(),
        "pack rollover: OK (4 acts, 5 anchors, 7 characters, endless on)"
    );
}

#[test]
fn template_validates() {
    let summary = check_source(&PackSource::Dir(root("_template"))).unwrap();
    assert!(summary.starts_with("pack template: OK ("), "{summary}");
}

#[test]
fn rollover_pool_meets_minimum_counts() {
    let pack = load(&PackSource::Embedded).unwrap();
    let p = &pack.pool;
    let counts = [
        ("titles", p.titles.len(), 40),
        ("bbs_names", p.bbs_names.len(), 30),
        ("taglines", p.taglines.len(), 30),
        ("level_cards", p.level_cards.len(), 20),
        ("sysops", p.sysops.len(), 20),
        ("host_names", p.host_names.len(), 20),
        ("node_names", p.node_names.len(), 40),
        ("ice_names", p.ice_names.len(), 20),
        ("daemons", p.daemons.len(), 12),
        ("interludes", p.interludes.len(), 20),
        ("beats", p.beats.len(), 20),
        ("captions", p.captions.len(), 30),
        ("post", p.post.len(), 40),
        ("text_file", p.text_file.len(), 20),
        ("host_file", p.host_file.len(), 20),
        ("choice", p.choice.len(), 12),
        ("words_5", p.words_5.len(), 40),
        ("words_6", p.words_6.len(), 40),
        ("words_7", p.words_7.len(), 40),
        ("words_8", p.words_8.len(), 40),
        ("eliza", p.eliza.len(), 6),
        ("eliza_fallback", p.eliza_fallback.len(), 8),
    ];
    for (name, have, need) in counts {
        assert!(have >= need, "{name}: {have} entries, need {need}");
    }
    for kind in ExitKind::ALL {
        let n = p.exits_of(kind).count();
        assert!(n >= 10, "exit {}: {n} entries, need 10", kind.as_str());
    }
    for (i, c) in p.choice.iter().enumerate() {
        for s in Stance::ALL {
            let n = c.options.iter().filter(|o| o.stance == s).count();
            assert_eq!(n, 1, "choice {} stance {}", i + 1, s.as_str());
        }
    }
}

#[test]
fn rollover_cast_has_enough_eliza() {
    let pack = load(&PackSource::Embedded).unwrap();
    for c in &pack.cast {
        assert!(
            c.eliza.len() >= 4,
            "{}: {} eliza groups",
            c.id,
            c.eliza.len()
        );
        assert!(c.eliza.iter().all(|g| g.replies.len() >= 3), "{}", c.id);
        assert!(
            c.fallback.len() >= 5,
            "{}: {} fallback lines",
            c.id,
            c.fallback.len()
        );
    }
}

#[test]
fn duplicate_anchor_depth_is_reported() {
    let (_t, dir) = scratch("rollover");
    edit(&dir, "anchors/03-black-sun.toml", "depth = 3", "depth = 0");
    let got = issues(&dir);
    assert!(
        got.contains(
            &"anchors/03-black-sun.toml: depth: depth 0 already used by anchors/00-the-lobby.toml"
                .to_string()
        ),
        "{got:#?}"
    );
}

#[test]
fn act_gap_is_reported() {
    let (_t, dir) = scratch("rollover");
    edit(&dir, "pack.toml", "depths = [4, 7]", "depths = [5, 7]");
    let got = issues(&dir);
    assert!(
        got.iter()
            .any(|i| i.starts_with("pack.toml: act[2].depths: gap: depths 4..4")),
        "{got:#?}"
    );
}

#[test]
fn act_overlap_is_reported() {
    let (_t, dir) = scratch("rollover");
    edit(&dir, "pack.toml", "depths = [4, 7]", "depths = [3, 7]");
    let got = issues(&dir);
    assert!(
        got.iter()
            .any(|i| i.starts_with("pack.toml: act[2].depths: overlaps")),
        "{got:#?}"
    );
}

#[test]
fn unknown_placeholder_is_reported() {
    let (_t, dir) = scratch("rollover");
    edit(
        &dir,
        "anchors/03-black-sun.toml",
        "tagline = \"It shines.",
        "tagline = \"{bogus} It shines.",
    );
    let got = issues(&dir);
    assert!(
        got.contains(
            &"anchors/03-black-sun.toml: tagline: unknown placeholder '{bogus}'".to_string()
        ),
        "{got:#?}"
    );
}

#[test]
fn pool_only_placeholders_are_rejected_elsewhere() {
    let (_t, dir) = scratch("rollover");
    edit(
        &dir,
        "anchors/03-black-sun.toml",
        "tagline = \"It shines.",
        "tagline = \"{motif} It shines.",
    );
    let got = issues(&dir);
    assert!(
        got.contains(
            &"anchors/03-black-sun.toml: tagline: unknown placeholder '{motif}'".to_string()
        ),
        "{got:#?}"
    );
}

#[test]
fn missing_ending_for_option_stance_is_reported() {
    let (_t, dir) = scratch("rollover");
    // Option 2 of the final choice is the repress option; retarget its ending.
    edit(
        &dir,
        "anchors/15-the-center.toml",
        "when_stance = \"repress\"",
        "when_stance = \"neutral\"",
    );
    let got = issues(&dir);
    assert!(
        got.contains(
            &"anchors/15-the-center.toml: ending: option 2 (repress) has no ending".to_string()
        ),
        "{got:#?}"
    );
}

#[test]
fn password_length_mismatch_is_reported() {
    let (_t, dir) = scratch("rollover");
    edit(
        &dir,
        "anchors/00-the-lobby.toml",
        "password = \"OWLET\"",
        "password = \"OWLETS\"",
    );
    let got = issues(&dir);
    assert!(
        got.iter()
            .any(|i| i
                .starts_with("anchors/00-the-lobby.toml: password: 'OWLETS' must be 5 letters")),
        "{got:#?}"
    );
}

#[test]
fn non_cp437_art_is_reported() {
    let (_t, dir) = scratch("rollover");
    let path = dir.join("art/wren.txt");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("\u{4DC0}{text}")).unwrap();
    let got = issues(&dir);
    assert!(
        got.iter()
            .any(|i| i.starts_with("art/wren.txt: art: character '\u{4DC0}'")
                && i.ends_with("is outside CP437")),
        "{got:#?}"
    );
}

#[test]
fn oversize_art_is_reported() {
    let (_t, dir) = scratch("rollover");
    let wide = format!("{}\n", "#".repeat(40));
    fs::write(dir.join("art/wren.txt"), wide).unwrap();
    let got = issues(&dir);
    assert!(
        got.contains(&"art/wren.txt: art: 40x1 exceeds the 36x16 limit".to_string()),
        "{got:#?}"
    );
}

#[test]
fn template_only_pack_inherits_the_rollover_pool() {
    let (_t, dir) = scratch("_template");
    assert!(!dir.join("pool.toml").exists());
    let custom = load(&PackSource::Dir(dir)).unwrap();
    let base = load(&PackSource::Embedded).unwrap();
    assert_eq!(custom.pool.titles, base.pool.titles);
    assert_eq!(custom.pool.words_7, base.pool.words_7);
    assert_eq!(custom.pool.post.len(), base.pool.post.len());
    assert_eq!(custom.pool.eliza_fallback, base.pool.eliza_fallback);
    assert_eq!(custom.pool.exit.len(), base.pool.exit.len());
}

#[test]
fn partial_pool_inherits_missing_lists_only() {
    let (_t, dir) = scratch("_template");
    fs::write(dir.join("pool.toml"), "titles = [\"ONLY ONE\"]\n").unwrap();
    let custom = load(&PackSource::Dir(dir)).unwrap();
    let base = load(&PackSource::Embedded).unwrap();
    assert_eq!(custom.pool.titles, vec!["ONLY ONE".to_string()]);
    assert_eq!(custom.pool.taglines, base.pool.taglines);
}

#[test]
fn pool_word_of_wrong_length_is_reported() {
    let (_t, dir) = scratch("_template");
    fs::write(
        dir.join("pool.toml"),
        "words_5 = [\"TOOLONG\", \"OKAYS\"]\n",
    )
    .unwrap();
    let got = issues(&dir);
    assert_eq!(
        got,
        vec!["pool.toml: words_5[1]: 'TOOLONG' must match ^[A-Z]{5}$".to_string()]
    );
}

#[test]
fn run_lists_every_error_and_exits_one() {
    let (_t, dir) = scratch("rollover");
    edit(&dir, "anchors/03-black-sun.toml", "depth = 3", "depth = 0");
    edit(
        &dir,
        "anchors/00-the-lobby.toml",
        "password = \"OWLET\"",
        "password = \"OWL\"",
    );
    let got = issues(&dir);
    assert!(
        got.iter().any(|i| i.contains("already used by")),
        "{got:#?}"
    );
    assert!(got.iter().any(|i| i.contains("password")), "{got:#?}");
    let code = run(Some(&dir)).unwrap();
    assert_eq!(
        format!("{code:?}"),
        format!("{:?}", std::process::ExitCode::from(1))
    );
}

#[test]
fn run_succeeds_on_valid_pack() {
    let code = run(Some(&root("rollover"))).unwrap();
    assert_eq!(
        format!("{code:?}"),
        format!("{:?}", std::process::ExitCode::SUCCESS)
    );
}

#[test]
fn parse_errors_are_listed_with_validation_errors_absent() {
    let (_t, dir) = scratch("rollover");
    fs::write(dir.join("anchors/03-black-sun.toml"), "depth = \"x\"\n").unwrap();
    fs::write(dir.join("cast.toml"), "[[character]\n").unwrap();
    let got = issues(&dir);
    assert!(
        got.iter()
            .any(|i| i.starts_with("anchors/03-black-sun.toml:")),
        "{got:#?}"
    );
    assert!(got.iter().any(|i| i.starts_with("cast.toml:")), "{got:#?}");
}
