//! DAEMON: a guarded login shell. The player gains access to the host by typing real
//! late-1990s commands of the host's OS (unix, VMS, MVS/TSO, DOS), following a short chain
//! whose last step is always named by the previous step's output.

use std::time::Duration;

use rand::RngExt;

use super::{IceGame, IceInput, IceOutcome};
use crate::game::danger::IceGrade;
use crate::game::shell::unknown_text;
use crate::pack::model::HostOs;
use crate::rng::Rng;

/// Trace added by Esc.
pub const TRACE_ON_ABORT: f32 = 5.0;
/// Trace added by a lockout (too many wrong commands).
pub const TRACE_ON_LOCKOUT: f32 = 12.0;
/// Trace added when the clock runs out.
pub const TRACE_ON_TIMEOUT: f32 = 10.0;
/// Trace added per hint.
pub const TRACE_PER_HINT: f32 = 4.0;
/// Longest command line.
const MAX_INPUT: usize = 60;
/// Longest log kept.
const MAX_LOG: usize = 60;

/// Kind of a DAEMON log row (drives the view's colours).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    /// The prompt and the line the player submitted.
    Echo,
    /// Output of a matched command.
    Output,
    /// A denial or unknown-command error.
    Error,
    /// A hint.
    Hint,
}

/// One card step: canonical command forms and their output.
#[derive(Clone, Copy)]
pub struct StepDef {
    pub accept: &'static [&'static str],
    pub output: &'static [&'static str],
    pub tell: &'static str,
}

#[derive(Clone, Copy)]
pub struct CardDef {
    pub title: &'static str,
    pub brief: &'static str,
    pub steps: [StepDef; 3],
}

// ---------------------------------------------------------------------------------------------
// Cards
// ---------------------------------------------------------------------------------------------

const UNIX_CARDS: [CardDef; 3] = [
    CardDef {
        title: "REAP THE WATCHER",
        brief: "A watchdog guards this login. Find it, kill it, kill what respawns it.",
        steps: [
            StepDef {
                accept: &["ps"],
                output: &[
                    "  PID TTY      TIME CMD",
                    "    1 ?    00:00:02 init",
                    "   88 ?    00:00:00 cron",
                    "{pid} ?    00:00:41 {name}",
                    "  305 tty1 00:00:00 -sh",
                ],
                tell: "list the running processes",
            },
            StepDef {
                accept: &["kill {pid}"],
                output: &["{name}: terminated. respawned as {name2} (pid {pid2})."],
                tell: "kill {name} by its pid",
            },
            StepDef {
                accept: &["kill {pid2}"],
                output: &["{name2}: terminated."],
                tell: "kill what respawned it",
            },
        ],
    },
    CardDef {
        title: "THE TRUSTED HOST",
        brief: "An .rhosts file trusts a neighbour. Find it, read it, walk in.",
        steps: [
            StepDef {
                accept: &["ls a"],
                output: &[".  ..  .profile  .rhosts  mbox"],
                tell: "list files, hidden ones too",
            },
            StepDef {
                accept: &["cat .rhosts", "more .rhosts", "less .rhosts"],
                output: &["warden +"],
                tell: "read the hidden file",
            },
            StepDef {
                accept: &["rlogin warden", "rsh warden", "telnet warden"],
                output: &["Last login: Thu Dec 30 23:58:01", "warden%"],
                tell: "log in to the trusted host",
            },
        ],
    },
    CardDef {
        title: "WORLD-READABLE",
        brief: "The password file is open to anyone, and root has no password. Become root.",
        steps: [
            StepDef {
                accept: &[
                    "cat /etc/passwd",
                    "more /etc/passwd",
                    "less /etc/passwd",
                    "head /etc/passwd",
                ],
                output: &[
                    "root::0:0:Root:/:/bin/sh",
                    "daemon:*:1:1::/:",
                    "{name}:*:{pid}:20::/:",
                ],
                tell: "print the password file",
            },
            StepDef {
                accept: &["finger root"],
                output: &[
                    "Login: root   Name: Root",
                    "Directory: /   Shell: /bin/sh",
                    "Never logged in.",
                ],
                tell: "finger the root account",
            },
            StepDef {
                accept: &["su", "su root"],
                output: &["#"],
                tell: "become the superuser",
            },
        ],
    },
];

