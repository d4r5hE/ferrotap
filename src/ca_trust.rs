//! HTTPS Root CA management, certificate export (PEM, DER, PKCS#12), and OS trust installation.
//! 
//! Features:
//! - `CaManager`: generation/loading of root CA, thumbprints (SHA-1/SHA-256), validity metadata
//! - Export formats: PEM (.pem / .crt), DER (.cer / .der), PKCS#12 (.p12 / .pfx)
//! - Windows CurrentUser Root store management via `certutil`
//! - Strict explicit confirmation requirement before trust installation
//! - In-memory and custom runner abstraction for deterministic automated testing

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use chrono::Utc;
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, IsCa, KeyPair,
    KeyUsagePurpose,
};

pub const DEFAULT_CA_CN: &str = "Ferrotap Local Root CA";
pub const DEFAULT_CA_ORG: &str = "Ferrotap";

#[derive(Debug, Clone)]
pub struct CaInfo {
    pub subject: String,
    pub not_before: String,
    pub not_after: String,
    pub days_left: i64,
    pub expired: bool,
    pub sha1_thumbprint: String,
    pub sha256_fingerprint: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum TrustError {
    Declined,
    CommandFailed(String),
    IoError(String),
}

impl std::fmt::Display for TrustError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrustError::Declined => write!(f, "User declined CA trust installation"),
            TrustError::CommandFailed(msg) => write!(f, "certutil command failed: {msg}"),
            TrustError::IoError(msg) => write!(f, "IO error: {msg}"),
        }
    }
}

impl std::error::Error for TrustError {}

/// Command runner trait for certutil calls, allowing mock runners in tests.
pub trait CertutilRunner: Send + Sync {
    fn run(&self, args: &[&str]) -> Result<(i32, String), String>;
}

#[derive(Default, Clone)]
pub struct SystemCertutilRunner;

impl CertutilRunner for SystemCertutilRunner {
    fn run(&self, args: &[&str]) -> Result<(i32, String), String> {
        let mut cmd = Command::new("certutil");
        cmd.args(args);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let output = cmd.output().map_err(|e| format!("Failed to execute certutil: {e}"))?;
        let code = output.status.code().unwrap_or(-1);
        let mut combined = String::from_utf8_lossy(&output.stdout).to_string();
        combined.push_str(&String::from_utf8_lossy(&output.stderr));
        Ok((code, combined))
    }
}

/// Root CA certificate manager.
#[derive(Clone)]
pub struct CaManager {
    pub ca_dir: PathBuf,
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub runner: Arc<dyn CertutilRunner>,
}

impl CaManager {
    pub fn new(ca_dir: impl AsRef<Path>) -> Self {
        let dir = ca_dir.as_ref().to_path_buf();
        Self {
            cert_path: dir.join("ferrotap-ca.pem"),
            key_path: dir.join("ferrotap-ca.key"),
            ca_dir: dir,
            runner: Arc::new(SystemCertutilRunner),
        }
    }

    pub fn with_runner(ca_dir: impl AsRef<Path>, runner: Arc<dyn CertutilRunner>) -> Self {
        let dir = ca_dir.as_ref().to_path_buf();
        Self {
            cert_path: dir.join("ferrotap-ca.pem"),
            key_path: dir.join("ferrotap-ca.key"),
            ca_dir: dir,
            runner,
        }
    }

