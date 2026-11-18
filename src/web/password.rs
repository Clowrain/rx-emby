use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use uuid::Uuid;

pub(crate) const DEFAULT_ITERATIONS: u32 = 120_000;
const MIN_ITERATIONS: u32 = 100_000;
const MAX_ITERATIONS: u32 = 2_000_000;
type HmacSha256 = Hmac<Sha256>;

fn pbkdf2_sha256(password: &[u8], salt: &[u8], iterations: u32) -> Option<[u8; 32]> {
    let base_mac = HmacSha256::new_from_slice(password).ok()?;
    let mut mac = base_mac.clone();
    mac.update(salt);
    mac.update(&1u32.to_be_bytes());
    let mut u = mac.finalize().into_bytes();
    let mut output: [u8; 32] = u.into();
    for _ in 1..iterations {
        let mut mac = base_mac.clone();
        mac.update(&u);
        u = mac.finalize().into_bytes();
        for (dst, byte) in output.iter_mut().zip(u.iter()) {
            *dst ^= byte;
        }
    }
    Some(output)
}

pub(crate) fn hash_password_sync(password: &str) -> Result<String, String> {
    if password.is_empty() {
        return Err("密码不能为空".to_string());
    }
    let salt = *Uuid::new_v4().as_bytes();
    let hash = pbkdf2_sha256(password.as_bytes(), &salt, DEFAULT_ITERATIONS)
        .ok_or_else(|| "无法初始化 HMAC".to_string())?;
    Ok(format!(
        "$pbkdf2-sha256${}${}${}",
        DEFAULT_ITERATIONS,
        BASE64.encode(salt),
        BASE64.encode(hash)
    ))
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut diff = (left.len() ^ right.len()) as u64;
    for i in 0..left.len().max(right.len()) {
        diff |= u64::from(left.get(i).copied().unwrap_or(0) ^ right.get(i).copied().unwrap_or(0));
    }
    diff == 0
}

pub(crate) fn verify_password_sync(stored: &str, password: &str) -> bool {
    if stored.is_empty() {
        return false;
    }
    if !stored.starts_with("$pbkdf2-sha256$") {
        return constant_time_eq(stored.as_bytes(), password.as_bytes());
    }
    let parts: Vec<_> = stored.split('$').collect();
    if parts.len() != 5 || parts[1] != "pbkdf2-sha256" {
        return false;
    }
    let Ok(iterations) = parts[2].parse::<u32>() else {
        return false;
    };
    if !(MIN_ITERATIONS..=MAX_ITERATIONS).contains(&iterations) {
        return false;
    }
    let (Ok(salt), Ok(expected)) = (BASE64.decode(parts[3]), BASE64.decode(parts[4])) else {
        return false;
    };
    let Some(actual) = pbkdf2_sha256(password.as_bytes(), &salt, iterations) else {
        return false;
    };
    constant_time_eq(&actual, &expected)
}

pub(crate) async fn hash_password_blocking(pw: String) -> Result<String, (StatusCode, String)> {
    tokio::task::spawn_blocking(move || hash_password_sync(&pw))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
}

pub(crate) async fn verify_password_blocking(
    stored: String,
    pw: String,
) -> Result<bool, (StatusCode, String)> {
    tokio::task::spawn_blocking(move || verify_password_sync(&stored, &pw))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(password: &str, iterations: u32) -> String {
        let salt = [7u8; 16];
        let hash = pbkdf2_sha256(password.as_bytes(), &salt, iterations).unwrap();
        format!(
            "$pbkdf2-sha256${iterations}${}${}",
            BASE64.encode(salt),
            BASE64.encode(hash)
        )
    }

    #[test]
    fn rejects_empty_passwords_and_weak_pbkdf2_hashes() {
        assert!(!verify_password_sync("", ""));
        assert_eq!(hash_password_sync(""), Err("密码不能为空".to_string()));
        assert!(!verify_password_sync(&encoded("secret", 99_999), "secret"));
    }

    #[test]
    fn verifies_120000_and_600000_iteration_hashes_and_plaintext_upgrade() {
        assert!(verify_password_sync(&encoded("secret", 120_000), "secret"));
        assert!(verify_password_sync(&encoded("secret", 600_000), "secret"));
        assert!(verify_password_sync("legacy", "legacy"));
        let upgraded = hash_password_sync("legacy").unwrap();
        assert!(upgraded.starts_with("$pbkdf2-sha256$"));
        assert!(verify_password_sync(&upgraded, "legacy"));
    }
}