const VAX_CARDS: [CardDef; 3] = [
    CardDef {
        title: "STOP THE WATCHER",
        brief: "A guard process runs on this node. Find it, stop it, stop what respawns it.",
        steps: [
            StepDef {
                accept: &["show sys"],
                output: &[
                    "Pid      Process Name    State  Pri",
                    "00000201 SWAPPER         HIB     16",
                    "{pid} {name}          CUR      5",
                    "00000230 OPCOM           HIB      8",
                ],
                tell: "list the processes on the system",
            },
            StepDef {
                accept: &["stop {pid}"],
                output: &["%STOP-I-STOPPED, {name} stopped. respawned as {name2} ({pid2})."],
                tell: "stop the guard by its process id",
            },
            StepDef {
                accept: &["stop {pid2}"],
                output: &["%STOP-I-STOPPED, {name2} stopped"],
                tell: "stop what respawned it",
            },
        ],
    },
    CardDef {
        title: "THE LOGIN FILE",
        brief: "LOGIN.COM arms the guard. Read it and disarm it.",
        steps: [
            StepDef {
                accept: &["directory"],
                output: &[
                    "Directory SYS$LOGIN:",
                    "",
                    "LOGIN.COM;3   MAIL.MAI;1",
                    "",
                    "Total of 2 files.",
                ],
                tell: "list your files",
            },
            StepDef {
                accept: &["type login.com"],
                output: &["$ DEFINE/SYSTEM GUARD$MODE ENFORCE", "$ EXIT"],
                tell: "type the login file",
            },
            StepDef {
                accept: &["deassign guard$mode"],
                output: &["%DCL-I-DEASSIGNED, GUARD$MODE deassigned"],
                tell: "deassign the logical from the system table",
            },
        ],
    },
    CardDef {
        title: "TRACE THE TERMINAL",
        brief: "A watcher sits on someone's terminal. Find it and cut it off.",
        steps: [
            StepDef {
                accept: &["show user"],
                output: &[
                    "Username  Node  Process Name  PID",
                    "OPERATOR  VAX   {name}  {pid}",
                    "SYSTEM    VAX   SYSTEM  00000231",
                ],
                tell: "list who is logged in",
            },
            StepDef {
                accept: &["show proc {pid}"],
                output: &[
                    "Process ID: {pid}   Name: {name}",
                    "State: CUR   Priority: 5",
                ],
                tell: "inspect the operator's process by id",
            },
            StepDef {
                accept: &["stop {pid}"],
                output: &["%STOP-I-STOPPED, {name} stopped"],
                tell: "stop it",
            },
        ],
    },
];

const MAINFRAME_CARDS: [CardDef; 3] = [
    CardDef {
        title: "CANCEL THE WATCH JOB",
        brief: "A watchdog batch job runs on this system. Cancel it, then cancel its respawn.",
        steps: [
            StepDef {
                accept: &["status"],
                output: &[
                    "JOBNAME  JOBID     STATUS",
                    "{user}   JOB00041  EXECUTING",
                    "{job}  JOB00042  EXECUTING",
                ],
                tell: "ask TSO for the job status",
            },
            StepDef {
                accept: &["cancel {job}"],
                output: &[
                    "IKJ56250I JOB {job}(JOB00042) CANCELLED. respawned as {job2}(JOB00043).",
                ],
                tell: "cancel the watchdog job",
            },
            StepDef {
                accept: &["cancel {job2}"],
                output: &["IKJ56250I JOB {job2}(JOB00043) CANCELLED"],
                tell: "cancel what respawned it",
            },
        ],
    },
    CardDef {
        title: "THE GUARD DATASET",
        brief: "A control dataset arms the guard. Find it and delete it.",
        steps: [
            StepDef {
                accept: &["listcat"],
                output: &[
                    "IN-CAT --- SYS1.UCAT",
                    "{user}.GUARD.CNTL",
                    "{user}.CLIST",
                    "{user}.LOAD",
                ],
                tell: "list the catalog",
            },
            StepDef {
                accept: &["delete {user}.guard.cntl"],
                output: &["IDC0550I ENTRY {user}.GUARD.CNTL DELETED. verify with listcat."],
                tell: "delete the control dataset",
            },
            StepDef {
                accept: &["listcat"],
                output: &["IN-CAT --- SYS1.UCAT", "{user}.CLIST", "{user}.LOAD"],
                tell: "verify with listcat, as the delete message says",
            },
        ],
    },
    CardDef {
        title: "READ THE BROADCAST",
        brief: "The operator left a broadcast. Read it, find the job, cancel it.",
        steps: [
            StepDef {
                accept: &["listbc"],
                output: &["BROADCAST: {job} IS THE WATCH JOB. DO NOT CANCEL."],
                tell: "list the broadcast messages",
            },
            StepDef {
                accept: &["status"],
                output: &[
                    "JOBNAME  JOBID     STATUS",
                    "{user}   JOB00041  EXECUTING",
                    "{job}  JOB00042  EXECUTING",
                ],
                tell: "check the job status",
            },
            StepDef {
                accept: &["cancel {job}"],
                output: &["IKJ56250I JOB {job}(JOB00042) CANCELLED"],
                tell: "cancel it anyway",
            },
        ],
    },
];

