//! Integration tests for the star-http binary.
//!
//! star-http is the workspace's only binary crate, so these tests spawn the
//! real compiled server (`env!("CARGO_BIN_EXE_star-http")` — cargo builds the
//! bin automatically for integration tests) as a child process and talk to it
//! over HTTP:
//! - /health returns 200 with exactly {"status":"ok"},
//! - unknown routes under /api/v1 return 404 (not 500, not open),
//! - the server fails fast at startup when STAR_JWT_SECRET is unset (exits
//!   non-zero quickly instead of serving with a broken config).
//!
//! The child is owned by a Drop guard, so it is killed even when an
//! assertion fails mid-test.
//!
//! Conventions (mirrors the rest of the workspace):
//! - Skipped cleanly when DATABASE_URL is not set (prints SKIP and returns).
//! - Run with: DATABASE_URL=postgres://star@127.0.0.1:5433/star_lms_test
//!   cargo test -p star-http
//! - The server runs migrations at startup, so it shares the same test DB as
//!   the other crates' integration tests (sqlx migrations are checksummed and
//!   idempotent).

use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};
use wait_timeout::ChildExt;

/// JWT secret handed to the server under test (any non-empty value works;
/// no request in this suite needs a valid token).
const JWT_SECRET: &str = "test-jwt-secret";

/// Total budget for waiting on /health to come up.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);

/// Poll interval while waiting for /health.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Max time the fail-fast run may take to exit on bad config.
const FAIL_FAST_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Free-port strategy
// ---------------------------------------------------------------------------

/// Pick a free high port by binding an ephemeral TcpListener on 127.0.0.1
/// and reading its OS-assigned port, then dropping the listener. To avoid a
/// TOCTOU race with another process grabbing the port in between, retries
/// are built into the startup polling: if the child dies because the port
/// was taken, the spawn helper picks a fresh port and retries.
fn free_port() -> u16 {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
        .expect("bind ephemeral listener for port pick");
    listener
        .local_addr()
        .expect("ephemeral listener local_addr")
        .port()
}

// ---------------------------------------------------------------------------
// Child-process plumbing
// ---------------------------------------------------------------------------

/// Kill the child on scope exit, even when an assertion fails first.
struct ChildGuard {
    child: Option<Child>,
    port: Option<u16>,
}

impl ChildGuard {
    fn spawn(port: u16, db_url: &str) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_star-http"))
            .env("DATABASE_URL", db_url)
            .env("STAR_JWT_SECRET", JWT_SECRET)
            .env("STAR_PORT", port.to_string())
            // Keep the child's log output out of the test harness's stdout.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn star-http binary");
        Self {
            child: Some(child),
            port: Some(port),
        }
    }

    /// Best-effort graceful stop: SIGTERM (star-http shuts down gracefully),
    /// then wait briefly, then SIGKILL.
    fn stop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let pid = child.id();
            unsafe {
                libc::kill(pid as libc::pid_t, libc::SIGTERM);
            }
            match child.wait_timeout(Duration::from_secs(5)) {
                Ok(Some(_)) => {}
                Ok(None) => {
                    unsafe {
                        libc::kill(pid as libc::pid_t, libc::SIGKILL);
                    }
                    let _ = child.wait();
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                }
            }
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Send a minimal HTTP/1.1 GET by hand and return (status_code, body).
///
/// The server speaks plain HTTP on localhost, so a raw TCP client is enough
/// and keeps this suite free of any HTTP-client dev-dependency.
fn raw_http_get(port: u16, path: &str) -> std::io::Result<(u16, String)> {
    let mut stream = TcpStream::connect(SocketAddrV4::new(Ipv4Addr::LOCALHOST, port))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;

    let request = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes())?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw)?;

    let text = String::from_utf8_lossy(&raw).into_owned();
    let (status_line, body) = match text.split_once("\r\n\r\n") {
        Some((head, body)) => (head, body),
        None => (text.as_str(), ""),
    };

    let status: u16 = status_line
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse().ok())
        .unwrap_or(0);

    Ok((status, body.to_string()))
}

