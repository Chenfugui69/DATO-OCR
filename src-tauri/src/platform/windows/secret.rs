//! DPAPI 密钥保护（规格 04 §6.3、07 §4.7）。
//!
//! 用当前用户账户作密钥，换用户/换机器解不开。额外的固定 entropy 防止同一用户下
//! 其他程序直接调 CryptUnprotectData 解开我们的数据。

use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
};

use crate::error::AppResult;

/// 改了这个值，已经加密保存的密钥就解不开了。品牌改名也不能动
const ENTROPY: &[u8] = b"CHENOCR.v1.secret";

fn blob(data: &[u8]) -> CRYPT_INTEGER_BLOB {
    CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    }
}

fn take(out: CRYPT_INTEGER_BLOB) -> Vec<u8> {
    // SAFETY: pbData/cbData 由 DPAPI 分配，用 LocalFree 释放。
    unsafe {
        let bytes = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(out.pbData.cast())));
        bytes
    }
}

pub fn protect(plain: &[u8]) -> AppResult<Vec<u8>> {
    let input = blob(plain);
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: 输入 blob 指向调用期间有效的切片；输出由 DPAPI 分配。
    unsafe {
        CryptProtectData(
            &input,
            windows::core::PCWSTR::null(),
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )?;
    }
    Ok(take(out))
}

pub fn unprotect(cipher: &[u8]) -> AppResult<Vec<u8>> {
    let input = blob(cipher);
    let entropy = blob(ENTROPY);
    let mut out = CRYPT_INTEGER_BLOB::default();
    // SAFETY: 同上。
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            Some(&entropy),
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out,
        )?;
    }
    Ok(take(out))
}

#[cfg(test)]
mod tests {
    #[test]
    fn roundtrip() {
        let cipher = super::protect(b"sk-test-123").unwrap();
        assert_ne!(cipher, b"sk-test-123");
        assert_eq!(super::unprotect(&cipher).unwrap(), b"sk-test-123");
    }
}