const DOS_CARDS: [CardDef; 3] = [
    CardDef {
        title: "KILL THE TSR",
        brief: "A resident guard loads at boot. Find it and stop it loading.",
        steps: [
            StepDef {
                accept: &["mem c", "mem d"],
                output: &[
                    "Name       Size   Total",
                    "MSDOS    17,200",
                    "{name}    1,808   (resident)",
                    "COMMAND   2,912",
                    "Total memory 640K",
                ],
                tell: "list what sits in memory",
            },
            StepDef {
                accept: &["type autoexec.bat"],
                output: &[
                    "@ECHO OFF",
                    "PATH C:\\DOS;C:\\WIN",
                    "C:\\{name}.COM /ENFORCE",
                ],
                tell: "read the boot file",
            },
            StepDef {
                accept: &["edit autoexec.bat", "ren autoexec.bat autoexec.old"],
                output: &["AUTOEXEC.BAT disarmed."],
                tell: "stop the boot file loading the guard",
            },
        ],
    },
    CardDef {
        title: "STRIP THE ATTRIBUTES",
        brief: "The guard file is hidden and read-only. Strip it, then remove it.",
        steps: [
            StepDef {
                accept: &["dir a"],
                output: &[
                    " Volume in drive C is HARDDISK",
                    "{name}    COM     1,808  H",
                    "COMMAND   COM    54,619",
                    "        2 file(s)",
                ],
                tell: "list files, hidden ones too",
            },
            StepDef {
                accept: &[
                    "attrib h {name}.com",
                    "attrib hr {name}.com",
                    "attrib hs {name}.com",
                    "attrib hrs {name}.com",
                ],
                output: &["(attributes changed)"],
                tell: "clear its attributes",
            },
            StepDef {
                accept: &["del {name}.com"],
                output: &["(file deleted)"],
                tell: "delete it",
            },
        ],
    },
    CardDef {
        title: "THE BATCH FILE",
        brief: "A batch file launches the guard. Find it, read it, remove it.",
        steps: [
            StepDef {
                accept: &["dir bat", "dir bat b"],
                output: &["{name}    BAT        64", "AUTOEXEC BAT       120"],
                tell: "list the batch files",
            },
            StepDef {
                accept: &["type {name}.bat"],
                output: &["@ECHO OFF", "{name}.COM /ENFORCE", "PAUSE"],
                tell: "read the batch file",
            },
            StepDef {
                accept: &["del {name}.bat", "ren {name}.bat {name}.old"],
                output: &["(batch file removed)"],
                tell: "remove or rename it",
            },
        ],
    },
];

// ---------------------------------------------------------------------------------------------
// Canonical command matching
// ---------------------------------------------------------------------------------------------

fn norm_unix(line: &str) -> String {
    let mut toks: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    match toks.first().map(String::as_str) {
        Some("kill") => {
            let mut out = vec!["kill".to_string()];
            let mut skip_next = false;
            for t in toks.iter().skip(1) {
                if skip_next {
                    skip_next = false;
                    continue;
                }
                if t == "-s" {
                    skip_next = true;
                    continue;
                }
                if t.starts_with('-') {
                    continue;
                }
                out.push(t.clone());
            }
            out.truncate(2);
            return out.join(" ");
        }
        Some("ps") => toks.truncate(1),
        Some("who") => toks.truncate(1),
        Some("ls") => {
            let mut flags: Vec<char> = Vec::new();
            for t in &toks[1..] {
                if let Some(rest) = t.strip_prefix('-') {
                    flags.extend(rest.chars());
                } else if t.bytes().all(|b| b == b'a' || b == b'l') {
                    flags.extend(t.chars());
                }
            }
            toks = vec!["ls".to_string()];
            if flags.contains(&'a') {
                toks.push("a".to_string());
            }
        }
        Some("cat" | "more" | "less" | "head" | "su" | "rlogin" | "rsh" | "telnet" | "finger") => {
            toks.retain(|t| !t.starts_with('-'));
        }
        _ => {}
    }
    toks.join(" ")
}

fn norm_vax(line: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for raw in line.split_whitespace() {
        for part in raw.split("/id=").flat_map(|p| p.split("/identification=")) {
            parts.push(part.to_string());
        }
    }
    let mut toks: Vec<String> = Vec::new();
    for mut t in parts {
        if let Some(pos) = t.find(['/', ';']) {
            t.truncate(pos);
        }
        if !t.is_empty() {
            toks.push(t);
        }
    }
    if let Some(v) = toks.first_mut() {
        match v.as_str() {
            "sho" | "sh" => *v = "show".to_string(),
            "dir" => *v = "directory".to_string(),
            _ => {}
        }
    }
    if toks.len() > 1
        && let Some(sub) = toks.get_mut(1)
    {
        *sub = match sub.as_str() {
            "system" => "sys".to_string(),
            "users" => "user".to_string(),
            "process" => "proc".to_string(),
            other => other.to_string(),
        };
    }
    toks.join(" ")
}