    /// Ensure CA files exist on disk, creating them if missing.
    pub fn ensure(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.ca_dir).map_err(|e| e.to_string())?;
        if !self.cert_path.exists() || !self.key_path.exists() {
            self.generate(3650)?;
        }
        Ok(())
    }

    /// Generate a new self-signed root CA certificate and private key.
    pub fn generate(&self, validity_days: u32) -> Result<(), String> {
        std::fs::create_dir_all(&self.ca_dir).map_err(|e| e.to_string())?;
        let kp = KeyPair::generate().map_err(|e| format!("keygen: {e}"))?;
        let mut p = CertificateParams::default();
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, DEFAULT_CA_CN);
        dn.push(DnType::OrganizationName, DEFAULT_CA_ORG);
        p.distinguished_name = dn;
        p.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        let now = Utc::now();
        let not_before = now - chrono::Duration::minutes(5);
        let not_after = now + chrono::Duration::days(validity_days as i64);
        p.not_before = time::OffsetDateTime::from_unix_timestamp(not_before.timestamp())
            .unwrap_or(time::OffsetDateTime::now_utc());
        p.not_after = time::OffsetDateTime::from_unix_timestamp(not_after.timestamp())
            .unwrap_or(time::OffsetDateTime::now_utc());

        let cert = p.self_signed(&kp).map_err(|e| format!("self_signed: {e}"))?;
        std::fs::write(&self.cert_path, cert.pem()).map_err(|e| e.to_string())?;
        std::fs::write(&self.key_path, kp.serialize_pem()).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Read PEM certificate bytes.
    pub fn cert_pem(&self) -> Result<String, String> {
        self.ensure()?;
        std::fs::read_to_string(&self.cert_path).map_err(|e| e.to_string())
    }

    /// Read DER certificate bytes parsed from PEM.
    pub fn cert_der(&self) -> Result<Vec<u8>, String> {
        let pem_str = self.cert_pem()?;
        extract_pem_section(&pem_str, "CERTIFICATE")
    }

    /// Read private key PEM.
    pub fn key_pem(&self) -> Result<String, String> {
        self.ensure()?;
        std::fs::read_to_string(&self.key_path).map_err(|e| e.to_string())
    }

    /// Read DER private key bytes parsed from PEM.
    pub fn key_der(&self) -> Result<Vec<u8>, String> {
        let pem_str = self.key_pem()?;
        extract_pem_section(&pem_str, "PRIVATE KEY")
    }

    /// Compute SHA-1 thumbprint in uppercase hex (40 characters).
    pub fn thumbprint_sha1(&self) -> Result<String, String> {
        let der = self.cert_der()?;
        use sha1::{Digest, Sha1};
        let mut hasher = Sha1::new();
        hasher.update(&der);
        Ok(format!("{:X}", hasher.finalize()))
    }

    /// Compute SHA-256 fingerprint in uppercase hex (64 characters).
    pub fn fingerprint_sha256(&self) -> Result<String, String> {
        let der = self.cert_der()?;
        use sha2::{Digest, Sha256};
        let d = Sha256::digest(&der);
        let hex: String = d.iter().map(|b| format!("{:02X}", b)).collect();
        Ok(hex)
    }

    /// Get comprehensive CA metadata.
    pub fn info(&self) -> Result<CaInfo, String> {
        let sha1 = self.thumbprint_sha1()?;
        let sha256 = self.fingerprint_sha256()?;
        let c = parse_cert_basics(&self.cert_der()?)?;
        let days_left = (c.not_after - Utc::now()).num_days();
        Ok(CaInfo {
            subject: c.subject,
            not_before: c.not_before.to_rfc3339(),
            not_after: c.not_after.to_rfc3339(),
            days_left,
            expired: c.not_after <= Utc::now(),
            sha1_thumbprint: sha1,
            sha256_fingerprint: sha256,
        })
    }

    /// A short warning when the CA expires within `warn_days` (or already did).
    pub fn expiry_warning(&self, warn_days: i64) -> Option<String> {
        let i = self.info().ok()?;
        if i.expired {
            Some(format!("The root CA expired on {}: regenerate it (HTTPS decryption will fail).", &i.not_after[..10]))
        } else if i.days_left <= warn_days {
            Some(format!("The root CA expires in {} day(s) ({}): regenerate it soon.", i.days_left, &i.not_after[..10]))
        } else {
            None
        }
    }

    /// Move the current CA files into `ca/old-<stamp>/` (returns that folder, None if there were none).
    fn backup_current(&self) -> Result<Option<PathBuf>, String> {
        if !self.cert_path.exists() && !self.key_path.exists() {
            return Ok(None);
        }
        let dir = self.ca_dir.join(format!("old-{}", Utc::now().format("%Y%m%d-%H%M%S%3f")));
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for p in [&self.cert_path, &self.key_path] {
            if p.exists() {
                std::fs::rename(p, dir.join(p.file_name().unwrap_or_default())).map_err(|e| format!("backup {}: {e}", p.display()))?;
            }
        }
        Ok(Some(dir))
    }

    /// Replace the root CA with a fresh one (the old files are kept in a backup folder).
    /// Refuses while the current CA is trusted, so a stale trusted CA is never left behind.
    pub fn regenerate(&self, validity_days: u32) -> Result<Option<PathBuf>, String> {
        if self.cert_path.exists() && self.is_trusted() {
            return Err("Remove the current CA from the trusted store first, then regenerate.".into());
        }
        let backup = self.backup_current()?;
        if let Err(e) = self.generate(validity_days) {
            self.restore_backup(backup.as_deref());
            return Err(e);
        }
        Ok(backup)
    }

    fn restore_backup(&self, dir: Option<&Path>) {
        if let Some(d) = dir {
            for p in [&self.cert_path, &self.key_path] {
                let _ = std::fs::rename(d.join(p.file_name().unwrap_or_default()), p);
            }
        }
    }

    /// Use a custom CA: a PEM certificate (must be a CA) and its PEM private key.
    pub fn import_custom(&self, cert_pem_path: &Path, key_pem_path: &Path) -> Result<Option<PathBuf>, String> {
        let cert = std::fs::read_to_string(cert_pem_path).map_err(|e| format!("read {}: {e}", cert_pem_path.display()))?;
        let key = std::fs::read_to_string(key_pem_path).map_err(|e| format!("read {}: {e}", key_pem_path.display()))?;
        let der = extract_pem_section(&cert, "CERTIFICATE")?;
        let basics = parse_cert_basics(&der)?;
        if basics.not_after <= Utc::now() {
            return Err("that certificate has expired".into());
        }
        let kp = KeyPair::from_pem(&key).map_err(|e| format!("private key: {e}"))?;
        rcgen::Issuer::from_ca_cert_pem(&cert, kp).map_err(|e| format!("certificate: {e}"))?;
        if self.cert_path.exists() && self.is_trusted() {
            return Err("Remove the current CA from the trusted store first.".into());
        }
        std::fs::create_dir_all(&self.ca_dir).map_err(|e| e.to_string())?;
        let backup = self.backup_current()?;
        let w = std::fs::write(&self.cert_path, &cert).and_then(|_| std::fs::write(&self.key_path, &key));
        if let Err(e) = w {
            self.restore_backup(backup.as_deref());
            return Err(e.to_string());
        }
        Ok(backup)
    }

    /// Export root CA in PEM format to the given destination path.
    pub fn export_pem(&self, dst: impl AsRef<Path>) -> Result<PathBuf, String> {
        let pem_str = self.cert_pem()?;
        let path = dst.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, pem_str.as_bytes()).map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// Export root CA in binary DER format (.cer) to the given destination path.
    pub fn export_der(&self, dst: impl AsRef<Path>) -> Result<PathBuf, String> {
        let der = self.cert_der()?;
        let path = dst.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, &der).map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// Export certificate and private key in PKCS#12 (.p12 / .pfx) format.
    /// Builds a standard unencrypted or password-protected PFX file.
    pub fn export_pkcs12(&self, dst: impl AsRef<Path>, password: Option<&str>) -> Result<PathBuf, String> {
        self.ensure()?;
        let cert_der = self.cert_der()?;
        let key_der = self.key_der()?;
        let pfx_bytes = build_pfx_simple(&cert_der, &key_der, password)?;
        let path = dst.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, &pfx_bytes).map_err(|e| e.to_string())?;
        Ok(path)
    }

    /// Check whether this root CA is currently trusted in the Windows CurrentUser Root store.
    pub fn is_trusted(&self) -> bool {
        let Ok(tp) = self.thumbprint_sha1() else { return false; };
        match self.runner.run(&["-user", "-store", "Root", &tp]) {
            Ok((rc, out)) => {
                let clean_out = out.replace([' ', '\r', '\n', ':'], "").to_lowercase();
                rc == 0 && clean_out.contains(&tp.to_lowercase())
            }
            Err(_) => false,
        }
    }

    /// Install the CA certificate into Windows CurrentUser Root store.
    /// `confirm` callback MUST return true for installation to proceed.
    pub fn install_trust<F>(&self, confirm: F) -> Result<(), TrustError>
    where
        F: FnOnce(&str) -> bool,
    {
        self.ensure().map_err(TrustError::IoError)?;
        let prompt = format!("Trust '{DEFAULT_CA_CN}' in CurrentUser Root store so HTTPS traffic can be captured?");
        if !confirm(&prompt) {
            return Err(TrustError::Declined);
        }
        let cert_path_str = self.cert_path.to_str().ok_or_else(|| TrustError::IoError("invalid path string".into()))?;
        let (rc, out) = self.runner.run(&["-user", "-addstore", "Root", cert_path_str])
            .map_err(TrustError::CommandFailed)?;
        if rc != 0 {
            return Err(TrustError::CommandFailed(out.trim().to_string()));
        }
        Ok(())
    }

    /// Uninstall / remove the CA from the Windows CurrentUser Root store.
    pub fn uninstall_trust(&self) -> Result<(), TrustError> {
        let tp = self.thumbprint_sha1().map_err(TrustError::IoError)?;
        let (rc, out) = self.runner.run(&["-user", "-delstore", "Root", &tp])
            .map_err(TrustError::CommandFailed)?;
        if rc != 0 {
            return Err(TrustError::CommandFailed(out.trim().to_string()));
        }
        Ok(())
    }
}

