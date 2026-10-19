//! The board: a TCP listener with one line. A caller gets a game; everyone else gets LINE BUSY.

use std::any::Any;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Context;

use super::session;
use crate::cli::GlobalOpts;
use crate::config::{Config, Paths};
use crate::llm::{Llm, OpenAiCompat};
use crate::ui::app::Ctx;

/// What a caller reads when the line is taken.
pub const BUSY: &[u8] = b"\r\nLINE BUSY\r\nNO CARRIER\r\n";

/// How long the accept loop waits after a failed `accept` (descriptors exhausted, say) so it does
/// not spin.
const ACCEPT_RETRY: Duration = Duration::from_millis(250);

/// Open the pack, bind `listen` and serve callers until the process is killed. Pack and bind
/// errors are returned before anything is accepted.
pub fn run(global: &GlobalOpts, config: Config, paths: Paths, listen: &str) -> anyhow::Result<()> {
    let pack = match crate::pack::open_for_run(global.pack.as_deref()) {
        Ok(p) => Arc::new(p),
        Err(errors) => {
            eprintln!("{errors}");
            anyhow::bail!("the story pack has errors");
        }
    };
    let llm: Option<Arc<dyn Llm>> = config
        .ai
        .enabled
        .then(|| Arc::new(OpenAiCompat::new(&config.ai)) as Arc<dyn Llm>);
    let listener =
        TcpListener::bind(listen).with_context(|| format!("cannot listen on {listen}"))?;
    println!(
        "ABRAXAS listening on {} (one line). Ctrl+C stops.",
        listener.local_addr()?
    );

    let global = global.clone();
    run_on(
        listener,
        Arc::new(move || {
            let ctx = Ctx::new(
                config.clone(),
                paths.clone(),
                global.clone(),
                pack.clone(),
                llm.clone(),
            );
            // Warm the model up so it is ready when the caller registers, as the local game does.
            if config.ai.enabled {
                ctx.director.warmup();
            }
            ctx
        }),
    )
}

/// Accept callers on `listener`, one at a time; a call that comes while another is on the line
/// gets [`BUSY`] and is dropped. Never returns.
///
/// `make_ctx` builds each caller's context on the session thread: a fresh director per call, so
/// a caller's generation jobs end with the call.
pub(crate) fn run_on(listener: TcpListener, make_ctx: Arc<dyn Fn() -> Ctx + Send + Sync>) -> ! {
    let line = Arc::new(AtomicBool::new(false));
    loop {
        let stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(e) => {
                tracing::warn!("accept failed: {e}");
                std::thread::sleep(ACCEPT_RETRY);
                continue;
            }
        };
        let peer = stream
            .peer_addr()
            .map_or_else(|_| "unknown".to_string(), |a| a.to_string());
        if line.swap(true, Ordering::SeqCst) {
            tracing::info!("busy: refused {peer}");
            refuse(stream);
            continue;
        }
        tracing::info!("call from {peer}");
        let (taken, make_ctx) = (line.clone(), make_ctx.clone());
        let spawned = std::thread::Builder::new()
            .name("abraxas-line".into())
            .spawn(move || {
                answer(&stream, &peer, &*make_ctx);
                taken.store(false, Ordering::SeqCst);
            });
        if let Err(e) = spawned {
            // The closure and the stream in it went with the failed spawn: free the line.
            tracing::error!("cannot start a session thread: {e}");
            line.store(false, Ordering::SeqCst);
        }
    }
}

/// Tell a caller the line is taken. The text is a few bytes into an empty socket buffer, so the
/// write cannot block the accept loop.
fn refuse(mut stream: TcpStream) {
    let _ = stream.write_all(BUSY);
}

/// Play one call. A panic in the session ends that call, not the board.
fn answer(stream: &TcpStream, peer: &str, make_ctx: &(dyn Fn() -> Ctx + Send + Sync)) {
    match catch_unwind(AssertUnwindSafe(|| session::serve_one(stream, make_ctx))) {
        Ok(Ok(())) => tracing::info!("hung up: {peer}"),
        Ok(Err(e)) => tracing::warn!("hung up: {peer}: {e}"),
        Err(panic) => tracing::error!("session for {peer} panicked: {}", panic_text(&*panic)),
    }
}

fn panic_text(panic: &(dyn Any + Send)) -> &str {
    panic
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("no message")
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::net::SocketAddr;
    use std::path::Path;

    use super::*;
    use crate::net::testkit::{Caller, PATIENCE};
    use crate::ui::app::test_ctx;
    use ratatui::layout::Size;

    /// A board on a free local port, running for the rest of the test process; `make_ctx` builds
    /// each call's context.
    fn open_board(make_ctx: Arc<dyn Fn() -> Ctx + Send + Sync>) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            run_on(listener, make_ctx);
        });
        addr
    }

    fn open_test_board(dir: &Path) -> SocketAddr {
        let dir = dir.to_path_buf();
        open_board(Arc::new(move || test_ctx(&dir)))
    }

    fn call(addr: SocketAddr) -> Caller {
        Caller::new(TcpStream::connect(addr).unwrap(), Size::new(80, 25))
    }

    #[test]
    fn a_second_caller_gets_line_busy_and_the_first_keeps_the_line() {
        let dir = tempfile::tempdir().unwrap();
        let addr = open_test_board(dir.path());

        let mut first = call(addr);
        first.wait_for("Handle:");

        let mut second = TcpStream::connect(addr).unwrap();
        second.set_read_timeout(Some(PATIENCE)).unwrap();
        let mut said = Vec::new();
        second.read_to_end(&mut said).unwrap();
        assert_eq!(said, BUSY, "{:?}", String::from_utf8_lossy(&said));

        // The refusal did not disturb the call in progress.
        first.send(b"ab");
        first.wait_for("Handle: ab");
    }

    #[test]
    fn the_line_is_free_again_once_the_caller_hangs_up() {
        let dir = tempfile::tempdir().unwrap();
        let addr = open_test_board(dir.path());

        let mut first = call(addr);
        first.wait_for("Handle:");
        first.send(b"\x03");
        first.wait_for("Hang up? [Y/n]");
        first.send(b"y");
        assert!(first.hung_up());

        // Redialling the moment the hang-up shows is answered, not told the line is busy.
        let mut next = call(addr);
        next.wait_for("Handle:");
    }

    #[test]
    fn a_session_that_panics_frees_the_line() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        let first_call = AtomicBool::new(true);
        let addr = open_board(Arc::new(move || {
            if first_call.swap(false, Ordering::SeqCst) {
                panic!("the first caller breaks the game");
            }
            test_ctx(&path)
        }));

        let mut first = call(addr);
        assert!(first.hung_up(), "the broken session closes its connection");

        let mut next = call(addr);
        next.wait_for("Handle:");
    }

    #[test]
    fn a_port_already_in_use_is_an_error_not_a_hang() {
        let taken = TcpListener::bind("127.0.0.1:0").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config::default();
        config.ai.enabled = false;
        let err = run(
            &GlobalOpts::default(),
            config,
            Paths::in_dir(dir.path().to_path_buf()),
            &taken.local_addr().unwrap().to_string(),
        )
        .unwrap_err();
        assert!(
            format!("{err:#}").starts_with("cannot listen on 127.0.0.1:"),
            "{err:#}"
        );
    }
}