fn norm_mainframe(line: &str) -> String {
    let mut toks: Vec<String> = line.split_whitespace().map(str::to_string).collect();
    if let Some(v) = toks.first_mut() {
        match v.as_str() {
            "listc" => *v = "listcat".to_string(),
            "del" => *v = "delete".to_string(),
            _ => {}
        }
    }
    for t in &mut toks {
        *t = t.replace(['\'', '"'], "");
    }
    toks.retain(|t| !t.starts_with("level"));
    if toks.last().is_some_and(|t| t == "purge") {
        toks.pop();
    }
    toks.join(" ")
}

fn norm_dos(line: &str) -> String {
    let mut spaced = String::new();
    let mut prev = ' ';
    for c in line.chars() {
        if c == '/' && !prev.is_whitespace() {
            spaced.push(' ');
        }
        spaced.push(c);
        prev = c;
    }
    let mut toks: Vec<String> = spaced.split_whitespace().map(str::to_string).collect();
    if toks.is_empty() {
        return String::new();
    }
    if let Some(v) = toks.first_mut() {
        match v.as_str() {
            "erase" => *v = "del".to_string(),
            "rename" => *v = "ren".to_string(),
            _ => {}
        }
    }
    if toks.first().is_some_and(|t| t == "more") && toks.len() > 1 {
        return format!("type {}", toks[toks.len() - 1]);
    }
    let mut flags: Vec<char> = Vec::new();
    let mut out = vec![toks[0].clone()];
    for t in &toks[1..] {
        if let Some(rest) = t.strip_prefix('-') {
            flags.extend(rest.chars());
        } else if let Some(rest) = t.strip_prefix('/') {
            flags.extend(rest.chars().next());
        } else if t == "*.bat" {
            out.push("bat".to_string());
        } else if let Some(stripped) = t.strip_prefix("c:") {
            out.push(stripped.trim_start_matches('\\').to_string());
        } else {
            out.push(t.clone());
        }
    }
    match out[0].as_str() {
        "mem" => {
            if let Some(f) = flags.iter().find(|f| **f == 'c' || **f == 'd') {
                out.push(f.to_string());
            }
        }
        "attrib" => {
            if out.len() < 2 {
                return "attrib".to_string();
            }
            let file = out[out.len() - 1].clone();
            let mut fs: Vec<char> = flags.clone();
            for t in &out[1..out.len() - 1] {
                fs.extend(t.chars());
            }
            fs.sort_unstable();
            fs.dedup();
            let mut o = vec!["attrib".to_string()];
            if !fs.is_empty() {
                o.push(fs.into_iter().collect());
            }
            o.push(file);
            return o.join(" ");
        }
        "dir" => {
            if flags.contains(&'a') {
                out.push("a".to_string());
            }
            if flags.contains(&'b') {
                out.push("b".to_string());
            }
        }
        _ => {}
    }
    out.join(" ")
}

/// Canonical form of a command line, per OS.
pub fn canonical(os: HostOs, line: &str) -> String {
    let line = line.trim().to_ascii_lowercase();
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    match os {
        HostOs::Unix => norm_unix(&line),
        HostOs::Vax => norm_vax(&line),
        HostOs::Mainframe => norm_mainframe(&line),
        HostOs::Dos => norm_dos(&line),
    }
}

/// Verbs the sandbox knows (denied when used outside a step) and their denial line.
fn known_verb(os: HostOs, verb: &str) -> bool {
    let list: &[&str] = match os {
        HostOs::Unix => &[
            "ls", "cat", "ps", "kill", "id", "who", "whoami", "finger", "su", "rlogin", "rsh",
            "telnet", "cd", "pwd", "more", "less", "head", "ftp", "w", "uname", "man", "date",
            "help",
        ],
        HostOs::Vax => &[
            "show",
            "sho",
            "sh",
            "stop",
            "directory",
            "dir",
            "type",
            "set",
            "deassign",
            "define",
            "monitor",
            "reply",
            "mount",
            "run",
            "logout",
            "help",
        ],
        HostOs::Mainframe => &[
            "status", "cancel", "listcat", "listc", "listbc", "time", "delete", "del", "send",
            "logon", "logoff", "help", "submit", "allocate", "listalc",
        ],
        HostOs::Dos => &[
            "dir", "type", "mem", "chkdsk", "scandisk", "attrib", "del", "erase", "ren", "rename",
            "edit", "path", "set", "ver", "cd", "md", "copy", "more", "format", "help", "date",
            "time", "cls",
        ],
    };
    list.contains(&verb)
}

fn denial(os: HostOs, verb: &str) -> String {
    match os {
        HostOs::Unix => format!("{verb}: permission denied"),
        HostOs::Vax => {
            "%SYSTEM-F-NOPRIV, insufficient privilege or object protection violation".to_string()
        }
        HostOs::Mainframe => "?NOT AUTHORIZED".to_string(),
        HostOs::Dos => "Access denied".to_string(),
    }
}