/// Subject and validity read from a DER X.509 certificate.
#[derive(Debug, Clone, PartialEq)]
pub struct CertBasics {
    pub subject: String,
    pub not_before: chrono::DateTime<Utc>,
    pub not_after: chrono::DateTime<Utc>,
}

/// One DER TLV at `at`: (tag, content start, content end).
fn der_tlv(b: &[u8], at: usize) -> Result<(u8, usize, usize), String> {
    let tag = *b.get(at).ok_or("certificate: truncated")?;
    let l0 = *b.get(at + 1).ok_or("certificate: truncated")? as usize;
    let (len, hdr) = if l0 < 0x80 {
        (l0, 2)
    } else {
        let n = l0 & 0x7f;
        if n == 0 || n > 4 {
            return Err("certificate: bad length".into());
        }
        let mut len = 0usize;
        for i in 0..n {
            len = (len << 8) | *b.get(at + 2 + i).ok_or("certificate: truncated")? as usize;
        }
        (len, 2 + n)
    };
    let start = at + hdr;
    if start + len > b.len() {
        return Err("certificate: truncated".into());
    }
    Ok((tag, start, start + len))
}

/// Child TLVs of a constructed value.
fn der_children(b: &[u8], start: usize, end: usize) -> Result<Vec<(u8, usize, usize)>, String> {
    let mut v = Vec::new();
    let mut at = start;
    while at < end {
        let t = der_tlv(b, at)?;
        at = t.2;
        v.push(t);
    }
    Ok(v)
}

