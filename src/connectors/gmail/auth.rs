use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::Secret;
use crate::connectors::ConnectorError;

pub const SCOPES: &str =
    "https://www.googleapis.com/auth/gmail.send https://www.googleapis.com/auth/gmail.readonly";
const DEFAULT_AUTH_BASE: &str = "https://accounts.google.com";
const DEFAULT_TOKEN_BASE: &str = "https://oauth2.googleapis.com";

#[derive(Debug, Clone)]
pub struct OauthClient {
    pub client_id: String,
    pub client_secret: Secret,
    auth_base: String,
    token_base: String,
}

#[derive(Deserialize)]
struct ClientFile {
    installed: ClientEntry,
}

#[derive(Deserialize)]
struct ClientEntry {
    client_id: String,
    client_secret: String,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub struct SenderAccount {
    pub email: String,
    pub name: String,
    pub refresh_token: String,
    pub daily_limit: i32,
}

impl std::fmt::Debug for SenderAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SenderAccount")
            .field("email", &self.email)
            .field("name", &self.name)
            .field("refresh_token", &"***")
            .field("daily_limit", &self.daily_limit)
            .finish()
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct AccessToken {
    pub token: String,
    pub expires_in_secs: u64,
}

impl OauthClient {
    pub fn new(client_id: impl Into<String>, client_secret: Secret) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret,
            auth_base: DEFAULT_AUTH_BASE.into(),
            token_base: DEFAULT_TOKEN_BASE.into(),
        }
    }

    pub fn from_file(path: &str) -> Result<Self, ConnectorError> {
        let raw = std::fs::read_to_string(path).map_err(|e| ConnectorError::Credentials {
            path: path.to_string(),
            reason: e.to_string(),
        })?;
        let parsed: ClientFile = serde_json::from_str(&raw).map_err(|e| ConnectorError::Credentials {
            path: path.to_string(),
            reason: format!("expected installed-app client json: {e}"),
        })?;
        Ok(Self {
            client_id: parsed.installed.client_id,
            client_secret: Secret::new(parsed.installed.client_secret),
            auth_base: DEFAULT_AUTH_BASE.into(),
            token_base: DEFAULT_TOKEN_BASE.into(),
        })
    }

    #[must_use]
    pub fn with_bases(mut self, auth_base: &str, token_base: &str) -> Self {
        self.auth_base = auth_base.trim_end_matches('/').to_string();
        self.token_base = token_base.trim_end_matches('/').to_string();
        self
    }

    pub fn auth_url(&self, redirect_uri: &str, challenge: &AuthChallenge) -> String {
        let code_challenge = challenge.code_challenge();
        let query = [
            ("client_id", self.client_id.as_str()),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", SCOPES),
            ("access_type", "offline"),
            ("prompt", "consent"),
            ("state", challenge.state.as_str()),
            ("code_challenge", code_challenge.as_str()),
            ("code_challenge_method", "S256"),
        ];
        let encoded: Vec<String> = query
            .iter()
            .map(|(k, v)| format!("{k}={}", urlencode(v)))
            .collect();
        format!("{}/o/oauth2/v2/auth?{}", self.auth_base, encoded.join("&"))
    }

    pub async fn exchange_code(
        &self,
        http: &reqwest::Client,
        code: &str,
        redirect_uri: &str,
        challenge: &AuthChallenge,
    ) -> Result<String, ConnectorError> {
        let form = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.expose()),
            ("code", code),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
            ("code_verifier", challenge.verifier.as_str()),
        ];
        let parsed = self.token_request(http, &form).await?;
        parsed
            .refresh_token
            .ok_or_else(|| ConnectorError::Auth("token response carried no refresh_token".into()))
    }

    pub async fn access_token(
        &self,
        http: &reqwest::Client,
        refresh_token: &str,
    ) -> Result<String, ConnectorError> {
        Ok(self.access_token_with_expiry(http, refresh_token).await?.token)
    }

    pub async fn access_token_with_expiry(
        &self,
        http: &reqwest::Client,
        refresh_token: &str,
    ) -> Result<AccessToken, ConnectorError> {
        let form = [
            ("client_id", self.client_id.as_str()),
            ("client_secret", self.client_secret.expose()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ];
        let parsed = self.token_request(http, &form).await?;
        let token = parsed
            .access_token
            .ok_or_else(|| ConnectorError::Auth("token response carried no access_token".into()))?;
        Ok(AccessToken {
            token,
            expires_in_secs: parsed.expires_in.unwrap_or(3600),
        })
    }

    async fn token_request(
        &self,
        http: &reqwest::Client,
        form: &[(&str, &str)],
    ) -> Result<TokenResponse, ConnectorError> {
        let url = format!("{}/token", self.token_base);
        let resp = http.post(&url).form(form).send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            if (status == 400 || status == 401)
                && (body.contains("invalid_grant") || body.contains("invalid_client"))
            {
                return Err(ConnectorError::Auth(format!("refresh token rejected: {body}")));
            }
            return Err(ConnectorError::Status { status, body });
        }
        let raw = resp.text().await?;
        serde_json::from_str(&raw).map_err(|e| ConnectorError::Decode(e.to_string()))
    }
}

#[derive(Debug, Clone)]
pub struct AuthChallenge {
    pub state: String,
    pub verifier: String,
}

