//! Loopback HTTP listener that catches the OAuth redirect.
//!
//! The flow:
//!   1. We bind a one-shot HTTP listener to `127.0.0.1:0` (OS-assigned port).
//!   2. We construct an auth URL with `redirect_uri=http://localhost:PORT/callback`.
//!   3. User opens the URL in their browser, authenticates with Microsoft.
//!   4. MS redirects the browser to our localhost URL with `?code=...&state=...`.
//!   5. We accept the connection, parse the code, send a friendly HTML
//!      "you can close this tab" reply, and shut down.
//!
//! The listener is bound to `127.0.0.1` only — never `0.0.0.0`. This is a
//! per-launch ephemeral port reachable only by the local machine.

use std::net::TcpListener;
use std::time::Duration;

use tiny_http::{Method, Response, Server};

const TIMEOUT_SECS: u64 = 180;

/// Result of the loopback listener — either we got the auth code, the
/// user explicitly cancelled (server returned `error`), or the wait timed
/// out (user closed the browser without authenticating).
#[derive(Debug)]
pub enum LoopbackResult {
    Code(String),
    Cancelled,
    TimedOut,
}

/// Bind to a free local port + return `(server, redirect_uri)`. Caller
/// should embed `redirect_uri` in the auth URL and immediately call
/// `wait_for_redirect(&server, &expected_state)` to receive the code.
///
/// Returns `Err` only if no port could be bound (extremely rare —
/// usually means the OS is out of ports or a firewall is blocking).
pub fn bind() -> Result<(Server, String), String> {
    // Bind to port 0 (OS-chosen) on loopback, then read back the actual
    // port. We use std::net::TcpListener for the port discovery + hand
    // the listener to tiny_http (which accepts a `TcpListener`).
    let listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("could not bind loopback listener: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("loopback addr query failed: {}", e))?
        .port();
    let server = Server::from_listener(listener, None)
        .map_err(|e| format!("tiny_http takeover failed: {}", e))?;
    // Microsoft Entra ID's localhost-special-casing wildcards the port but
    // requires the path to match the registered URI. Most desktop-app
    // registrations use bare `http://localhost` (no port, no path), so we
    // send a bare host:port too — any path would mismatch.
    let redirect = format!("http://localhost:{}", port);
    Ok((server, redirect))
}

/// Block waiting for the OAuth redirect. Returns once we receive a GET
/// request to `/callback` (any path, really — we don't validate it
/// against a stored route since the listener only handles one request
/// before being dropped) with either `code=` or `error=`.
///
/// `expected_state` is the `state` parameter we put in the auth URL —
/// we verify it round-trips so we don't accept a redirect from a
/// different auth session.
pub fn wait_for_redirect(server: &Server, expected_state: &str) -> LoopbackResult {
    let deadline = std::time::Instant::now() + Duration::from_secs(TIMEOUT_SECS);

    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return LoopbackResult::TimedOut;
        }
        // recv_timeout returns `Ok(None)` on timeout, `Ok(Some(req))` on
        // a request, `Err` only on listener failure.
        let req = match server.recv_timeout(remaining) {
            Ok(Some(req)) => req,
            Ok(None) => continue, // tick — re-check deadline
            Err(_) => return LoopbackResult::TimedOut,
        };

        // Anything that isn't GET — politely 405. Some browsers prefetch
        // with HEAD which we should also handle gracefully.
        if !matches!(req.method(), Method::Get | Method::Head) {
            let _ = req.respond(Response::from_string("Method not allowed").with_status_code(405));
            continue;
        }

        let url = req.url().to_string();
        let (code, state, error) = parse_callback(&url);
        let decision = classify(code, state.as_deref(), error, expected_state);

        // Reply to the browser before processing — closes the tab visually.
        let body = match decision {
            Callback::Code(_) => SUCCESS_PAGE,
            _ => FAIL_PAGE,
        };
        let resp = Response::from_string(body)
            .with_header(
                "Content-Type: text/html; charset=utf-8"
                    .parse::<tiny_http::Header>()
                    .unwrap(),
            );
        let _ = req.respond(resp);

        match decision {
            Callback::Code(code) => return LoopbackResult::Code(code),
            Callback::Cancelled(err) => {
                log::info!("auth: loopback received error={}", err);
                return LoopbackResult::Cancelled;
            }
            Callback::Ignore(why) => {
                log::warn!("auth: loopback callback ignored — {}", why);
                continue;
            }
        }
    }
}