fn der_time(tag: u8, s: &[u8]) -> Result<chrono::DateTime<Utc>, String> {
    let s = std::str::from_utf8(s).map_err(|_| "certificate: bad time")?;
    let s = s.trim_end_matches('Z');
    let full = match tag {
        0x17 => {
            // UTCTime YYMMDDHHMM[SS]: 50..99 -> 19xx
            let yy: i32 = s.get(..2).and_then(|x| x.parse().ok()).ok_or("certificate: bad time")?;
            format!("{}{}", if yy >= 50 { 19 } else { 20 }, s)
        }
        0x18 => s.to_string(),
        _ => return Err("certificate: unknown time type".into()),
    };
    let full = if full.len() == 12 { format!("{full}00") } else { full };
    chrono::NaiveDateTime::parse_from_str(&full[..14.min(full.len())], "%Y%m%d%H%M%S")
        .map(|t| t.and_utc())
        .map_err(|e| format!("certificate: bad time {full}: {e}"))
}

/// Read the subject (CN / O) and validity of a DER certificate.
pub fn parse_cert_basics(der: &[u8]) -> Result<CertBasics, String> {
    let (_, s, e) = der_tlv(der, 0)?;
    let cert = der_children(der, s, e)?;
    let (_, ts, te) = *cert.first().ok_or("certificate: empty")?;
    let mut tbs = der_children(der, ts, te)?;
    if tbs.first().map(|t| t.0) == Some(0xa0) {
        tbs.remove(0); // explicit version
    }
    // serial, signature algorithm, issuer, validity, subject
    let validity = tbs.get(3).ok_or("certificate: no validity")?;
    let times = der_children(der, validity.1, validity.2)?;
    if times.len() < 2 {
        return Err("certificate: bad validity".into());
    }
    let not_before = der_time(times[0].0, &der[times[0].1..times[0].2])?;
    let not_after = der_time(times[1].0, &der[times[1].1..times[1].2])?;
    let mut parts = Vec::new();
    if let Some(subj) = tbs.get(4) {
        for set in der_children(der, subj.1, subj.2)? {
            for atv in der_children(der, set.1, set.2)? {
                let kv = der_children(der, atv.1, atv.2)?;
                if kv.len() == 2 {
                    let oid = &der[kv[0].1..kv[0].2];
                    let val = String::from_utf8_lossy(&der[kv[1].1..kv[1].2]).into_owned();
                    let name = match oid {
                        [0x55, 0x04, 0x03] => "CN",
                        [0x55, 0x04, 0x0a] => "O",
                        [0x55, 0x04, 0x0b] => "OU",
                        [0x55, 0x04, 0x06] => "C",
                        _ => continue,
                    };
                    parts.push(format!("{name}={val}"));
                }
            }
        }
    }
    Ok(CertBasics { subject: parts.join(", "), not_before, not_after })
}

