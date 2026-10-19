//! Signing in: one password for the whole app, and sessions for the browsers signed in with it.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rand::RngCore;
use sha2::{Digest, Sha256};
use sqlx::SqliteExecutor;
use subtle::ConstantTimeEq;

use crate::settings::{Setting, SettingError};

pub const PASSWORD: Setting<String> = Setting::new("auth_password_hash");
pub const MIN_PASSWORD_LENGTH: usize = 8;
pub const SESSION_TTL: Duration = Duration::from_secs(30 * 24 * 3600);

// scrypt's cost: N = 2^14, r = 8, p = 1, a 32-byte key.
const SCRYPT_LOG_N: u8 = 14;
const SCRYPT_R: u32 = 8;
const SCRYPT_P: u32 = 1;
const SCRYPT_KEY_BYTES: usize = 32;

/// Whether a password is long enough. Length counts characters, not bytes.
pub fn check_strength(password: &str) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_LENGTH {
        return Err(format!(
            "Password must be at least {MIN_PASSWORD_LENGTH} characters"
        ));
    }
    Ok(())
}

/// `scrypt$N$r$p$<salt hex>$<key hex>`. Slow by design: call it
/// off the async runtime (`spawn_blocking`).
pub fn hash_password(password: &str) -> String {
    let mut salt = [0_u8; 16];
    rand::rng().fill_bytes(&mut salt);
    let key = scrypt_key(password, &salt, SCRYPT_LOG_N, SCRYPT_R, SCRYPT_P)
        .expect("the default scrypt parameters are valid");
    format!(
        "scrypt${}${SCRYPT_R}${SCRYPT_P}${}${}",
        1_u64 << SCRYPT_LOG_N,
        hex::encode(salt),
        hex::encode(key)
    )
}

/// Whether `password` matches a stored hash. A hash that can't be read never matches. Slow by
/// design: call it off the async runtime.
pub fn verify_password(password: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    let [_, n, r, p, salt, expected] = parts[..] else {
        return false;
    };
    let (Ok(n), Ok(r), Ok(p), Ok(salt), Ok(expected)) = (
        n.parse::<u64>(),
        r.parse::<u32>(),
        p.parse::<u32>(),
        hex::decode(salt),
        hex::decode(expected),
    ) else {
        return false;
    };
    if !n.is_power_of_two() || n < 2 {
        return false;
    }
    let log_n = n.trailing_zeros() as u8;
    match scrypt_key(password, &salt, log_n, r, p) {
        Ok(key) => key.ct_eq(&expected).into(),
        Err(()) => false,
    }
}

fn scrypt_key(password: &str, salt: &[u8], log_n: u8, r: u32, p: u32) -> Result<Vec<u8>, ()> {
    let params = scrypt::Params::new(log_n, r, p, SCRYPT_KEY_BYTES).map_err(|_| ())?;
    let mut key = vec![0_u8; SCRYPT_KEY_BYTES];
    scrypt::scrypt(password.as_bytes(), salt, &params, &mut key).map_err(|_| ())?;
    Ok(key)
}

/// Whether a password has been chosen yet. Until it has, the app shows its setup screen.
pub async fn password_is_set(db: impl SqliteExecutor<'_>) -> Result<bool, SettingError> {
    Ok(PASSWORD.get(db).await?.is_some())
}

