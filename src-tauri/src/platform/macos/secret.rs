//! 密钥保护：钥匙串里存一把随机主密钥，密钥表里的内容用它做 AES-256-GCM。
//!
//! 和 Windows 的 DPAPI 一样，换用户 / 换机器解不开。钥匙串条目归创建它的程序所有，
//! 别的程序读它时系统会问用户。开发时每次重新编译，可执行文件的签名都会变，
//! 系统会把它当成另一个程序再问一次，点"始终允许"即可；正式签名的包不会反复问。

use aes_gcm::aead::{Aead, AeadCore, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Key, Nonce};
use parking_lot::Mutex;
use security_framework::passwords::{get_generic_password, set_generic_password};

use crate::error::{AppError, AppResult};

const SERVICE: &str = "com.datocor.app";
const ACCOUNT: &str = "secret-store-key";
/// 密文开头的版本标记
const MAGIC: &[u8] = b"DC1";
const NONCE_LEN: usize = 12;
/// errSecItemNotFound
const ITEM_NOT_FOUND: i32 = -25300;

/// 只缓存成功的结果：用户第一次拒绝了钥匙串访问，之后还能再试。
static MASTER_KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);

fn master_key() -> AppResult<[u8; 32]> {
    let mut cached = MASTER_KEY.lock();
    if let Some(key) = *cached {
        return Ok(key);
    }
    let key = match get_generic_password(SERVICE, ACCOUNT) {
        Ok(bytes) => <[u8; 32]>::try_from(bytes.as_slice())
            .map_err(|_| AppError::msg("钥匙串里的主密钥长度不对"))?,
        Err(err) if err.code() == ITEM_NOT_FOUND => {
            let key: [u8; 32] = Aes256Gcm::generate_key(OsRng).into();
            set_generic_password(SERVICE, ACCOUNT, &key)
                .map_err(|e| AppError::msg(format!("写入钥匙串失败：{e}")))?;
            key
        }
        Err(err) => return Err(AppError::msg(format!("读取钥匙串失败：{err}"))),
    };
    *cached = Some(key);
    Ok(key)
}

fn seal(key: &[u8; 32], plain: &[u8]) -> AppResult<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let sealed = cipher
        .encrypt(&nonce, plain)
        .map_err(|_| AppError::msg("加密失败"))?;
    let mut out = Vec::with_capacity(MAGIC.len() + NONCE_LEN + sealed.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&sealed);
    Ok(out)
}

fn open(key: &[u8; 32], sealed: &[u8]) -> AppResult<Vec<u8>> {
    let body = sealed
        .strip_prefix(MAGIC)
        .filter(|b| b.len() > NONCE_LEN)
        .ok_or_else(|| AppError::msg("密文格式不对"))?;
    let (nonce, data) = body.split_at(NONCE_LEN);
    Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
        .decrypt(Nonce::from_slice(nonce), data)
        .map_err(|_| AppError::msg("解密失败"))
}

pub fn protect(plain: &[u8]) -> AppResult<Vec<u8>> {
    seal(&master_key()?, plain)
}

pub fn unprotect(cipher: &[u8]) -> AppResult<Vec<u8>> {
    open(&master_key()?, cipher)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 不碰钥匙串（会弹系统对话框），只测加解密本身。
    #[test]
    fn roundtrip() {
        let key = [7u8; 32];
        let sealed = seal(&key, b"sk-test-123").unwrap();
        assert_ne!(sealed, b"sk-test-123");
        assert_eq!(open(&key, &sealed).unwrap(), b"sk-test-123");
        // 每次的随机数不同，同样的明文密文也不同
        assert_ne!(seal(&key, b"sk-test-123").unwrap(), sealed);
        assert!(open(&[8u8; 32], &sealed).is_err());
        assert!(open(&key, b"garbage").is_err());
    }
}