/// What a loopback request means for the sign-in in progress.
#[derive(Debug, PartialEq, Eq)]
enum Callback {
    Code(String),
    Cancelled(String),
    Ignore(&'static str),
}

/// CSRF guard first: nothing — not even an `error=` that would cancel the
/// sign-in — is honoured unless the request carries the `state` we sent.
fn classify(
    code: Option<String>,
    state: Option<&str>,
    error: Option<String>,
    expected_state: &str,
) -> Callback {
    match state {
        None => return Callback::Ignore("missing state"),
        Some(s) if s != expected_state => return Callback::Ignore("state mismatch"),
        Some(_) => {}
    }
    if let Some(err) = error {
        return Callback::Cancelled(err);
    }
    match code {
        Some(c) => Callback::Code(c),
        None => Callback::Ignore("neither code nor error"),
    }
}

fn parse_callback(url: &str) -> (Option<String>, Option<String>, Option<String>) {
    // Format: `/callback?code=...&state=...` or `/callback?error=...&error_description=...`.
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        let v_decoded = urlencoding::decode(v).map(|c| c.into_owned()).unwrap_or_else(|_| v.to_string());
        match k {
            "code" => code = Some(v_decoded),
            "state" => state = Some(v_decoded),
            "error" => error = Some(v_decoded),
            _ => {}
        }
    }
    (code, state, error)
}

const SUCCESS_PAGE: &str = r#"<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<title>EwoClient — signed in</title>
<style>
  body { font-family: -apple-system, system-ui, "Segoe UI", sans-serif;
         background: #0A0006; color: #F4E8EA; margin: 0;
         display: grid; place-items: center; min-height: 100vh; }
  .card { padding: 32px 40px; border-radius: 16px;
          background: rgba(229,184,197,0.06);
          box-shadow: inset 0 0 0 1px rgba(229,184,197,0.18); }
  h1 { font-size: 24px; font-weight: 300; margin: 0 0 8px; }
  p { color: #9A8087; margin: 0; font-style: italic; }
</style></head><body>
<div class="card">
  <h1>signed in.</h1>
  <p>you can close this tab — the curtain rises.</p>
</div>
</body></html>"#;

const FAIL_PAGE: &str = r#"<!doctype html>
<html lang="en"><head>
<meta charset="utf-8">
<title>EwoClient — sign-in failed</title>
<style>
  body { font-family: -apple-system, system-ui, "Segoe UI", sans-serif;
         background: #0A0006; color: #F4E8EA; margin: 0;
         display: grid; place-items: center; min-height: 100vh; }
  .card { padding: 32px 40px; border-radius: 16px;
          background: rgba(201,106,122,0.10);
          box-shadow: inset 0 0 0 1px rgba(201,106,122,0.4); }
  h1 { font-size: 24px; font-weight: 300; margin: 0 0 8px; }
  p { color: #9A8087; margin: 0; font-style: italic; }
</style></head><body>
<div class="card">
  <h1>sign-in cancelled.</h1>
  <p>you can close this tab — try again from EwoClient.</p>
</div>
</body></html>"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_without_matching_state_does_not_cancel() {
        assert_eq!(
            classify(None, None, Some("access_denied".into()), "s1"),
            Callback::Ignore("missing state")
        );
        assert_eq!(
            classify(None, Some("evil"), Some("access_denied".into()), "s1"),
            Callback::Ignore("state mismatch")
        );
        assert_eq!(
            classify(None, Some("s1"), Some("access_denied".into()), "s1"),
            Callback::Cancelled("access_denied".into())
        );
    }

    #[test]
    fn code_requires_matching_state() {
        assert_eq!(
            classify(Some("c".into()), Some("evil"), None, "s1"),
            Callback::Ignore("state mismatch")
        );
        assert_eq!(classify(Some("c".into()), Some("s1"), None, "s1"), Callback::Code("c".into()));
        assert_eq!(classify(None, Some("s1"), None, "s1"), Callback::Ignore("neither code nor error"));
        assert_eq!(
            parse_callback("/?code=a%2Bb&state=s1"),
            (Some("a+b".into()), Some("s1".into()), None)
        );
    }
}