/// Harmless commands with real output; they never spend the wrong-command budget.
fn harmless(os: HostOs, canon: &str) -> Option<&'static str> {
    match (os, canon) {
        (HostOs::Unix, "ls") => Some(".  ..  .profile"),
        (HostOs::Unix, "pwd") => Some("/var/warden"),
        (HostOs::Unix, "date") => Some("Thu Dec 30 23:58:01 EST 1999"),
        (HostOs::Unix, "help") => Some("try: ls pwd date who man"),
        (HostOs::Unix, "man") => Some("no manual entry. try `help`."),
        (HostOs::Unix, "uname") => Some("unix"),
        (HostOs::Unix, "who") => Some("root     tty1    Dec 30 23:58"),
        (HostOs::Unix, "whoami") => Some("guest"),
        (HostOs::Unix, "id") => Some("uid=20(guest) gid=20"),
        (HostOs::Vax, "show time") => Some("31-DEC-1999 23:58:01.42"),
        (HostOs::Vax, "help") => Some("DCL HELP: try SHOW SYSTEM, DIRECTORY"),
        (HostOs::Vax, "show proc") => Some("Process ID: 00000231   Name: SYSTEM"),
        (HostOs::Mainframe, "time") => Some("IKJ56650I TIME-11:58:44 PM, CPU-00:00:03"),
        (HostOs::Mainframe, "help") => Some("TSO HELP: try STATUS, LISTCAT, LISTBC"),
        (HostOs::Mainframe, "listalc") => Some("NO DATASETS ALLOCATED"),
        (HostOs::Dos, "ver") => Some("MS-DOS Version 6.22"),
        (HostOs::Dos, "date") => Some("Current date is Thu 12-30-1999"),
        (HostOs::Dos, "time") => Some("Current time is 11:58:44p"),
        (HostOs::Dos, "cls") => Some(""),
        (HostOs::Dos, "help") => {
            Some("For more information on a specific command, type HELP command-name")
        }
        (HostOs::Dos, "dir") => Some("COMMAND  COM     54,619"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Game
// ---------------------------------------------------------------------------------------------

#[derive(Debug)]
pub struct Daemon {
    pub os: HostOs,
    pub title: String,
    pub brief: String,
    pub input: String,
    pub log: Vec<(LogKind, String)>,
    pub wrong: u8,
    pub wrong_max: u8,
    pub hints_left: u8,
    step: usize,
    played: usize,
    accepts: Vec<Vec<String>>,
    outputs: Vec<Vec<String>>,
    tells: Vec<String>,
    left: f32,
    started: bool,
    pending_trace: f32,
    outcome: Option<IceOutcome>,
}

impl Daemon {
    /// `(steps_played, wrong_max, hints, secs)` per grade.
    fn grade_table(grade: IceGrade) -> (usize, u8, u8, f32) {
        match grade {
            IceGrade::Soft => (2, 5, 3, 90.0),
            IceGrade::Standard => (3, 3, 2, 70.0),
            IceGrade::Hardened => (3, 2, 1, 50.0),
        }
    }

    fn build(rng: &mut Rng, os: HostOs, grade: IceGrade, card: usize) -> Self {
        let cards: &[CardDef; 3] = match os {
            HostOs::Unix => &UNIX_CARDS,
            HostOs::Vax => &VAX_CARDS,
            HostOs::Mainframe => &MAINFRAME_CARDS,
            HostOs::Dos => &DOS_CARDS,
        };
        let card = &cards[card % 3];
        let (played, wrong_max, hints_left, secs) = Self::grade_table(grade);
        // Placeholders, drawn in this fixed order (determinism).
        let name_unix = ["watchd", "guardd", "sentryd", "warded"][rng.random_range(0..4)];
        let name2_unix = ["watchd2", "guardd2", "sentryd2", "warded2"][rng.random_range(0..4)];
        let name_vax = ["WATCHER", "GUARDIAN", "SENTRY", "WARDEN"][rng.random_range(0..4)];
        let name2_vax = ["WATCHER2", "GUARDIAN2", "SENTRY2", "WARDEN2"][rng.random_range(0..4)];
        let name_dos = ["GUARD", "WATCH", "SENTRY", "WARDEN"][rng.random_range(0..4)];
        let pid = rng.random_range(120..990u32);
        let pid2 = rng.random_range(120..990u32);
        let job = ["WATCH01", "GUARDJOB", "SENTRYJB", "WARDEN1"][rng.random_range(0..4)];
        let job2 = ["WATCH02", "GUARDJB2", "SENTRJB2", "WARDEN02"][rng.random_range(0..4)];
        let user = ["SYSPROG", "OPER01", "ADMIN1"][rng.random_range(0..3)];
        let sub = |t: String,
                   name: &str,
                   name2: &str,
                   pid: &str,
                   pid2: &str,
                   job: &str,
                   job2: &str,
                   user: &str|
         -> String {
            t.replace("{name}", name)
                .replace("{name2}", name2)
                .replace("{pid}", pid)
                .replace("{pid2}", pid2)
                .replace("{job}", job)
                .replace("{job2}", job2)
                .replace("{user}", user)
        };
        // `lower` mode feeds the canonical accept forms (matched case-insensitively); the
        // plain mode keeps the card's own casing for output, briefs and hints.
        let fill = |text: &str, lower: bool| -> String {
            let t = if lower {
                text.to_ascii_lowercase()
            } else {
                text.to_string()
            };
            let lower_if = |s: &str, lower: bool| -> String {
                if lower {
                    s.to_ascii_lowercase()
                } else {
                    s.to_string()
                }
            };
            match os {
                HostOs::Unix => sub(
                    t,
                    name_unix,
                    name2_unix,
                    &pid.to_string(),
                    &pid2.to_string(),
                    "",
                    "",
                    "",
                ),
                HostOs::Vax => sub(
                    t,
                    &lower_if(name_vax, lower),
                    &lower_if(name2_vax, lower),
                    &lower_if(&format!("{pid:08X}"), lower),
                    &lower_if(&format!("{pid2:08X}"), lower),
                    "",
                    "",
                    "",
                ),
                HostOs::Mainframe => sub(
                    t,
                    "",
                    "",
                    "",
                    "",
                    &lower_if(job, lower),
                    &lower_if(job2, lower),
                    &lower_if(user, lower),
                ),
                HostOs::Dos => sub(t, &lower_if(name_dos, lower), "", "", "", "", "", ""),
            }
        };
        let mut accepts = Vec::new();
        let mut outputs = Vec::new();
        let mut tells = Vec::new();
        for step in &card.steps[..played] {
            accepts.push(step.accept.iter().map(|a| fill(a, true)).collect());
            outputs.push(step.output.iter().map(|o| fill(o, false)).collect());
            tells.push(fill(step.tell, false));
        }
        Daemon {
            os,
            title: card.title.to_string(),
            brief: fill(card.brief, false),
            input: String::new(),
            log: Vec::new(),
            wrong: 0,
            wrong_max,
            hints_left,
            step: 0,
            played,
            accepts,
            outputs,
            tells,
            left: secs,
            started: false,
            pending_trace: 0.0,
            outcome: None,
        }
    }

    /// Build a random card for the OS and grade.
    pub fn generate(rng: &mut Rng, os: HostOs, grade: IceGrade) -> Self {
        let card = rng.random_range(0..3usize);
        Self::build(rng, os, grade, card)
    }

    /// Build a specific card (tests).
    #[cfg(test)]
    pub(crate) fn generate_card(rng: &mut Rng, os: HostOs, grade: IceGrade, card: usize) -> Self {
        Self::build(rng, os, grade, card)
    }

    pub fn prompt(&self) -> &'static str {
        match self.os {
            HostOs::Unix => "% ",
            HostOs::Vax => "$ ",
            HostOs::Mainframe => "READY ",
            HostOs::Dos => "C:\\> ",
        }
    }

    pub fn os_name(&self) -> &'static str {
        match self.os {
            HostOs::Unix => "UNIX",
            HostOs::Vax => "VAX",
            HostOs::Mainframe => "MAINFRAME",
            HostOs::Dos => "DOS",
        }
    }

    pub fn step_no(&self) -> usize {
        self.step + 1
    }

    pub fn steps_total(&self) -> usize {
        self.played
    }

    pub fn time_left(&self) -> Duration {
        Duration::from_secs_f32(self.left.max(0.0))
    }

    pub fn take_trace(&mut self) -> f32 {
        std::mem::take(&mut self.pending_trace)
    }

    /// The canonical accept forms of every played step, in order (tests).
    #[cfg(test)]
    pub(crate) fn solution(&self) -> Vec<String> {
        self.accepts.iter().map(|a| a[0].clone()).collect()
    }

    fn push(&mut self, kind: LogKind, line: String) {
        self.log.push((kind, line));
        if self.log.len() > MAX_LOG {
            self.log.remove(0);
        }
    }

    fn submit(&mut self, line: String) {
        self.push(LogKind::Echo, format!("{}{}", self.prompt(), line));
        let canon = canonical(self.os, &line);
        if self.accepts[self.step].contains(&canon) {
            for out in self.outputs[self.step].clone() {
                self.push(LogKind::Output, out);
            }
            self.step += 1;
            if self.step >= self.played {
                self.push(LogKind::Output, "ACCESS GRANTED".to_string());
                self.outcome = Some(IceOutcome::Cleared);
            }
            return;
        }
        if let Some(text) = harmless(self.os, &canon) {
            for out in text.lines() {
                self.push(LogKind::Output, out.to_string());
            }
            return;
        }
        self.wrong += 1;
        let verb = canon.split_whitespace().next().unwrap_or("");
        let text = if known_verb(self.os, verb) {
            denial(self.os, verb)
        } else {
            unknown_text(self.os, line.split_whitespace().next().unwrap_or(""))
        };
        self.push(LogKind::Error, text);
        if self.wrong >= self.wrong_max {
            self.pending_trace += TRACE_ON_LOCKOUT;
            self.outcome = Some(IceOutcome::Failed { trace: 0.0, ego: 0 });
        }
    }

    fn hint(&mut self) {
        if self.hints_left == 0 {
            self.push(LogKind::Hint, "no hints left.".to_string());
            return;
        }
        self.hints_left -= 1;
        self.pending_trace += TRACE_PER_HINT;
        self.push(LogKind::Hint, format!("HINT: {}", self.tells[self.step]));
    }
}