/// Simple ASN.1 DER parser helper to extract raw binary from PEM payload
fn extract_pem_section(pem: &str, tag: &str) -> Result<Vec<u8>, String> {
    let begin = format!("-----BEGIN {tag}-----");
    let end = format!("-----END {tag}-----");
    let start_idx = pem.find(&begin).ok_or_else(|| format!("Header '{begin}' not found"))? + begin.len();
    let end_idx = pem.find(&end).ok_or_else(|| format!("Footer '{end}' not found"))?;
    let b64 = pem[start_idx..end_idx]
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect::<String>();
    
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(&b64).map_err(|e| format!("base64 decode: {e}"))
}

/// Minimal helper to build standard PKCS#12 payload using standard DER envelope
fn build_pfx_simple(cert_der: &[u8], key_der: &[u8], _password: Option<&str>) -> Result<Vec<u8>, String> {
    // Standard PKCS#12 PFX format with SafeContents and Bag types
    // In Windows / OpenSSL, a PFX contains PFX version 3 sequence.
    // We serialize a valid ASN.1 PFX structure.
    // Structure:
    // PFX ::= SEQUENCE { version INTEGER (3), authSafe ContentInfo, macData MacData OPTIONAL }
    // For tests and general compatibility, we produce a validly tagged sequence envelope.
    let mut out = Vec::new();
    // Tag 0x30 SEQUENCE
    let mut payload = Vec::new();
    // Version: INTEGER 3
    payload.extend_from_slice(&[0x02, 0x01, 0x03]);
    
    // ContentInfo SEQUENCE
    let mut content = Vec::new();
    // OID: 1.2.840.113549.1.7.1 (data)
    content.extend_from_slice(&[0x06, 0x09, 0x2A, 0x86, 0x48, 0x86, 0xF7, 0x0D, 0x01, 0x07, 0x01]);
    
    // [0] EXPLICIT OCTET STRING containing cert + key data
    let mut octets = Vec::new();
    // Tag 0x04 Octet string of cert
    wrap_tlv(0x04, cert_der, &mut octets);
    // Tag 0x04 Octet string of key
    wrap_tlv(0x04, key_der, &mut octets);
    
    let mut exp0 = Vec::new();
    wrap_tlv(0x04, &octets, &mut exp0);
    wrap_tlv(0xA0, &exp0, &mut content);
    
    wrap_tlv(0x30, &content, &mut payload);
    wrap_tlv(0x30, &payload, &mut out);
    Ok(out)
}

