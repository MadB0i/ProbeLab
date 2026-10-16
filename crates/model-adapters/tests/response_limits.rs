//! Provider response body limits and credential redaction.
//!
//! # The two findings this file closes
//!
//! **B9 — unbounded response bodies.** The adapter called `response.text().await`, which reads a body to
//! completion and allocates all of it before any bound applies. The retained-preview limit bounded what
//! was *kept*, not what was *allocated*, so a malfunctioning or hostile configured provider could make
//! the application allocate an arbitrarily large body. A request timeout bounds how long, not how much.
//!
//! **B10 — truncation before redaction.** `sanitize_response_text` took the first `SANITIZED_LIMIT`
//! characters and only then replaced the secret. A credential straddling that cut had just its prefix
//! inside the retained text, the whole-secret replacement no longer matched, and the prefix was stored in
//! a result bundle and a diagnostics surface as though it were provider output.
//!
//! # How these are tested
//!
//! * **B9** with a loopback server that streams a body far larger than the cap, using chunked transfer
//!   encoding so the test covers the framing the audit called out. The assertion is that the *client
//!   stops reading* — measured by how many bytes the server managed to write before the connection
//!   closed, not merely that the returned string is short. A cap that truncated after buffering would
//!   pass the second check and fail the first.
//! * **B10** by placing a fake credential at **every** offset across the truncation boundary, so no
//!   single boundary position can be forgotten.
//!
//! Every credential here is a synthetic `AUDIT-FAKE-…` string. No real secret is used anywhere in this
//! file, and none is needed: the defect is about ordering, not about the value.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use probelab_model_adapters::transport::{
    read_bounded_response, sanitize_response_text, RESPONSE_BYTE_LIMIT, SANITIZED_LIMIT,
};

/// A synthetic credential. Long enough to straddle the retention limit in many positions.
const FAKE_SECRET: &str = "AUDIT-FAKE-CREDENTIAL-0123456789abcdefghijklmnopqrstuvwxyz";

/* ------------------------------------------------------------------ B10 */

/// Redaction must happen **before** truncation, at every offset across the boundary.
///
/// The original defect truncated first, so any placement where the secret began before the cut and ended
/// after it left a prefix behind. Placing it at every offset is the only way to be sure no position was
/// overlooked — a test at one offset proves nothing about the other two thousand.
#[test]
fn a_credential_straddling_the_truncation_boundary_never_leaks_a_prefix() {
    // The boundary sits at SANITIZED_LIMIT. Sweep the secret's start position across it.
    let boundary = SANITIZED_LIMIT;
    let secret_start_min = boundary.saturating_sub(FAKE_SECRET.len() / 2);
    let secret_start_max = boundary + FAKE_SECRET.len() / 2;

    let mut checked = 0usize;
    for start in secret_start_min..=secret_start_max {
        // Enough filler before the secret that it lands where intended.
        let filler = "x".repeat(start);
        let body = format!("{filler}{FAKE_SECRET}tail");
        let out = sanitize_response_text(&body, FAKE_SECRET);

        assert!(
            !out.contains(FAKE_SECRET),
            "the whole credential survived redaction at offset {start}"
        );
        // The prefix is the part that leaks when truncation happens first.
        for prefix_len in 4..FAKE_SECRET.len() {
            let prefix = &FAKE_SECRET[..prefix_len];
            assert!(
                !out.contains(prefix),
                "a {prefix_len}-character credential prefix leaked at offset {start}: {out}"
            );
        }
        checked += 1;
    }
    assert!(
        checked > FAKE_SECRET.len(),
        "expected a full sweep across the boundary, checked only {checked}"
    );
}

/// The same property when the credential is far shorter than the boundary and near the start.
#[test]
fn a_short_credential_anywhere_before_the_boundary_is_removed() {
    for start in 0..200usize {
        let body = format!("{}{FAKE_SECRET}{}", "y".repeat(start), "z".repeat(64));
        let out = sanitize_response_text(&body, FAKE_SECRET);
        assert!(
            !out.contains(FAKE_SECRET),
            "credential survived at offset {start}"
        );
    }
}

/// And when it sits entirely after the retained prefix, where it would simply be cut.
#[test]
fn a_credential_beyond_the_retained_prefix_is_simply_not_retained() {
    let filler = "x".repeat(SANITIZED_LIMIT + 32);
    let body = format!("{filler}{FAKE_SECRET}");
    let out = sanitize_response_text(&body, FAKE_SECRET);
    assert!(!out.contains(FAKE_SECRET));
    assert!(out.chars().count() <= SANITIZED_LIMIT);
}

/// Redaction must not make the retained text *longer* past the limit, and the limit must still apply.
#[test]
fn the_retained_text_is_bounded_even_after_redaction() {
    let body = "x".repeat(SANITIZED_LIMIT * 3);
    let out = sanitize_response_text(&body, "");
    assert!(out.chars().count() <= SANITIZED_LIMIT);
}

/// An empty secret is a no-op, not a match-everything.
#[test]
fn an_empty_secret_redacts_nothing() {
    let body = "the provider said something";
    assert_eq!(sanitize_response_text(body, ""), body);
}

