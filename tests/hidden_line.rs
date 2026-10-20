//! The hidden line end to end over a real Unix socket: MOTD, zine directory, TAP, GET, reload
//! and hangup.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::sync::mpsc::channel;
use std::time::Duration;

use abraxas::game::danger::IceGrade;
use abraxas::game::programs::Program;
use abraxas::game::state::Backdoor;
use abraxas::hidden::board::{BoardSnapshot, Relay, ZineEntry, ZineKind};
use abraxas::hidden::{HiddenEvent, HiddenLine};
use abraxas::pack::model::HostOs;

fn snapshot(depth: u32, backdoor: Backdoor) -> BoardSnapshot {
    BoardSnapshot {
        depth,
        handle: "Tester".to_string(),
        bbs_name: "BAUDLINE".to_string(),
        title: "the quiet board".to_string(),
        sysop: "TESTSYS".to_string(),
        host_name: "HOST".to_string(),
        os: HostOs::Unix,
        relay: Some(Relay {
            id: "relay1".to_string(),
            display: "THE RELAY".to_string(),
            grade: IceGrade::Soft,
            tones: vec!['1', '2', '*'],
        }),
        backdoor,
        files: vec![
            ZineEntry {
                name: "DEMO1.ANS".to_string(),
                description: "inline art".to_string(),
                kind: ZineKind::Ans {
                    text: "\u{1b}[1;33mART\u{1b}[0m\r\n".to_string(),
                },
                owned: false,
                learned: false,
            },
            ZineEntry {
                name: "PACK.ANS".to_string(),
                description: "an artpack file".to_string(),
                kind: ZineKind::Ans {
                    text: "\u{1b}[31mhi\r\n".to_string(),
                },
                owned: false,
                learned: false,
            },
            ZineEntry {
                name: "WINNUKE.TXT".to_string(),
                description: "out of band".to_string(),
                kind: ZineKind::Article {
                    body: "the wire is yours.".to_string(),
                    attack: Program::WinNuke,
                },
                owned: false,
                learned: false,
            },
        ],
    }
}

