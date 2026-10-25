//! G6 password-protected snapshots: AES-256-GCM with a key from PBKDF2-HMAC-SHA256
//! (random salt and nonce per file). An encrypted snapshot keeps its .ftap name; its text is
//! `FTAPENC1` + newline + base64(salt | nonce | ciphertext+tag).
use aws_lc_rs::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use aws_lc_rs::rand::{SecureRandom, SystemRandom};
use base64::Engine as _;
use std::num::NonZeroU32;
use std::path::Path;

pub const MAGIC: &str = "FTAPENC1";
const SALT_LEN: usize = 16;
const ITERATIONS: u32 = 210_000;

fn key(password: &str, salt: &[u8]) -> Result<LessSafeKey, String> {
    let mut k = [0u8; 32];
    aws_lc_rs::pbkdf2::derive(aws_lc_rs::pbkdf2::PBKDF2_HMAC_SHA256, NonZeroU32::new(ITERATIONS).expect("iterations"), salt, password.as_bytes(), &mut k);
    let unbound = UnboundKey::new(&AES_256_GCM, &k).map_err(|_| "bad key".to_string())?;
    Ok(LessSafeKey::new(unbound))
}

pub fn is_encrypted_text(text: &str) -> bool {
    text.trim_start_matches('\u{feff}').starts_with(MAGIC)
}

/// Encrypt `plain` with `password`.
pub fn encrypt(plain: &str, password: &str) -> Result<String, String> {
    if password.is_empty() {
        return Err("the password is empty".into());
    }
    let rng = SystemRandom::new();
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    rng.fill(&mut salt).map_err(|_| "no randomness".to_string())?;
    rng.fill(&mut nonce).map_err(|_| "no randomness".to_string())?;
    let mut data = plain.as_bytes().to_vec();
    key(password, &salt)?
        .seal_in_place_append_tag(Nonce::assume_unique_for_key(nonce), Aad::from(MAGIC.as_bytes()), &mut data)
        .map_err(|_| "encryption failed".to_string())?;
    let mut blob = Vec::with_capacity(SALT_LEN + NONCE_LEN + data.len());
    blob.extend_from_slice(&salt);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&data);
    Ok(format!("{MAGIC}\n{}\n", base64::engine::general_purpose::STANDARD.encode(blob)))
}

/// Decrypt text produced by `encrypt`. A wrong password and a damaged file give the same error.
pub fn decrypt(text: &str, password: &str) -> Result<String, String> {
    let body = text.trim_start_matches('\u{feff}').strip_prefix(MAGIC).ok_or("not an encrypted snapshot")?;
    let blob = base64::engine::general_purpose::STANDARD.decode(body.trim()).map_err(|_| "the encrypted snapshot is damaged".to_string())?;
    if blob.len() < SALT_LEN + NONCE_LEN + 16 {
        return Err("the encrypted snapshot is damaged".into());
    }
    let (salt, rest) = blob.split_at(SALT_LEN);
    let (nonce, ct) = rest.split_at(NONCE_LEN);
    let mut data = ct.to_vec();
    let nonce = Nonce::try_assume_unique_for_key(nonce).map_err(|_| "bad nonce".to_string())?;
    let plain = key(password, salt)?
        .open_in_place(nonce, Aad::from(MAGIC.as_bytes()), &mut data)
        .map_err(|_| "wrong password (or the file was changed)".to_string())?;
    String::from_utf8(plain.to_vec()).map_err(|_| "the decrypted snapshot is not text".to_string())
}

pub fn is_encrypted(path: &Path) -> bool {
    std::fs::read(path).map(|b| b.starts_with(MAGIC.as_bytes()) || b.get(3..).is_some_and(|r| r.starts_with(MAGIC.as_bytes()))).unwrap_or(false)
}

/// Set, change or remove a snapshot's password. `old` is needed when it is protected now;
/// `new` = None stores it unprotected again.
pub fn set_password(path: &Path, old: Option<&str>, new: Option<&str>) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let plain = if is_encrypted_text(&text) { decrypt(&text, old.ok_or("this snapshot needs its current password")?)? } else { text };
    let out = match new {
        Some(p) => encrypt(&plain, p)?,
        None => plain,
    };
    std::fs::write(path, out).map_err(|e| format!("write {}: {e}", path.display()))
}

/// Read a snapshot's sessions, decrypting with `password` when it is protected.
pub fn load(path: &Path, password: Option<&str>) -> Result<Vec<crate::model::SessionRecord>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if is_encrypted_text(&text) {
        let p = password.ok_or("this snapshot is password protected")?;
        crate::data_io::parse_ftap(&decrypt(&text, p)?)
    } else {
        crate::data_io::parse_ftap(&text)
    }
}