impl AuthChallenge {
    pub fn generate() -> Self {
        Self {
            state: uuid::Uuid::new_v4().simple().to_string(),
            verifier: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
        }
    }

    pub fn code_challenge(&self) -> String {
        use base64::Engine;
        use sha2::{Digest, Sha256};
        let digest = Sha256::digest(self.verifier.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
    }
}

pub fn load_senders(path: &str) -> Result<Vec<SenderAccount>, ConnectorError> {
    if !Path::new(path).exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(path).map_err(|e| ConnectorError::Credentials {
        path: path.to_string(),
        reason: e.to_string(),
    })?;
    serde_json::from_str(&raw).map_err(|e| ConnectorError::Credentials {
        path: path.to_string(),
        reason: format!("expected a json array of sender accounts: {e}"),
    })
}

pub fn save_senders(path: &str, senders: &[SenderAccount]) -> Result<(), ConnectorError> {
    let raw = serde_json::to_string_pretty(senders).map_err(|e| ConnectorError::Decode(e.to_string()))?;
    let credentials_error = |reason: String| ConnectorError::Credentials {
        path: path.to_string(),
        reason,
    };
    std::fs::write(path, raw).map_err(|e| credentials_error(e.to_string()))?;
    restrict_to_owner(path).map_err(|e| credentials_error(e.to_string()))
}

#[cfg(unix)]
fn restrict_to_owner(path: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &str) -> std::io::Result<()> {
    Ok(())
}

fn urlencode(raw: &str) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(raw.len());
    for byte in raw.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    #[test]
    fn saved_senders_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("senders-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("senders.json");
        let path = path.to_str().unwrap();
        super::save_senders(
            path,
            &[super::SenderAccount {
                email: "s@x.co".into(),
                name: "S".into(),
                refresh_token: "rt".into(),
                daily_limit: 1,
            }],
        )
        .unwrap();
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    use super::*;
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> OauthClient {
        OauthClient::new("cid", Secret::new("csec")).with_bases(&server.uri(), &server.uri())
    }

    #[test]
    fn client_file_parses_installed_shape() {
        let dir = std::env::temp_dir().join(format!("gmail-client-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("client.json");
        std::fs::write(
            &file,
            r#"{"installed":{"client_id":"abc.apps.googleusercontent.com","client_secret":"s3cret","token_uri":"https://oauth2.googleapis.com/token"}}"#,
        )
        .unwrap();
        let parsed = OauthClient::from_file(file.to_str().unwrap()).unwrap();
        assert_eq!(parsed.client_id, "abc.apps.googleusercontent.com");
        assert_eq!(parsed.client_secret.expose(), "s3cret");
    }

    #[test]
    fn missing_client_file_is_a_credentials_error() {
        let err = OauthClient::from_file("/nonexistent/client.json").unwrap_err();
        assert!(matches!(err, ConnectorError::Credentials { .. }));
    }

    #[test]
    fn auth_url_carries_scopes_offline_access_and_consent() {
        let server_free = OauthClient::new("cid", Secret::new("csec"));
        let challenge = AuthChallenge::generate();
        let url = server_free.auth_url("http://localhost:3847/oauth2callback", &challenge);
        assert!(url.starts_with("https://accounts.google.com/o/oauth2/v2/auth?"));
        assert!(url.contains("gmail.send"));
        assert!(url.contains("gmail.readonly"));
        assert!(url.contains("access_type=offline"));
        assert!(url.contains("prompt=consent"));
        assert!(url.contains("redirect_uri=http%3A%2F%2Flocalhost%3A3847%2Foauth2callback"));
        assert!(url.contains(&format!("state={}", challenge.state)));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains(&format!("code_challenge={}", challenge.code_challenge())));
    }

    #[tokio::test]
    async fn code_exchange_returns_refresh_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=authorization_code"))
            .and(body_string_contains("code_verifier="))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "at-1", "refresh_token": "rt-1", "expires_in": 3599,
            })))
            .expect(1)
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let token = client(&server)
            .exchange_code(
                &http,
                "auth-code",
                "http://localhost:3847/oauth2callback",
                &AuthChallenge::generate(),
            )
            .await
            .unwrap();
        assert_eq!(token, "rt-1");
    }

    #[tokio::test]
    async fn refresh_exchange_returns_access_token_and_rejects_bad_grants() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=rt-1"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"access_token": "at-2", "expires_in": 3599})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let http = reqwest::Client::new();
        let oauth = client(&server);
        let token = oauth.access_token(&http, "rt-1").await.unwrap();
        assert_eq!(token, "at-2");

        let bad = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_string(r#"{"error":"invalid_grant"}"#))
            .mount(&bad)
            .await;
        let err = client(&bad).access_token(&http, "revoked").await.unwrap_err();
        assert!(matches!(err, ConnectorError::Auth(_)));
    }

    #[test]
    fn senders_round_trip_through_the_store_file() {
        let dir = std::env::temp_dir().join(format!("gmail-senders-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("senders.json");
        let path = file.to_str().unwrap();
        assert!(load_senders(path).unwrap().is_empty());
        let senders = vec![SenderAccount {
            email: "jane@acme.io".into(),
            name: "Jane".into(),
            refresh_token: "rt".into(),
            daily_limit: 50,
        }];
        save_senders(path, &senders).unwrap();
        assert_eq!(load_senders(path).unwrap(), senders);
    }
}
