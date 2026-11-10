//! QR login via Discord's remote-auth gateway.
//!
//! Ported from discordo `internal/ui/login/qr/msg.go` + mrarfarf
//! `cmd/qr_login.go` (GPL — re-implemented here in Rust, not copied verbatim;
//! plan §7). Flow:
//!
//! ```text
//! wss://remote-auth-gateway.discord.gg/?v=2
//!   hello {heartbeat_interval, timeout_ms}
//!   -> init {encoded_public_key: base64 PKIX SPKI DER of RSA-2048 pub}
//!   <- nonce_proof {encrypted_nonce}  -> nonce_proof {nonce: base64url(OAEP dec)}
//!   <- pending_remote_init {fingerprint}   -> print QR https://discord.com/ra/{fp}
//!   <- pending_ticket {encrypted_user_payload}
//!   <- pending_login {ticket}
//! POST /api/v9/users/@me/remote-auth/login {ticket} + X-Fingerprint
//!   -> {encrypted_token} -> OAEP decrypt -> token
//! ```
//!
//! RSA-OAEP-SHA256, base64 Std for wire, RawURL for the decrypted nonce.
//! Note: the ticket exchange uses **api/v9** deliberately (arikawa Version=9,
//! review#15) — do NOT "fix" to v10.

use anyhow::{Context, Result};
use base64::Engine;
use rand::rngs::OsRng;
use rsa::pkcs8::EncodePublicKey;
use rsa::{Oaep, RsaPrivateKey, RsaPublicKey};
use sha2::Sha256;

/// Gateway URL for Discord's QR remote-auth (discordo msg.go:32).
const REMOTE_AUTH_URL: &str = "wss://remote-auth-gateway.discord.gg/?v=2";
/// Ticket exchange endpoint — api/v9 DELIBERATE (review#15).
const TICKET_EXCHANGE_URL: &str = "https://discord.com/api/v9/users/@me/remote-auth/login";

/// Browser UA used for the WS dial + ticket exchange.
fn browser_ua() -> String {
    discord_core::stealth::browser_user_agent()
}

/// Print an ASCII QR code for `data` to stderr (terminal-friendly).
fn print_ascii_qr(data: &str) -> Result<()> {
    let code = qrcode::QrCode::new(data.as_bytes()).context("qr encode")?;
    let render = code
        .render::<char>()
        .quiet_zone(true)
        .module_dimensions(2, 1)
        .build();
    eprintln!("Scan with Discord mobile:\n{render}");
    Ok(())
}

/// RSA-OAEP-SHA256 decrypt (base64 Std input -> base64 RawURL output for the
/// nonce; plain bytes for the user payload / token).
fn oaep_decrypt_b64(priv_key: &RsaPrivateKey, b64: &str) -> Result<Vec<u8>> {
    let cipher = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .context("b64 decode")?;
    let oaep = Oaep::new::<Sha256>();
    priv_key.decrypt(oaep, &cipher).context("OAEP decrypt")
}

