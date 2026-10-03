//! API-key vault: AES-256-GCM, blob = 12-byte random nonce || ciphertext. Provider name is bound as AAD.
use aes_gcm::{aead::{Aead, KeyInit, Payload}, Aes256Gcm, Nonce};
use anyhow::{anyhow, bail, Result};
use rand::RngCore;
use std::{io::Write, os::unix::fs::{OpenOptionsExt, PermissionsExt}, path::Path};

/// Reads the 32-byte master key, creating it (mode 0600) on first use. Refuses group/other-accessible files.
fn master(path: &Path) -> Result<[u8; 32]> {
    if !path.exists() {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut k = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut k);
        match std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path) {
            Ok(mut f) => { f.write_all(&k)?; f.sync_all()?; }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {} // lost a creation race; read theirs
            Err(e) => return Err(e.into()),
        }
    }
    if std::fs::metadata(path)?.permissions().mode() & 0o077 != 0 {
        bail!("master key file {} is group/other accessible; chmod 600 it", path.display());
    }
    std::fs::read(path)?.try_into().map_err(|_| anyhow!("master key file must be exactly 32 bytes"))
}

pub fn encrypt(path: &Path, provider: &str, plain: &str) -> Result<Vec<u8>> {
    let c = Aes256Gcm::new(&master(path)?.into());
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let ct = c.encrypt(Nonce::from_slice(&nonce), Payload { msg: plain.as_bytes(), aad: provider.as_bytes() }).map_err(|_| anyhow!("encrypt failed"))?;
    Ok([nonce.as_slice(), &ct].concat())
}

pub fn decrypt(path: &Path, provider: &str, blob: &[u8]) -> Result<String> {
    if blob.len() < 12 + 16 {
        bail!("stored key is corrupt");
    }
    let c = Aes256Gcm::new(&master(path)?.into());
    let pt = c.decrypt(Nonce::from_slice(&blob[..12]), Payload { msg: &blob[12..], aad: provider.as_bytes() }).map_err(|_| anyhow!("cannot decrypt stored key (wrong master key?)"))?;
    Ok(String::from_utf8(pt)?)
}

/// Env var names for a provider, in priority order.
pub fn env_names(provider: &str) -> &'static [&'static str] {
    match provider {
        "anthropic" => &["ANTHROPIC_API_KEY"],
        "gemini" => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        _ => &[],
    }
}

/// Key from the environment (never ANTHROPIC_BASE_URL: that points at a different proxy).
pub fn env_key(provider: &str) -> Option<String> {
    env_names(provider).iter().find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()))
}