/// Forgets the password and signs everyone out, so the next visit shows the setup screen.
pub async fn reset(db: &sqlx::SqlitePool) -> Result<(), SettingError> {
    let mut tx = db.begin().await?;
    PASSWORD.clear(&mut *tx).await?;
    sqlx::query("DELETE FROM auth_sessions")
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Unix seconds.
pub(crate) fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// How tokens are stored: their SHA-256, in hex.
pub(crate) fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// `bytes` random bytes, URL-safe base64 without padding.
pub(crate) fn random_token(bytes: usize) -> String {
    let mut random = vec![0_u8; bytes];
    rand::rng().fill_bytes(&mut random);
    URL_SAFE_NO_PAD.encode(random)
}

/// Starts a session, returning the token for the browser's cookie. Only its hash is stored.
/// Expired sessions are cleared out on the way.
pub async fn create_session(db: &sqlx::SqlitePool) -> Result<String, sqlx::Error> {
    let token = random_token(32);
    let now = now();
    let ttl = i64::try_from(SESSION_TTL.as_secs()).expect("a small duration");

    let mut tx = db.begin().await?;
    sqlx::query("DELETE FROM auth_sessions WHERE expires_at <= ?")
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("INSERT INTO auth_sessions (token_hash, expires_at) VALUES (?, ?)")
        .bind(token_hash(&token))
        .bind(now + ttl)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(token)
}

pub async fn is_valid_session(
    db: impl SqliteExecutor<'_>,
    token: Option<&str>,
) -> Result<bool, sqlx::Error> {
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        return Ok(false);
    };
    let expires_at: Option<i64> =
        sqlx::query_scalar("SELECT expires_at FROM auth_sessions WHERE token_hash = ?")
            .bind(token_hash(token))
            .fetch_optional(db)
            .await?;
    Ok(expires_at.is_some_and(|expires_at| expires_at > now()))
}

pub async fn end_session(db: impl SqliteExecutor<'_>, token: &str) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM auth_sessions WHERE token_hash = ?")
        .bind(token_hash(token))
        .execute(db)
        .await?;
    Ok(())
}

/// Signs out every other browser, keeping the session with this token.
pub async fn end_other_sessions(
    db: impl SqliteExecutor<'_>,
    keep_token: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM auth_sessions WHERE token_hash != ?")
        .bind(token_hash(keep_token))
        .execute(db)
        .await?;
    Ok(())
}

/// Allows a limited number of failed sign-ins per client within a sliding window. Kept in
/// memory: a restart clears it.
pub struct LoginThrottle {
    max_failures: usize,
    window: Duration,
    failures: Mutex<HashMap<String, VecDeque<Instant>>>,
}

impl Default for LoginThrottle {
    fn default() -> Self {
        Self::new(10, Duration::from_secs(15 * 60))
    }
}

impl LoginThrottle {
    pub fn new(max_failures: usize, window: Duration) -> Self {
        Self {
            max_failures,
            window,
            failures: Mutex::new(HashMap::new()),
        }
    }

    pub fn max_failures(&self) -> usize {
        self.max_failures
    }

    pub fn is_blocked(&self, client: &str) -> bool {
        self.is_blocked_at(client, Instant::now())
    }

    pub fn record_failure(&self, client: &str) {
        self.record_failure_at(client, Instant::now());
    }

    pub fn reset(&self, client: &str) {
        self.lock().remove(client);
    }

    fn is_blocked_at(&self, client: &str, now: Instant) -> bool {
        let mut failures = self.lock();
        let recent = Self::prune(&mut failures, client, now, self.window);
        recent >= self.max_failures
    }

    fn record_failure_at(&self, client: &str, now: Instant) {
        let mut failures = self.lock();
        Self::prune(&mut failures, client, now, self.window);
        failures
            .entry(client.to_owned())
            .or_default()
            .push_back(now);
    }