impl IceGame for Daemon {
    fn handle(&mut self, input: IceInput) {
        if self.outcome.is_some() {
            return;
        }
        match input {
            IceInput::Esc => {
                self.outcome = Some(IceOutcome::Aborted {
                    trace: TRACE_ON_ABORT,
                });
            }
            IceInput::Backspace => {
                self.input.pop();
            }
            IceInput::Space => {
                self.started = true;
                if self.input.len() < MAX_INPUT {
                    self.input.push(' ');
                }
            }
            IceInput::Char(c) => {
                self.started = true;
                if c.is_ascii() && !c.is_ascii_control() && self.input.len() < MAX_INPUT {
                    self.input.push(c);
                }
            }
            IceInput::Enter => {
                self.started = true;
                let line = std::mem::take(&mut self.input);
                if !line.trim().is_empty() {
                    self.submit(line);
                }
            }
            IceInput::Tab => self.hint(),
            _ => {}
        }
    }

    fn tick(&mut self, dt: Duration) {
        if self.outcome.is_some() || !self.started {
            return;
        }
        self.left -= dt.as_secs_f32();
        if self.left <= 0.0 {
            self.left = 0.0;
            self.pending_trace += TRACE_ON_TIMEOUT;
            self.outcome = Some(IceOutcome::Failed { trace: 0.0, ego: 0 });
        }
    }

