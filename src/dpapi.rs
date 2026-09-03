//! Windows DPAPI 加密存储：密文仅能被当前 Windows 用户解开。
use base64::{engine::general_purpose::STANDARD, Engine as _};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
};

/// 加密字符串，返回 base64 密文
pub fn protect(plain: &str) -> Result<String, String> {
    if plain.is_empty() {
        return Ok(String::new());
    }
    unsafe {
        let bytes = plain.as_bytes();
        let input = CRYPT_INTEGER_BLOB {
            cbData: bytes.len() as u32,
            pbData: bytes.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        CryptProtectData(&input, PCWSTR::null(), None, None, None, 0, &mut output)
            .map_err(|e| format!("DPAPI 加密失败: {e}"))?;
        let cipher = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut _));
        Ok(STANDARD.encode(cipher))
    }
}

/// 解密 base64 密文，返回原字符串
pub fn unprotect(b64: &str) -> Result<String, String> {
    if b64.is_empty() {
        return Ok(String::new());
    }
    let raw = STANDARD
        .decode(b64.trim())
        .map_err(|e| format!("密文 base64 解码失败: {e}"))?;
    unsafe {
        let input = CRYPT_INTEGER_BLOB {
            cbData: raw.len() as u32,
            pbData: raw.as_ptr() as *mut u8,
        };
        let mut output = CRYPT_INTEGER_BLOB::default();
        CryptUnprotectData(&input, None, None, None, None, 0, &mut output)
            .map_err(|e| format!("DPAPI 解密失败: {e}"))?;
        let plain = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(HLOCAL(output.pbData as *mut _));
        String::from_utf8(plain).map_err(|e| format!("明文 UTF-8 解码失败: {e}"))
    }
}