    /// Drops failures older than the window, returning how many are left.
    fn prune(
        failures: &mut HashMap<String, VecDeque<Instant>>,
        client: &str,
        now: Instant,
        window: Duration,
    ) -> usize {
        let Some(times) = failures.get_mut(client) else {
            return 0;
        };
        while times
            .front()
            .is_some_and(|t| now.duration_since(*t) >= window)
        {
            times.pop_front();
        }
        let left = times.len();
        if left == 0 {
            failures.remove(client);
        }
        left
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, VecDeque<Instant>>> {
        // A panic while holding the lock leaves the map usable, so carry on with it.
        self.failures.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    /// `hash_password("correct horse battery")`, with a fixed salt.
    const KNOWN_HASH: &str = "scrypt$16384$8$1$3329456a0c1b24efc287d5f5675260ee$\
                               317a6f744a86d689b603ffc1e9b959e86fe447e472e95723c456cf455026ee17";

    #[test]
    fn password_hash_round_trip() {
        let stored = hash_password("s3cret-password");
        assert_ne!(stored, "s3cret-password");
        assert!(stored.starts_with("scrypt$16384$8$1$"));
        assert!(verify_password("s3cret-password", &stored));
        assert!(!verify_password("wrong-password", &stored));
        assert!(!verify_password("anything", "not-a-valid-hash"));
    }

    #[test]
    fn accepts_a_known_hash() {
        assert!(verify_password("correct horse battery", KNOWN_HASH));
        assert!(!verify_password("correct horse batteries", KNOWN_HASH));
    }

    #[test]
    fn refuses_malformed_hashes() {
        for stored in [
            "",
            "scrypt$16384$8$1$zz$00",
            "scrypt$1000$8$1$00$00",
            "scrypt$16384$8$1$00",
            "scrypt$16384$0$1$00$00",
        ] {
            assert!(!verify_password("password", stored), "{stored}");
        }
    }

    #[test]
    fn strength_counts_characters() {
        assert!(check_strength("short").is_err());
        assert!(check_strength("12345678").is_ok());
        // Eight characters, more than eight bytes
        assert!(check_strength("ééééééé").is_err());
        assert!(check_strength("éééééééé").is_ok());
    }

    #[test]
    fn token_hashes_are_sha256_hex() {
        assert_eq!(
            token_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[tokio::test]
    async fn sessions_start_check_and_end() {
        let pool = db::open_in_memory().await.unwrap();
        let first = create_session(&pool).await.unwrap();
        let second = create_session(&pool).await.unwrap();
        assert_eq!(
            first.len(),
            43,
            "32 random bytes, base64url without padding"
        );
        assert_ne!(first, second);
        assert!(is_valid_session(&pool, Some(&first)).await.unwrap());
        assert!(!is_valid_session(&pool, Some("forged")).await.unwrap());
        assert!(!is_valid_session(&pool, Some("")).await.unwrap());
        assert!(!is_valid_session(&pool, None).await.unwrap());

        end_other_sessions(&pool, &second).await.unwrap();
        assert!(!is_valid_session(&pool, Some(&first)).await.unwrap());
        assert!(is_valid_session(&pool, Some(&second)).await.unwrap());

        end_session(&pool, &second).await.unwrap();
        assert!(!is_valid_session(&pool, Some(&second)).await.unwrap());
    }

    #[tokio::test]
    async fn expired_sessions_are_refused_and_cleared_out() {
        let pool = db::open_in_memory().await.unwrap();
        let token = create_session(&pool).await.unwrap();
        sqlx::query("UPDATE auth_sessions SET expires_at = ?")
            .bind(now() - 1)
            .execute(&pool)
            .await
            .unwrap();
        assert!(!is_valid_session(&pool, Some(&token)).await.unwrap());

        create_session(&pool).await.unwrap();
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM auth_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(left, 1);
    }

    #[tokio::test]
    async fn reset_forgets_the_password_and_signs_everyone_out() {
        let pool = db::open_in_memory().await.unwrap();
        PASSWORD
            .set(&pool, &hash_password("password1"))
            .await
            .unwrap();
        let token = create_session(&pool).await.unwrap();
        assert!(password_is_set(&pool).await.unwrap());

        reset(&pool).await.unwrap();
        assert!(!password_is_set(&pool).await.unwrap());
        assert!(!is_valid_session(&pool, Some(&token)).await.unwrap());
    }

    #[test]
    fn throttle_blocks_after_too_many_failures_until_the_window_passes() {
        let throttle = LoginThrottle::new(3, Duration::from_secs(60));
        let start = Instant::now();
        for i in 0..3 {
            assert!(!throttle.is_blocked_at("a", start));
            throttle.record_failure_at("a", start + Duration::from_secs(i));
        }
        assert!(throttle.is_blocked_at("a", start + Duration::from_secs(10)));
        assert!(
            !throttle.is_blocked_at("b", start),
            "other clients aren't affected"
        );
        // Once the first failure is a minute old, one more attempt is allowed.
        assert!(!throttle.is_blocked_at("a", start + Duration::from_secs(60)));
    }

    #[test]
    fn throttle_resets_after_a_success() {
        let throttle = LoginThrottle::new(1, Duration::from_secs(60));
        throttle.record_failure("a");
        assert!(throttle.is_blocked("a"));
        throttle.reset("a");
        assert!(!throttle.is_blocked("a"));
    }
}