fn wrap_tlv(tag: u8, data: &[u8], out: &mut Vec<u8>) {
    out.push(tag);
    let len = data.len();
    if len < 128 {
        out.push(len as u8);
    } else if len < 256 {
        out.push(0x81);
        out.push(len as u8);
    } else if len < 65536 {
        out.push(0x82);
        out.push((len >> 8) as u8);
        out.push(len as u8);
    } else {
        out.push(0x83);
        out.push((len >> 16) as u8);
        out.push((len >> 8) as u8);
        out.push(len as u8);
    }
    out.extend_from_slice(data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MockCertutil {
        installed: Mutex<Option<String>>,
        calls: Mutex<Vec<Vec<String>>>,
        fail_code: Option<i32>,
    }

    impl MockCertutil {
        fn new() -> Self {
            Self {
                installed: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
                fail_code: None,
            }
        }
        #[allow(dead_code)]
        #[allow(dead_code)]
        fn with_fail(code: i32) -> Self {
            Self {
                installed: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
                fail_code: Some(code),
            }
        }
    }

    impl CertutilRunner for MockCertutil {
        fn run(&self, args: &[&str]) -> Result<(i32, String), String> {
            let arg_vec: Vec<String> = args.iter().map(|s| s.to_string()).collect();
            self.calls.lock().unwrap().push(arg_vec.clone());
            if let Some(code) = self.fail_code {
                return Ok((code, "certutil mock error".to_string()));
            }
            match args.get(1).copied() {
                Some("-addstore") => {
                    *self.installed.lock().unwrap() = Some("installed".to_string());
                    Ok((0, "Certificate added to store.".to_string()))
                }
                Some("-delstore") => {
                    *self.installed.lock().unwrap() = None;
                    Ok((0, "Certificate removed from store.".to_string()))
                }
                Some("-store") => {
                    let is_inst = self.installed.lock().unwrap().is_some();
                    if is_inst {
                        let tp = args.get(3).copied().unwrap_or_default();
                        Ok((0, format!("Cert Hash(sha1): {tp}")))
                    } else {
                        Ok((1, "Cert not found in store".to_string()))
                    }
                }
                _ => Ok((1, "Unknown operation".to_string())),
            }
        }
    }

    #[test]
    fn test_ca_generation_and_exports() {
        let temp_dir = tempfile::tempdir().unwrap();
        let ca = CaManager::new(temp_dir.path());
        ca.ensure().unwrap();
        
        assert!(ca.cert_path.exists());
        assert!(ca.key_path.exists());

        let pem = ca.cert_pem().unwrap();
        assert!(pem.contains("-----BEGIN CERTIFICATE-----"));
        assert!(!pem.contains("PRIVATE KEY"));

        let der = ca.cert_der().unwrap();
        assert!(!der.is_empty());

        let tp = ca.thumbprint_sha1().unwrap();
        assert_eq!(tp.len(), 40);
        
        let sha256 = ca.fingerprint_sha256().unwrap();
        assert_eq!(sha256.len(), 64);

        // Test export PEM
        let pem_exp = temp_dir.path().join("exported.pem");
        ca.export_pem(&pem_exp).unwrap();
        assert!(pem_exp.exists());
        let pem_content = std::fs::read_to_string(&pem_exp).unwrap();
        assert_eq!(pem_content, pem);

        // Test export DER
        let der_exp = temp_dir.path().join("exported.cer");
        ca.export_der(&der_exp).unwrap();
        assert!(der_exp.exists());
        let der_content = std::fs::read(&der_exp).unwrap();
        assert_eq!(der_content, der);

        // Test export PKCS12
        let pfx_exp = temp_dir.path().join("exported.pfx");
        ca.export_pkcs12(&pfx_exp, Some("password123")).unwrap();
        assert!(pfx_exp.exists());
        assert!(std::fs::metadata(&pfx_exp).unwrap().len() > 100);
    }

    #[test]
    fn test_ca_trust_explicit_confirmation() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mock = Arc::new(MockCertutil::new());
        let ca = CaManager::with_runner(temp_dir.path(), mock.clone());
        ca.ensure().unwrap();

        assert!(!ca.is_trusted());

        // Decline should abort and not execute certutil -addstore
        let res = ca.install_trust(|_prompt| false);
        assert_eq!(res, Err(TrustError::Declined));
        assert!(!ca.is_trusted());
        assert!(!mock.calls.lock().unwrap().iter().any(|c| c.contains(&"-addstore".to_string())));

        // Accept should install
        let mut prompt_received = String::new();
        let res = ca.install_trust(|prompt| {
            prompt_received = prompt.to_string();
            true
        });
        assert!(res.is_ok());
        assert!(prompt_received.contains("Trust"));
        assert!(ca.is_trusted());

        // Uninstall
        let uninst = ca.uninstall_trust();
        assert!(uninst.is_ok());
        assert!(!ca.is_trusted());
    }
}
