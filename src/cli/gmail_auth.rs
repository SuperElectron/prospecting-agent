use serde::Deserialize;

use crate::connectors::ConnectorError;
use crate::connectors::gmail::{AuthChallenge, OauthClient, SenderAccount, load_senders, save_senders};

#[derive(Deserialize)]
struct Profile {
    #[serde(rename = "emailAddress")]
    email_address: String,
}

pub struct GmailAuthArgs {
    pub client_file: String,
    pub senders_file: String,
    pub port: u16,
    pub daily_limit: i32,
}

pub async fn run(args: &GmailAuthArgs) -> Result<(), ConnectorError> {
    let oauth = OauthClient::from_file(&args.client_file)?;
    let challenge = AuthChallenge::generate();
    let redirect = format!("http://localhost:{}/oauth2callback", args.port);
    let auth_url = oauth.auth_url(&redirect, &challenge);
    println!("Open this URL, sign in as the sending account, and approve:");
    println!("\n  {auth_url}\n");
    open_browser(&auth_url);

    let port = args.port;
    let expected_state = challenge.state.clone();
    let code = tokio::time::timeout(
        std::time::Duration::from_secs(300),
        tokio::task::spawn_blocking(move || wait_for_code(port, &expected_state)),
    )
    .await
    .map_err(|_| ConnectorError::Auth("no browser redirect arrived within 5 minutes".into()))?
    .map_err(|e| ConnectorError::Auth(format!("auth listener task failed: {e}")))??;
    let http = reqwest::Client::new();
    let refresh_token = oauth.exchange_code(&http, &code, &redirect, &challenge).await?;
    let access_token = oauth.access_token(&http, &refresh_token).await?;
    let email = fetch_profile_email(&http, &access_token).await?;

    let mut senders = load_senders(&args.senders_file)?;
    senders.retain(|s| s.email != email);
    senders.push(SenderAccount {
        name: email.split('@').next().unwrap_or("sender").to_string(),
        email: email.clone(),
        refresh_token,
        daily_limit: args.daily_limit,
    });
    save_senders(&args.senders_file, &senders)?;
    println!(
        "Authorized {email}. {} sender(s) now in {}",
        senders.len(),
        args.senders_file
    );
    println!("Run again to add another sender.");
    Ok(())
}

fn open_browser(url: &str) {
    let command = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "start"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(command).arg(url).spawn();
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then(|| urldecode(v))
    })
}

fn wait_for_code(port: u16, expected_state: &str) -> Result<String, ConnectorError> {
    let server = tiny_http::Server::http(("127.0.0.1", port))
        .map_err(|e| ConnectorError::Auth(format!("cannot listen on port {port}: {e}")))?;
    println!("Waiting for the browser redirect on port {port}...");
    for request in server.incoming_requests() {
        let url = request.url().to_string();
        if !url.starts_with("/oauth2callback") {
            let _ = request.respond(tiny_http::Response::from_string("not found").with_status_code(404));
            continue;
        }
        let state = query_param(&url, "state");
        if state.as_deref() != Some(expected_state) {
            let _ = request.respond(tiny_http::Response::from_string("state mismatch").with_status_code(400));
            return Err(ConnectorError::Auth(
                "redirect state did not match this session".into(),
            ));
        }
        match query_param(&url, "code") {
            Some(code) if !code.is_empty() => {
                let page = "<h2>Gmail authorization successful.</h2><p>You can close this tab.</p>";
                let response = tiny_http::Response::from_string(page).with_header(
                    tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html"[..])
                        .expect("static header is valid"),
                );
                let _ = request.respond(response);
                return Ok(code);
            }
            _ => {
                let _ =
                    request.respond(tiny_http::Response::from_string("missing code").with_status_code(400));
                return Err(ConnectorError::Auth(
                    "redirect carried no authorization code".into(),
                ));
            }
        }
    }
    Err(ConnectorError::Auth(
        "auth server stopped before a redirect arrived".into(),
    ))
}

async fn fetch_profile_email(http: &reqwest::Client, access_token: &str) -> Result<String, ConnectorError> {
    let resp = http
        .get("https://gmail.googleapis.com/gmail/v1/users/me/profile")
        .bearer_auth(access_token)
        .send()
        .await?;
    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        let body = resp.text().await.unwrap_or_default();
        return Err(ConnectorError::Status { status, body });
    }
    let raw = resp.text().await?;
    let profile: Profile = serde_json::from_str(&raw).map_err(|e| ConnectorError::Decode(e.to_string()))?;
    Ok(profile.email_address)
}

fn urldecode(raw: &str) -> String {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_param_matches_exact_keys_only() {
        let url = "/oauth2callback?authcode=zz&code=real%2Fcode&state=abc";
        assert_eq!(query_param(url, "code").as_deref(), Some("real/code"));
        assert_eq!(query_param(url, "state").as_deref(), Some("abc"));
        assert_eq!(query_param(url, "missing"), None);
    }

    #[test]
    fn urldecode_handles_percent_sequences_and_plus() {
        assert_eq!(urldecode("4%2F0AXo%2B-code"), "4/0AXo+-code");
        assert_eq!(urldecode("a+b"), "a b");
        assert_eq!(urldecode("plain"), "plain");
        assert_eq!(urldecode("bad%zz"), "bad%zz");
    }
}
