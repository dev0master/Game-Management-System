//! Storing API credentials with Windows DPAPI.
//!
//! The IGDB integration needs a Twitch client secret. Keeping it as plain text in the
//! catalogue would mean anyone with read access to the database file — including a
//! backup, or the drive it was copied to — has the secret. DPAPI encrypts it against
//! the current user account, so the file is useless anywhere else.

use std::path::PathBuf;
use windows_sys::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
};

/// Where secrets live: beside the catalogue, never on an external drive.
pub fn secrets_path() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("GameVault").join("secrets.bin")
}

/// Encrypt bytes for the current user.
pub fn protect(plain: &[u8]) -> Result<Vec<u8>, String> {
    let mut input = CRYPT_INTEGER_BLOB {
        cbData: plain.len() as u32,
        pbData: plain.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };

    let ok = unsafe {
        CryptProtectData(
            &mut input,
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            // Never prompt: this runs on a background thread with no window to own a
            // dialog, and a blocked prompt would hang the caller.
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!("CryptProtectData failed: {}", std::io::Error::last_os_error()));
    }

    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { windows_sys::Win32::Foundation::LocalFree(output.pbData as *mut _) };
    Ok(bytes)
}

/// Decrypt bytes previously produced by [`protect`] on this account.
pub fn unprotect(cipher: &[u8]) -> Result<Vec<u8>, String> {
    let mut input = CRYPT_INTEGER_BLOB {
        cbData: cipher.len() as u32,
        pbData: cipher.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };

    let ok = unsafe {
        CryptUnprotectData(
            &mut input,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err(format!("CryptUnprotectData failed: {}", std::io::Error::last_os_error()));
    }

    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe { windows_sys::Win32::Foundation::LocalFree(output.pbData as *mut _) };
    Ok(bytes)
}

/// The credentials the metadata provider needs.
///
/// One field, because RAWG authenticates with a single key on the query string. Every
/// field is `#[serde(default)]` so a secrets file written by an older build — which held
/// Twitch fields for IGDB — still deserialises instead of being read as corrupt.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct Credentials {
    #[serde(default)]
    pub rawg_api_key: String,
}

impl Credentials {
    pub fn is_complete(&self) -> bool {
        !self.rawg_api_key.trim().is_empty()
    }

    /// The key as it is safe to show: enough to recognise, not enough to reuse.
    ///
    /// The settings screen needs to say *which* key is stored without putting the secret
    /// back into webview memory and into every IPC log.
    pub fn masked(&self) -> String {
        let k = self.rawg_api_key.trim();
        if k.is_empty() {
            return String::new();
        }
        let tail: String = k.chars().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
        format!("••••••••{tail}")
    }
}

/// Read stored credentials. Returns defaults when nothing is saved yet, so a first run
/// needs no special case.
pub fn load_credentials() -> Credentials {
    let Ok(cipher) = std::fs::read(secrets_path()) else {
        return Credentials::default();
    };
    let Ok(plain) = unprotect(&cipher) else {
        // Written by a different user account, or corrupted. Treat as absent rather
        // than failing: the app works fine without credentials.
        return Credentials::default();
    };
    serde_json::from_slice(&plain).unwrap_or_default()
}

pub fn save_credentials(c: &Credentials) -> Result<(), String> {
    let path = secrets_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let plain = serde_json::to_vec(c).map_err(|e| e.to_string())?;
    let cipher = protect(&plain)?;
    std::fs::write(&path, cipher).map_err(|e| e.to_string())
}

pub fn clear_credentials() -> Result<(), String> {
    let path = secrets_path();
    if path.exists() {
        std::fs::remove_file(&path).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_secret_round_trips_through_dpapi() {
        let secret = b"twitch-client-secret-value";
        let cipher = protect(secret).expect("protect");
        assert_ne!(cipher.as_slice(), secret, "the stored form must not be plaintext");
        assert_eq!(unprotect(&cipher).expect("unprotect"), secret);
    }

    /// The point of DPAPI here: what lands on disk must not contain the secret.
    #[test]
    fn the_ciphertext_does_not_contain_the_plaintext() {
        let secret = b"SUPERSECRETTOKEN0123456789";
        let cipher = protect(secret).unwrap();
        let needle = b"SUPERSECRETTOKEN";
        assert!(
            !cipher.windows(needle.len()).any(|w| w == needle),
            "plaintext found in the encrypted blob"
        );
    }

    #[test]
    fn corrupted_data_fails_cleanly_rather_than_panicking() {
        let cipher = protect(b"x").unwrap();
        let mut broken = cipher.clone();
        for b in broken.iter_mut().take(8) {
            *b ^= 0xFF;
        }
        assert!(unprotect(&broken).is_err());
    }

    #[test]
    fn missing_credentials_are_reported_as_incomplete() {
        assert!(!Credentials::default().is_complete());
        assert!(Credentials { rawg_api_key: "abc123".into() }.is_complete());
        // Whitespace is not a credential.
        assert!(!Credentials { rawg_api_key: "   ".into() }.is_complete());
    }

    /// The settings screen has to name the stored key without handing it back.
    #[test]
    fn the_masked_key_shows_only_its_last_four_characters() {
        let c = Credentials { rawg_api_key: "11248e2f037d400bb7189b42bb02be5a".into() };
        let m = c.masked();
        assert!(m.ends_with("be5a"), "{m}");
        assert!(!m.contains("11248e2f"), "the secret must not survive masking: {m}");
        assert_eq!(Credentials::default().masked(), "");
    }

    /// A secrets file written by the previous build held Twitch fields. It must read as
    /// "no key saved" rather than as corruption, so the user simply enters a RAWG key.
    #[test]
    fn a_secrets_file_from_the_previous_provider_still_parses() {
        let old = br#"{"twitch_client_id":"abc","twitch_client_secret":"def"}"#;
        let c: Credentials = serde_json::from_slice(old).expect("older shape still parses");
        assert!(!c.is_complete());
        assert_eq!(c.rawg_api_key, "");
    }
}