/// Spawn the server on a free port and poll /health until it answers 200.
///
/// Retries on a different free port when a spawn dies before ever becoming
/// healthy (e.g. another process won the race for the previously picked
/// port): that is the TOCTOU escape hatch for the free-port strategy.
fn spawn_healthy(db_url: &str) -> Option<ChildGuard> {
    for _ in 0..3 {
        let port = free_port();
        let mut guard = ChildGuard::spawn(port, db_url);

        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Ok((status, _)) = raw_http_get(port, "/health") {
                if status == 200 {
                    return Some(guard);
                }
            }

            match guard.child.as_mut().map(Child::try_wait) {
                // Child is still alive — keep polling until the deadline.
                Some(Ok(None)) => {}
                // Child already exited: it will never become healthy.
                Some(Ok(Some(_))) => break,
                Some(Err(_)) => break,
                None => break,
            }

            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }

        // Not healthy on this port — drop (kill) and retry on a fresh one.
        guard.stop();
    }
    None
}

/// DATABASE_URL from the environment, or None so the caller can skip.
fn db_url() -> Option<String> {
    std::env::var("DATABASE_URL")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

// ---------------------------------------------------------------------------
// /health returns 200 {"status":"ok"}
// ---------------------------------------------------------------------------

#[test]
fn health_returns_200_status_ok() {
    let Some(db_url) = db_url() else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let Some(mut guard) = spawn_healthy(&db_url) else {
        panic!("star-http did not become healthy on /health within {STARTUP_TIMEOUT:?}");
    };

    let port = guard.port.expect("healthy server must remember its port");

    let (status, body) = raw_http_get(port, "/health").expect("GET /health");
    assert_eq!(status, 200, "GET /health must be 200, body: {body}");
    assert_eq!(
        body, "{\"status\":\"ok\"}",
        "GET /health body must be exactly the coded shape"
    );

    guard.stop();
}

// ---------------------------------------------------------------------------
// Unknown /api/v1 route returns 404
// ---------------------------------------------------------------------------

#[test]
fn unknown_api_v1_route_returns_404() {
    let Some(db_url) = db_url() else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };
    let Some(mut guard) = spawn_healthy(&db_url) else {
        panic!("star-http did not become healthy on /health within {STARTUP_TIMEOUT:?}");
    };

    let port = guard.port.expect("healthy server must remember its port");

    let (status, _) =
        raw_http_get(port, "/api/v1/definitely-not-a-route").expect("GET unknown /api/v1 route");
    assert_eq!(
        status, 404,
        "unknown /api/v1 route must 404, not leak a 500 or a body"
    );

    guard.stop();
}

// ---------------------------------------------------------------------------
// Fail fast on missing STAR_JWT_SECRET
// ---------------------------------------------------------------------------

#[test]
fn missing_jwt_secret_fails_fast_nonzero_exit() {
    let Some(db_url) = db_url() else {
        eprintln!("SKIP: DATABASE_URL not set");
        return;
    };

    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_star-http"))
        .env("DATABASE_URL", db_url)
        .env_remove("STAR_JWT_SECRET")
        .env("STAR_PORT", port.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn star-http without STAR_JWT_SECRET");

    // The guard ensures cleanup even if the exit assertions below fail.
    let mut guard = ChildGuard {
        child: Some(child),
        port: None,
    };

    let deadline = Instant::now() + FAIL_FAST_TIMEOUT;
    loop {
        match guard.child.as_mut().map(Child::try_wait) {
            Some(Ok(Some(status))) => {
                assert!(
                    !status.success(),
                    "star-http must exit non-zero when STAR_JWT_SECRET is unset, got {status}"
                );
                break;
            }
            Some(Ok(None)) => {}
            Some(Err(e)) => panic!("waitpid on star-http child failed: {e}"),
            None => unreachable!("guard always holds a child here"),
        }

        assert!(
            Instant::now() < deadline,
            "star-http kept running {FAIL_FAST_TIMEOUT:?} without STAR_JWT_SECRET — \
             it must fail fast instead of serving"
        );
        std::thread::sleep(POLL_INTERVAL);
    }

    guard.stop();
}