fn connect(path: &Path) -> UnixStream {
    let stream = UnixStream::connect(path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    stream
}

fn read_until(stream: &mut UnixStream, needle: &str) -> String {
    let mut bytes: Vec<u8> = Vec::new();
    let mut buf = [0u8; 512];
    let mut timeouts = 0;
    while !String::from_utf8_lossy(&bytes).contains(needle) {
        match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut
                    || e.kind() == std::io::ErrorKind::Interrupted =>
            {
                timeouts += 1;
                if timeouts > 3 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

#[test]
fn the_hidden_line_serves_a_board() {
    let dir = tempfile::Builder::new().prefix("hl").tempdir().unwrap();
    let path = dir.path().join("s.sock");
    let (tx, rx) = channel();
    let mut line = HiddenLine::idle(tx, 0);
    line.listen(path.clone()).unwrap();

    // No board published yet.
    let mut early = connect(&path);
    assert!(read_until(&mut early, "NO CARRIER").contains("NO CARRIER"));
    drop(early);

    line.publish(Some(snapshot(0, Backdoor::Open { misses: 0 })));
    let mut caller = connect(&path);
    let motd = read_until(&mut caller, "> ");
    assert!(motd.contains("board   : BAUDLINE"), "{motd}");
    assert!(motd.contains("level 00"), "{motd}");
    assert!(motd.contains("relay   : relay1"), "{motd}");

    caller.write_all(b"DIR\n").unwrap();
    let listing = read_until(&mut caller, "> ");
    assert!(listing.contains(" DEMO1.ANS"), "{listing}");
    assert!(listing.contains(" PACK.ANS"), "{listing}");
    assert!(listing.contains(" WINNUKE.TXT"), "{listing}");
    assert!(listing.contains("ANS"), "{listing}");
    assert!(listing.contains("ZINE"), "{listing}");
    assert!(listing.contains("3 file(s)."), "{listing}");

    caller.write_all(b"TYPE PACK.ANS\n").unwrap();
    let typed = read_until(&mut caller, "> ");
    assert!(typed.contains("at modem speed"), "{typed}");
    assert!(typed.contains("\u{1b}[31mhi"), "{typed}");
    assert!(typed.contains("\u{1b}[0m\n"), "{typed}");

    caller.write_all(b"TYPE WINNUKE.TXT\n").unwrap();
    let learned = read_until(&mut caller, "> ");
    assert!(learned.contains("you learned WINNUKE"), "{learned}");

    caller.write_all(b"TAP\n").unwrap();
    let tap = read_until(&mut caller, "keys> ");
    assert!(tap.contains("*** tapping relay1"), "{tap}");
    assert!(tap.contains("tones:"), "{tap}");
    caller.write_all(b"9\n").unwrap();
    let wrong = read_until(&mut caller, "keys> ");
    assert!(wrong.contains("*** wrong. 2 attempt(s) left."), "{wrong}");

    // A refresh must not reset the session: the generation is unchanged and TAP mode survives.
    line.refresh(Some(snapshot(0, Backdoor::Open { misses: 1 })));
    std::thread::sleep(Duration::from_millis(400));
    caller.write_all(b"12*\n").unwrap();
    let planted = read_until(&mut caller, "> ");
    assert!(planted.contains("BACKDOOR PLANTED"), "{planted}");
    assert!(!planted.contains("unknown command"), "{planted}");

    caller.write_all(b"GET PACK.ANS\n").unwrap();
    let got = read_until(&mut caller, "> ");
    assert!(got.contains("saved to your files"), "{got}");

    let mut events = Vec::new();
    while let Ok(event) = rx.try_recv() {
        events.push(event);
    }
    assert!(
        events
            .iter()
            .any(|e| matches!(e, HiddenEvent::Learn(Program::WinNuke))),
        "{events:?}"
    );
    let pairs: Vec<&HiddenEvent> = events
        .iter()
        .filter(|e| matches!(e, HiddenEvent::Backdoor { .. }))
        .collect();
    assert!(pairs.len() >= 2, "{events:?}");
    assert_eq!(
        pairs[0],
        &HiddenEvent::Backdoor {
            depth: 0,
            state: Backdoor::Open { misses: 1 },
        }
    );
    assert_eq!(
        pairs[pairs.len() - 1],
        &HiddenEvent::Backdoor {
            depth: 0,
            state: Backdoor::Planted,
        }
    );
    let download = events
        .iter()
        .find_map(|e| match e {
            HiddenEvent::Download { file, .. } => Some(file),
            _ => None,
        })
        .expect("a download event");
    assert_eq!(download.name, "PACK.ANS");
    assert_eq!(download.body, "\u{1b}[31mhi\r\n");

    // A new board reloads the connected session.
    line.publish(Some(snapshot(1, Backdoor::Open { misses: 0 })));
    std::thread::sleep(Duration::from_millis(400));
    let changed = read_until(&mut caller, "> ");
    assert!(changed.starts_with("*** CARRIER CHANGED"), "{changed}");
    assert!(changed.contains("level 01"), "{changed}");

    caller.write_all(b"QUIT\n").unwrap();
    let mut tail: Vec<u8> = Vec::new();
    let mut closed = false;
    let mut chunk = [0u8; 256];
    for _ in 0..8 {
        match caller.read(&mut chunk) {
            Ok(0) => {
                closed = true;
                break;
            }
            Ok(n) => tail.extend_from_slice(&chunk[..n]),
            Err(_) => break,
        }
    }
    let tail = String::from_utf8_lossy(&tail);
    assert!(tail.contains("NO CARRIER"), "{tail}");
    assert!(closed, "the stream stayed open");

    let socket = line.path().unwrap().to_path_buf();
    drop(caller);
    drop(line);
    assert!(!socket.exists(), "dropping the line removes the socket");
}
