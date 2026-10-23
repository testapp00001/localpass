//! Values handed to `WebAuthNPluginAddAuthenticator`.

use data_encoding::BASE64;

/// RFC 4122 byte order (the hex digits of the canonical string, left to right). This is the order CTAP and
/// authenticator data use, NOT the `GUID` struct field order and not `Guid.ToByteArray()`.
pub fn guid_str_to_rfc4122(guid: &str) -> Option<[u8; 16]> {
    let hex: String = guid
        .chars()
        .filter(|c| !matches!(c, '-' | '{' | '}'))
        .collect();
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// CTAP2 `authenticatorGetInfo` CBOR: `{1: ["FIDO_2_0","FIDO_2_1"], 3: aaguid, 4: {rk,up,uv: true},
/// 9: ["internal"], 10: [{alg: -7, type: "public-key"}]}`. 88 bytes. No extensions are advertised.
pub fn authenticator_info(aaguid: [u8; 16]) -> Vec<u8> {
    let mut v = Vec::with_capacity(88);
    v.extend_from_slice(&[0xA5, 0x01, 0x82, 0x68]);
    v.extend_from_slice(b"FIDO_2_0");
    v.push(0x68);
    v.extend_from_slice(b"FIDO_2_1");
    v.extend_from_slice(&[0x03, 0x50]);
    v.extend_from_slice(&aaguid);
    v.extend_from_slice(&[0x04, 0xA3, 0x62]);
    v.extend_from_slice(b"rk");
    v.extend_from_slice(&[0xF5, 0x62]);
    v.extend_from_slice(b"up");
    v.extend_from_slice(&[0xF5, 0x62]);
    v.extend_from_slice(b"uv");
    v.extend_from_slice(&[0xF5, 0x09, 0x81, 0x68]);
    v.extend_from_slice(b"internal");
    v.extend_from_slice(&[0x0A, 0x81, 0xA2, 0x63]);
    v.extend_from_slice(b"alg");
    v.extend_from_slice(&[0x26, 0x64]);
    v.extend_from_slice(b"type");
    v.push(0x6A);
    v.extend_from_slice(b"public-key");
    v
}

/// A small SVG 1.1 logo as the API wants it: base64 of the UTF-8 text.
pub fn logo_base64(dark: bool) -> String {
    let (bg, fg) = if dark {
        ("#3b82f6", "#ffffff")
    } else {
        ("#1d4ed8", "#ffffff")
    };
    let svg = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"32\" height=\"32\" viewBox=\"0 0 32 32\">\
         <rect width=\"32\" height=\"32\" rx=\"6\" fill=\"{bg}\"/>\
         <circle cx=\"16\" cy=\"13\" r=\"5\" fill=\"{fg}\"/><rect x=\"14\" y=\"16\" width=\"4\" height=\"9\" fill=\"{fg}\"/></svg>"
    );
    BASE64.encode(svg.as_bytes())
}

/// NUL-terminated UTF-16 (the one definition lives in [`crate::sys`], next to the FFI types).
pub use crate::sys::wide;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_is_88_bytes_with_aaguid_in_place() {
        let aaguid = guid_str_to_rfc4122("a315fc5e-a41e-4d69-9825-83d6c0419893").unwrap();
        let info = authenticator_info(aaguid);
        assert_eq!(info.len(), 88);
        assert_eq!(&info[..3], &[0xA5, 0x01, 0x82]);
        assert_eq!(&info[21..23], &[0x03, 0x50]);
        assert_eq!(&info[23..39], &aaguid);
        assert_eq!(aaguid[0], 0xA3);
        assert_eq!(aaguid[15], 0x93);
    }

    #[test]
    fn guid_parsing_rejects_garbage() {
        assert!(guid_str_to_rfc4122("nope").is_none());
        assert!(guid_str_to_rfc4122("{A1E209BF-2035-41D0-831F-B9A5D166DC03}").is_some());
    }

    #[test]
    fn logo_is_base64_of_svg() {
        let decoded = BASE64.decode(logo_base64(false).as_bytes()).unwrap();
        assert!(decoded.starts_with(b"<svg "));
    }
}