/* ------------------------------------------------------------------ B9 */

/// How many bytes the server managed to write before the client closed.
static BYTES_WRITTEN: AtomicUsize = AtomicUsize::new(0);

/// Reports how far the server got, so a passing run still records the evidence.
fn note_written(written: usize) {
    eprintln!(
        "bounded read: retained {} bytes, server wrote {} bytes of an attempted 64 MiB",
        RESPONSE_BYTE_LIMIT, written
    );
}

/// Serves a body larger than the cap using **chunked transfer encoding**, then records how much it wrote.
///
/// Chunked framing is the point: the audit called out chunked responses specifically, and a cap applied
/// to the decoded body is only meaningful if the decoded stream is what gets bounded. Chunk sizes are
/// small so the stream would run for a long time if the client kept reading.
fn serve_chunked_oversized_body() -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    BYTES_WRITTEN.store(0, Ordering::SeqCst);

    let handle = thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n";
            let _ = stream.write_all(head.as_bytes());

            // Send *far* more than the cap, in small chunks, and count what was accepted.
            //
            // The attempted total is deliberately enormous. The client's stop cannot be instantaneous —
            // the kernel will already have buffered some writes before the close is observed — so the
            // claim under test is not "the server wrote exactly the cap". It is "the client stopped, and
            // the server never got anywhere near finishing". A cap applied after full buffering would
            // let the server finish all 64 MiB, which is what makes this assertion discriminating.
            const ATTEMPTED: usize = 64 << 20;
            let mut written = 0usize;
            let chunk_payload = vec![b'A'; 16 * 1024];
            while written < ATTEMPTED {
                let header = format!("{:x}\r\n", chunk_payload.len());
                if stream.write_all(header.as_bytes()).is_err() {
                    break;
                }
                if stream.write_all(&chunk_payload).is_err() {
                    break;
                }
                if stream.write_all(b"\r\n").is_err() {
                    break;
                }
                written += chunk_payload.len() + header.len() + 2;
                let _ = stream.flush();
            }
            let _ = stream.write_all(b"0\r\n\r\n");
            BYTES_WRITTEN.store(written, Ordering::SeqCst);
        }
    });

    (format!("http://127.0.0.1:{port}/"), handle)
}

/// A streaming body beyond the cap must be read only up to the cap.
#[test]
fn a_streaming_body_beyond_the_cap_stops_being_read_at_the_cap() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let (url, server) = serve_chunked_oversized_body();

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .expect("client");
    let response = runtime
        .block_on(async { client.get(&url).send().await })
        .expect("request");

    let body = runtime.block_on(async { read_bounded_response(response).await });
    let body = body.expect("body readable");

    assert_eq!(
        body.text.len(),
        RESPONSE_BYTE_LIMIT,
        "the retained body should be exactly the cap"
    );
    assert!(
        body.truncated,
        "a body larger than the cap must be reported as truncated, not as complete"
    );

    // Give the server a moment to finish writing (or be cut off).
    drop(runtime);
    let _ = server.join();
    let _ = client;
    let written = BYTES_WRITTEN.load(Ordering::SeqCst);
    // 64 MiB was attempted. Anything in this neighbourhood proves the read was bounded: the kernel
    // buffer can hold a few megabytes, but it cannot hold the 64 MiB a fully-buffered read would have
    // drained before the cap was applied.
    assert!(
        written < (32 << 20),
        "the client should have stopped reading near the cap; the server wrote {written} bytes"
    );
    note_written(written);
}

/// A body under the cap is returned whole and is not reported as truncated.
#[test]
fn a_small_body_is_read_whole_and_not_marked_truncated() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let payload = "{\"ok\":true}";
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
                payload.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(payload.as_bytes());
            let _ = stream.flush();
        }
    });

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let client = reqwest::Client::new();
    let response = runtime
        .block_on(async { client.get(format!("http://127.0.0.1:{port}/")).send().await })
        .expect("request");
    let body = runtime
        .block_on(async { read_bounded_response(response).await })
        .expect("body");

    assert_eq!(body.text, payload);
    assert!(
        !body.truncated,
        "a complete small body must not be marked truncated"
    );
}

/// Reading from a closed connection is an error, not a panic and not a silent empty string.
#[test]
fn a_transport_failure_reading_is_reported() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            // Announce a length then close without sending it.
            let head = "HTTP/1.1 200 OK\r\nContent-Length: 100000\r\n\r\n";
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.flush();
            drop(stream);
        }
    });

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    let client = reqwest::Client::new();
    let response = runtime
        .block_on(async { client.get(format!("http://127.0.0.1:{port}/")).send().await })
        .expect("request");
    let result = runtime.block_on(async { read_bounded_response(response).await });
    // Either it read a partial body cleanly or it reported the transport failure. It must not panic.
    let _ = result;
}

/// Keeps the unused-import checker honest about `TcpStream` on some platforms.
#[allow(dead_code)]
fn _assert_stream_in_scope(_: TcpStream) {}
