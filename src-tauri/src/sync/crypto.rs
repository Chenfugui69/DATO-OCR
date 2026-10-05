//! 同步用到的密码学。全部用 RustCrypto / dalek 的现成实现，不自己造（规格 09 §3.2）。
//!
//! - 内容加密：XChaCha20-Poly1305，每条随机 24 字节 nonce，放在密文前面
//! - 局域网配对：双方各出一个一次性 X25519 密钥，HKDF 派生出会话密钥和 4 位核对数字
//!   （两边屏幕上的数字一致 = 中间没有人调包）
//! - WebDAV：同步密码经 Argon2id 派生出内容密钥，盐和参数放在网盘上（盐不是秘密）

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey};

use crate::error::{AppError, AppResult};

pub type SecretKey = [u8; 32];
const NONCE_LEN: usize = 24;

pub fn random_key() -> SecretKey {
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    key
}

pub fn random_bytes(len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    OsRng.fill_bytes(&mut out);
    out
}

/// 加密：输出 `nonce(24) ‖ 密文 ‖ tag(16)`。`aad` 不加密但参与校验（用来绑定用途，防止挪用）。
pub fn seal(key: &SecretKey, plain: &[u8], aad: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce);
    let body = cipher
        .encrypt(XNonce::from_slice(&nonce), Payload { msg: plain, aad })
        // 只在明文超过 256 GB 时失败；真失败了输出空的，对方解密会报错
        .unwrap_or_default();
    let mut out = Vec::with_capacity(NONCE_LEN + body.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&body);
    out
}

pub fn open(key: &SecretKey, sealed: &[u8], aad: &[u8]) -> AppResult<Vec<u8>> {
    if sealed.len() < NONCE_LEN + 16 {
        return Err(AppError::msg("密文太短"));
    }
    let (nonce, body) = sealed.split_at(NONCE_LEN);
    XChaCha20Poly1305::new(key.into())
        .decrypt(XNonce::from_slice(nonce), Payload { msg: body, aad })
        .map_err(|_| AppError::msg("解密失败：密钥不对或内容被改过"))
}

/// 配对的一方。`transcript` 两边必须一样：版本 ‖ 加入方公钥 ‖ 主机公钥 ‖ 设备码。
pub struct Handshake {
    secret: EphemeralSecret,
    public: PublicKey,
}

pub struct PairSession {
    pub key: SecretKey,
    /// 4 位核对数字
    pub sas: String,
}

impl Handshake {
    pub fn new() -> Self {
        let secret = EphemeralSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret);
        Self { secret, public }
    }

    pub fn public(&self) -> [u8; 32] {
        self.public.to_bytes()
    }

    pub fn finish(self, peer: &[u8], transcript: &[u8]) -> AppResult<PairSession> {
        let peer: [u8; 32] = peer
            .try_into()
            .map_err(|_| AppError::msg("对方公钥长度不对"))?;
        let shared = self.secret.diffie_hellman(&PublicKey::from(peer));
        // 低阶点（全零共享密钥）：对方在捣乱
        if !shared.was_contributory() {
            return Err(AppError::msg("对方公钥无效"));
        }
        let hk = Hkdf::<Sha256>::new(Some(transcript), shared.as_bytes());
        let mut key = [0u8; 32];
        hk.expand(b"dato-cor/pair/key", &mut key)
            .map_err(|_| AppError::msg("密钥派生失败"))?;
        let mut sas = [0u8; 4];
        hk.expand(b"dato-cor/pair/sas", &mut sas)
            .map_err(|_| AppError::msg("密钥派生失败"))?;
        Ok(PairSession {
            key,
            sas: format!("{:04}", u32::from_be_bytes(sas) % 10_000),
        })
    }
}

pub fn pair_transcript(member_pub: &[u8], host_pub: &[u8], code: &str) -> Vec<u8> {
    let mut t = b"dato-cor/pair/v1".to_vec();
    t.extend_from_slice(member_pub);
    t.extend_from_slice(host_pub);
    t.extend_from_slice(code.as_bytes());
    t
}

/// Argon2id 参数，跟着盐一起存在网盘上，以后调参数旧数据还能解。
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct KdfParams {
    /// 内存，KiB
    pub m: u32,
    pub t: u32,
    pub p: u32,
}

impl Default for KdfParams {
    /// 64 MB、3 轮、4 线程（规格 09 §3.2），一般电脑上半秒左右
    fn default() -> Self {
        Self {
            m: 64 * 1024,
            t: 3,
            p: 4,
        }
    }
}

pub fn password_key(password: &str, salt: &[u8], params: KdfParams) -> AppResult<SecretKey> {
    let params = Params::new(params.m, params.t, params.p, Some(32))
        .map_err(|e| AppError::msg(format!("Argon2 参数无效：{e}")))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| AppError::msg(format!("同步密码派生失败：{e}")))?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seal_roundtrip_and_tamper() {
        let key = random_key();
        let sealed = seal(&key, b"hello", b"record");
        assert_eq!(open(&key, &sealed, b"record").unwrap(), b"hello");
        // 换了用途、换了密钥、改了一个字节都解不开
        assert!(open(&key, &sealed, b"other").is_err());
        assert!(open(&random_key(), &sealed, b"record").is_err());
        let mut bad = sealed.clone();
        *bad.last_mut().unwrap() ^= 1;
        assert!(open(&key, &bad, b"record").is_err());
    }

    #[test]
    fn both_sides_agree_on_key_and_sas() {
        let member = Handshake::new();
        let host = Handshake::new();
        let (mp, hp) = (member.public(), host.public());
        let t = pair_transcript(&mp, &hp, "12345678");
        let a = member.finish(&hp, &t).unwrap();
        let b = host.finish(&mp, &t).unwrap();
        assert_eq!(a.key, b.key);
        assert_eq!(a.sas, b.sas);
        assert_eq!(a.sas.len(), 4);
    }

    #[test]
    fn different_code_gives_different_key() {
        let member = Handshake::new();
        let host = Handshake::new();
        let (mp, hp) = (member.public(), host.public());
        let a = member
            .finish(&hp, &pair_transcript(&mp, &hp, "11111111"))
            .unwrap();
        let b = host
            .finish(&mp, &pair_transcript(&mp, &hp, "22222222"))
            .unwrap();
        assert_ne!(a.key, b.key);
    }

    #[test]
    fn low_order_point_is_rejected() {
        assert!(Handshake::new().finish(&[0u8; 32], b"t").is_err());
    }

    #[test]
    fn password_key_is_deterministic() {
        let fast = KdfParams { m: 256, t: 1, p: 1 };
        let a = password_key("同步密码", b"saltsaltsalt", fast).unwrap();
        let b = password_key("同步密码", b"saltsaltsalt", fast).unwrap();
        let c = password_key("同步密码!", b"saltsaltsalt", fast).unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