/// Perform the full QR login; returns the authenticated token.
/// Prints the QR to stderr; waits up to `timeout_secs` for the scan.
pub async fn qr_login(timeout_secs: u64) -> Result<String> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut req = REMOTE_AUTH_URL
        .into_client_request()
        .context("ws request")?;
    let h = req.headers_mut();
    // Browser-like headers required by Cloudflare on remote-auth-gateway.
    // Without these the WebSocket upgrade is rejected with 403 (verified
    // empirically; mirrors discordo http.Headers()).
    h.insert("User-Agent", browser_ua().parse().context("ua header")?);
    h.insert("Accept", "*/*".parse().unwrap());
    h.insert("Accept-Language", "en-US,en;q=0.9".parse().unwrap());
    h.insert("Origin", "https://discord.com".parse().unwrap());
    h.insert(
        "Referer",
        "https://discord.com/channels/@me".parse().unwrap(),
    );
    h.insert("Sec-Fetch-Dest", "empty".parse().unwrap());
    h.insert("Sec-Fetch-Mode", "cors".parse().unwrap());
    h.insert("Sec-Fetch-Site", "same-origin".parse().unwrap());
    h.insert("X-Debug-Options", "bugReporterEnabled".parse().unwrap());
    h.insert("X-Discord-Locale", "en-US".parse().unwrap());
    let (ws, _) = tokio_tungstenite::connect_async(req)
        .await
        .context("ws connect")?;
    let (mut writer, mut reader) = ws.split();

    // RSA-2048 keypair; public key as PKIX SPKI DER, base64 Std.
    let mut rng = OsRng;
    let priv_key = RsaPrivateKey::new(&mut rng, 2048).context("rsa keygen")?;
    let pub_key = RsaPublicKey::from(&priv_key);
    let der = pub_key
        .to_public_key_der()
        .context("pubkey der")?
        .as_ref()
        .to_vec();
    let encoded_pub = base64::engine::general_purpose::STANDARD.encode(&der);

    // Heartbeat timer, created on `hello` from the server-provided
    // `heartbeat_interval` (was a hard-coded ≤5s tick).
    let mut heartbeat: Option<tokio::time::Interval> = None;
    let mut fingerprint: Option<String> = None;
    let mut ticket: Option<String> = None; // assigned in pending_login
    let _ = &ticket; // silence clippy: init value replaced before read

    // Deadline for the whole flow (stale QR protection, review#14).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(timeout_secs);

    loop {
        if std::time::Instant::now() > deadline {
            return Err(anyhow::anyhow!("QR login timed out after {timeout_secs}s"));
        }
        tokio::select! {
            // Server-paced heartbeat (no tick until `hello` arrives).
            _ = async {
                match heartbeat.as_mut() {
                    Some(iv) => iv.tick().await,
                    None => std::future::pending().await,
                }
            } => {
                writer
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        r#"{"op":"heartbeat"}"#.into(),
                    ))
                    .await
                    .context("send heartbeat")?;
            }
            msg = reader.next() => {
                let text = match msg {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(t))) => t,
                    Some(Ok(_)) => continue, // binary/ping/pong — ignore
                    Some(Err(e)) => return Err(e).context("ws read"),
                    None => return Err(anyhow::anyhow!("ws closed")),
                };
                let v: serde_json::Value =
                    serde_json::from_str(&text).with_context(|| format!("bad json: {text}"))?;
                let op = v["op"].as_str().unwrap_or_default();
                match op {
            "hello" => {
                let interval_ms = v["heartbeat_interval"].as_u64().unwrap_or(41_250);
                // Arm the heartbeat at the server-provided interval; skip
                // the immediate first tick (intervals fire instantly).
                let mut iv = tokio::time::interval(std::time::Duration::from_millis(
                    interval_ms.max(1),
                ));
                iv.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                iv.tick().await;
                heartbeat = Some(iv);
                // Send init with our public key.
                let init = serde_json::json!({
                    "op": "init",
                    "encoded_public_key": encoded_pub,
                });
                writer
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        init.to_string(),
                    ))
                    .await
                    .context("send init")?;
            }
            "nonce_proof" => {
                let enc = v["encrypted_nonce"]
                    .as_str()
                    .context("missing encrypted_nonce")?;
                let dec = oaep_decrypt_b64(&priv_key, enc)?;
                let nonce_b64url = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&dec);
                let proof = serde_json::json!({
                    "op": "nonce_proof",
                    "nonce": nonce_b64url,
                });
                writer
                    .send(tokio_tungstenite::tungstenite::Message::Text(
                        proof.to_string(),
                    ))
                    .await
                    .context("send nonce_proof")?;
            }
            "pending_remote_init" => {
                let fp = v["fingerprint"].as_str().context("missing fingerprint")?;
                fingerprint = Some(fp.to_string());
                print_ascii_qr(&format!("https://discord.com/ra/{fp}"))?;
            }
            "pending_ticket" => {
                // encrypted_user_payload splits on ':' → [1]=discriminator,
                // [3]=username (display only; see plan §QR flow).
                let enc = v["encrypted_user_payload"].as_str().unwrap_or_default();
                if let Ok(dec) = oaep_decrypt_b64(&priv_key, enc) {
                    if let Ok(plain) = String::from_utf8(dec) {
                        let parts: Vec<&str> = plain.split(':').collect();
                        let user = parts
                            .get(3)
                            .or_else(|| parts.get(1))
                            .copied()
                            .unwrap_or("?");
                        eprintln!("Waiting for \"{user}\" to scan...");
                    }
                }
            }
            "pending_login" => {
                ticket = Some(v["ticket"].as_str().context("missing ticket")?.to_string());
                break;
            }
            "cancel" => return Err(anyhow::anyhow!("login cancelled on device")),
            other => {
                eprintln!("unhandled remote-auth op: {other}");
            }
                }
            }
        }
    }

    // Exchange ticket -> encrypted_token.
    let ticket = ticket.context("no ticket")?;
    let fp = fingerprint.context("no fingerprint")?;
    let client = reqwest::Client::builder()
        .user_agent(browser_ua())
        .build()
        .context("http client")?;
    let resp = client
        .post(TICKET_EXCHANGE_URL)
        .header("X-Fingerprint", &fp)
        .header("Referer", format!("https://discord.com/ra/{fp}"))
        .json(&serde_json::json!({ "ticket": ticket }))
        .timeout(std::time::Duration::from_secs(15))
        .send()
        .await
        .context("ticket exchange request")?;
    if !resp.status().is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(anyhow::anyhow!("ticket exchange failed: {body}"));
    }
    let body: serde_json::Value = resp.json().await.context("ticket response json")?;
    let enc_token = body["encrypted_token"]
        .as_str()
        .context("missing encrypted_token")?;
    let token_bytes = oaep_decrypt_b64(&priv_key, enc_token)?;
    let token = String::from_utf8(token_bytes).context("token utf8")?;
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rsa::pkcs8::DecodePublicKey;

    #[test]
    fn rsa_oaep_roundtrip() {
        let mut rng = OsRng;
        let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let pub_key = RsaPublicKey::from(&priv_key);
        // Encrypt with OAEP-SHA256, decrypt with our helper path.
        let oaep = Oaep::new::<Sha256>();
        let secret = b"hello-discord-qr";
        let cipher = pub_key.encrypt(&mut rng, oaep, secret).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&cipher);
        let dec = oaep_decrypt_b64(&priv_key, &b64).unwrap();
        assert_eq!(dec, secret);
    }

    #[test]
    fn pubkey_der_is_pkix_spki() {
        let mut rng = OsRng;
        let priv_key = RsaPrivateKey::new(&mut rng, 2048).unwrap();
        let pub_key = RsaPublicKey::from(&priv_key);
        let der = pub_key.to_public_key_der().unwrap();
        // Decode back to confirm PKIX SPKI structure.
        let decoded = RsaPublicKey::from_public_key_der(der.as_ref()).unwrap();
        assert_eq!(decoded, pub_key);
    }

    #[test]
    fn nonce_b64url_matches_rawurl() {
        let bytes = vec![0x12, 0x34, 0xAB, 0xCD, 0xFF];
        let url = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes);
        // RawURLEncoding in Go == URL_SAFE_NO_PAD in Rust (no padding, -_).
        assert!(!url.contains('+') && !url.contains('/') && !url.contains('='));
    }
}