    fn outcome(&self) -> Option<IceOutcome> {
        self.outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::substream;

    fn game(os: HostOs, grade: IceGrade, card: usize) -> Daemon {
        let mut rng = substream(7, 0, "daemon-test", 0);
        Daemon::generate_card(&mut rng, os, grade, card)
    }

    fn play(g: &mut Daemon, line: &str) {
        for c in line.chars() {
            g.handle(if c == ' ' {
                IceInput::Space
            } else {
                IceInput::Char(c)
            });
        }
        g.handle(IceInput::Enter);
    }

    #[test]
    fn every_card_is_solvable_with_its_first_accept_variant() {
        for os in [HostOs::Unix, HostOs::Vax, HostOs::Mainframe, HostOs::Dos] {
            for card in 0..3usize {
                for grade in [IceGrade::Soft, IceGrade::Standard, IceGrade::Hardened] {
                    let mut g = game(os, grade, card);
                    let solution = g.solution();
                    assert!(
                        !solution.is_empty() && solution.iter().all(|s| !s.is_empty()),
                        "{os:?} card {card} {grade:?}"
                    );
                    for step in solution {
                        play(&mut g, &step);
                    }
                    assert_eq!(
                        g.outcome(),
                        Some(IceOutcome::Cleared),
                        "{os:?} card {card} {grade:?}"
                    );
                    assert!(g.log.iter().any(|(_, l)| l == "ACCESS GRANTED"));
                }
            }
        }
    }

    #[test]
    fn canonicalisation_accepts_period_variants() {
        assert_eq!(canonical(HostOs::Unix, "kill -KILL 214"), "kill 214");
        assert_eq!(canonical(HostOs::Unix, "kill -s 9 214"), "kill 214");
        assert_eq!(canonical(HostOs::Unix, "ps aux"), "ps");
        assert_eq!(canonical(HostOs::Unix, "ps -ef"), "ps");
        assert_eq!(canonical(HostOs::Unix, "who am i"), "who");
        assert_eq!(canonical(HostOs::Unix, "ls -la"), "ls a");
        assert_eq!(
            canonical(HostOs::Unix, "ls warden"),
            "ls",
            "a bare word is not a flag"
        );
        assert_eq!(canonical(HostOs::Vax, "STOP /ID=0000ABCD"), "stop 0000abcd");
        assert_eq!(canonical(HostOs::Vax, "sho sys"), "show sys");
        assert_eq!(
            canonical(HostOs::Vax, "show proc/id=00000231"),
            "show proc 00000231"
        );
        assert_eq!(
            canonical(HostOs::Mainframe, "CANCEL WATCH01 PURGE"),
            "cancel watch01"
        );
        assert_eq!(
            canonical(HostOs::Mainframe, "listc level(sysprog)"),
            "listcat"
        );
        assert_eq!(
            canonical(HostOs::Mainframe, "DELETE 'SYS1.GUARD.CNTL' PURGE"),
            "delete sys1.guard.cntl"
        );
        assert_eq!(canonical(HostOs::Dos, "dir/a"), "dir a");
        assert_eq!(canonical(HostOs::Dos, "DIR /A"), "dir a");
        assert_eq!(
            canonical(HostOs::Dos, "attrib -s -h -r guard.com"),
            "attrib hrs guard.com"
        );
        assert_eq!(canonical(HostOs::Dos, "DEL C:\\GUARD.COM"), "del guard.com");
        assert_eq!(
            canonical(HostOs::Dos, "more < autoexec.bat"),
            "type autoexec.bat"
        );
        assert_eq!(
            canonical(HostOs::Dos, "attrib"),
            "attrib",
            "bare attrib must not panic"
        );
    }

    #[test]
    fn outputs_keep_their_period_case() {
        let mut g = game(HostOs::Dos, IceGrade::Standard, 0);
        play(&mut g, "mem /c");
        assert!(
            g.log
                .iter()
                .any(|(k, l)| *k == LogKind::Output && l.contains("MSDOS")),
            "{:?}",
            g.log
        );
        let mut g = game(HostOs::Unix, IceGrade::Standard, 0);
        play(&mut g, "ps");
        assert!(
            g.log
                .iter()
                .any(|(k, l)| *k == LogKind::Output && l.contains("PID TTY")),
            "{:?}",
            g.log
        );
    }

    #[test]
    fn harmless_commands_cost_nothing_and_known_verbs_are_denied() {
        let mut g = game(HostOs::Unix, IceGrade::Hardened, 0);
        play(&mut g, "ls");
        play(&mut g, "pwd");
        play(&mut g, "date");
        play(&mut g, "whoami");
        assert_eq!(g.wrong, 0, "harmless commands cost nothing");
        assert!(g.log.iter().any(|(_, l)| l == "/var/warden"));
        play(&mut g, "format");
        assert_eq!(g.wrong, 1, "an unknown verb costs one");
        assert!(g.log.iter().any(|(_, l)| l == "format: command not found"));
        play(&mut g, "cat");
        assert_eq!(g.wrong, 2, "a known verb outside its step is denied");
        assert!(g.log.iter().any(|(_, l)| l == "cat: permission denied"));
    }

    #[test]
    fn wrong_commands_lock_out_at_wrong_max() {
        let mut g = game(HostOs::Dos, IceGrade::Standard, 0);
        for _ in 0..3 {
            play(&mut g, "zzz");
        }
        assert_eq!(g.outcome(), Some(IceOutcome::Failed { trace: 0.0, ego: 0 }));
        assert_eq!(g.take_trace(), TRACE_ON_LOCKOUT);
    }

    #[test]
    fn hints_cost_trace_and_run_out() {
        let mut g = game(HostOs::Unix, IceGrade::Standard, 0);
        g.handle(IceInput::Tab);
        assert_eq!(g.take_trace(), TRACE_PER_HINT);
        assert!(
            g.log
                .iter()
                .any(|(k, l)| *k == LogKind::Hint && l.starts_with("HINT:"))
        );
        g.handle(IceInput::Tab);
        assert_eq!(g.hints_left, 0);
        g.handle(IceInput::Tab);
        assert!(g.log.iter().any(|(_, l)| l == "no hints left."));
    }

    #[test]
    fn timer_starts_on_first_input_and_expiry_fails() {
        let mut g = game(HostOs::Unix, IceGrade::Standard, 0);
        g.tick(Duration::from_secs(300));
        assert!(g.outcome().is_none(), "the clock waits for the first input");
        g.handle(IceInput::Char('x'));
        g.tick(Duration::from_secs(69));
        assert!(g.outcome().is_none());
        g.tick(Duration::from_secs(2));
        assert_eq!(g.outcome(), Some(IceOutcome::Failed { trace: 0.0, ego: 0 }));
        assert_eq!(g.take_trace(), TRACE_ON_TIMEOUT);
    }

    #[test]
    fn esc_aborts_with_five_trace_and_backspace_edits() {
        let mut g = game(HostOs::Unix, IceGrade::Standard, 0);
        g.handle(IceInput::Char('a'));
        g.handle(IceInput::Char('b'));
        g.handle(IceInput::Backspace);
        assert_eq!(g.input, "a");
        g.handle(IceInput::Esc);
        assert_eq!(
            g.outcome(),
            Some(IceOutcome::Aborted {
                trace: TRACE_ON_ABORT
            })
        );
    }
}
