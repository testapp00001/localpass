//! Reading a two-factor setup QR code from a picture (desktop only; a phone scans with its camera).
//!
//! Three sources feed one decoder, so the QR's secret never has to touch the disk:
//! - the system clipboard (a copied screenshot: the main path, see `clipboard::read_image`),
//! - an image file (the open dialog or a dropped file),
//! - the screen (Windows: GDI capture of all monitors; Linux: the screenshot portal, whose file is
//!   deleted right after it is read).
//!
//! Only `otpauth://totp/...` codes that `TotpSpec` accepts are returned. Pixel buffers are wiped
//! after use. The decoded link goes to the form as the user's own input, just as a pasted key does.

use std::fmt;
use std::io::Read;
use std::path::Path;

use lp_core::totp::TotpSpec;
use serde::Serialize;
use zeroize::Zeroizing;

/// Largest picture file we read.
const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
/// Largest picture we decode (pixels), whatever its source: 50 MP.
const MAX_PIXELS: usize = 50_000_000;

/// A two-factor setup code found in a picture.
#[derive(Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    /// The `otpauth://totp/...` link, exactly as the QR code carried it.
    pub uri: String,
    pub issuer: Option<String>,
    pub account: Option<String>,
}

impl fmt::Debug for Found {
    // The link holds the secret, so it never reaches a log through `{:?}`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Found")
            .field("issuer", &self.issuer)
            .field("account", &self.account)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum QrError {
    /// No readable QR code in the picture.
    NoCode,
    /// A QR code, but not a two-factor link.
    NotTwoFactor,
    /// Google Authenticator's "transfer accounts" code.
    Migration,
    /// An `otpauth://` link LocalPass cannot use (counter-based, damaged secret, ...).
    Invalid(String),
    /// Over the file or pixel limit.
    TooBig,
    /// A file type other than PNG, JPEG, BMP or WebP.
    Unsupported,
    /// The file or picture could not be read.
    Unreadable(String),
}

impl QrError {
    /// When a picture holds several codes, the most specific complaint wins.
    fn rank(&self) -> u8 {
        match self {
            QrError::NoCode => 0,
            QrError::NotTwoFactor => 1,
            QrError::Migration => 2,
            _ => 3,
        }
    }
}

impl fmt::Display for QrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            QrError::NoCode => f.write_str(
                "No QR code found in the picture. Make sure the whole code is in view and big enough to read.",
            ),
            QrError::NotTwoFactor => f.write_str("That QR code is not a two-factor setup code."),
            QrError::Migration => f.write_str(
                "That is an authenticator-app transfer code, which LocalPass cannot read. Use the setup QR code or key that the website shows when you turn on two-factor authentication.",
            ),
            QrError::Invalid(why) => write!(f, "LocalPass cannot use that two-factor code ({why})."),
            QrError::TooBig => f.write_str("That picture is too large to read."),
            QrError::Unsupported => {
                f.write_str("Pick a PNG, JPEG, BMP or WebP picture.")
            }
            QrError::Unreadable(why) => write!(f, "Could not read the picture ({why})."),
        }
    }
}

impl std::error::Error for QrError {}

fn check_size(width: usize, height: usize) -> Result<usize, QrError> {
    match width.checked_mul(height) {
        Some(n) if n > 0 && n <= MAX_PIXELS => Ok(n),
        Some(0) => Err(QrError::Unreadable("it is empty".into())),
        _ => Err(QrError::TooBig),
    }
}

/// Brightness of one pixel; a see-through pixel counts as white paper.
fn luma(r: u8, g: u8, b: u8, a: u8) -> u8 {
    let y = (299 * u32::from(r) + 587 * u32::from(g) + 114 * u32::from(b)) / 1000;
    if a == 255 {
        y as u8
    } else {
        let a = u32::from(a);
        ((y * a + 255 * (255 - a)) / 255) as u8
    }
}

fn starts_with_ci(text: &str, prefix: &str) -> bool {
    text.len() >= prefix.len()
        && text.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
}

fn classify(text: &str) -> Result<Found, QrError> {
    let text = text.trim();
    if starts_with_ci(text, "otpauth-migration://") {
        return Err(QrError::Migration);
    }
    if !starts_with_ci(text, "otpauth://") {
        return Err(QrError::NotTwoFactor);
    }
    let spec = TotpSpec::parse(text).map_err(|e| {
        let why = e.to_string();
        QrError::Invalid(
            why.strip_prefix("invalid TOTP: ")
                .unwrap_or(&why)
                .to_string(),
        )
    })?;
    Ok(Found {
        uri: text.to_string(),
        issuer: spec.issuer.clone().filter(|s| !s.is_empty()),
        account: spec.account.clone().filter(|s| !s.is_empty()),
    })
}

/// Finds the first usable two-factor code in an 8-bit grayscale picture (`width * height` bytes,
/// row by row).
pub fn from_gray(width: usize, height: usize, gray: &[u8]) -> Result<Found, QrError> {
    let pixels = check_size(width, height)?;
    if gray.len() != pixels {
        return Err(QrError::Unreadable(
            "its size does not match its data".into(),
        ));
    }
    let mut image =
        rqrr::PreparedImage::prepare_from_greyscale(width, height, |x, y| gray[y * width + x]);
    let mut worst = QrError::NoCode;
    for grid in image.detect_grids() {
        let Ok((_, text)) = grid.decode() else {
            continue;
        };
        let text = Zeroizing::new(text);
        match classify(&text) {
            Ok(found) => return Ok(found),
            Err(e) if e.rank() > worst.rank() => worst = e,
            Err(_) => {}
        }
    }
    Err(worst)
}

/// Same for an RGBA picture (the clipboard's and the decoders' layout).
pub fn from_rgba(width: usize, height: usize, rgba: &[u8]) -> Result<Found, QrError> {
    let pixels = check_size(width, height)?;
    if rgba.len() != pixels.saturating_mul(4) {
        return Err(QrError::Unreadable(
            "its size does not match its data".into(),
        ));
    }
    let gray: Zeroizing<Vec<u8>> = Zeroizing::new(
        rgba.as_chunks::<4>()
            .0
            .iter()
            .map(|p| luma(p[0], p[1], p[2], p[3]))
            .collect(),
    );
    from_gray(width, height, &gray)
}

/// Reads a PNG, JPEG, BMP or WebP file of at most 20 MB and 50 MP.
pub fn from_file(path: &Path) -> Result<Found, QrError> {
    use image::{ImageDecoder, ImageFormat};

    let file = std::fs::File::open(path).map_err(|e| QrError::Unreadable(e.to_string()))?;
    let meta = file
        .metadata()
        .map_err(|e| QrError::Unreadable(e.to_string()))?;
    if !meta.is_file() {
        return Err(QrError::Unreadable("it is not a file".into()));
    }
    if meta.len() > MAX_FILE_BYTES {
        return Err(QrError::TooBig);
    }
    // `take` keeps a file that grows while we read it inside the limit too.
    let mut bytes = Zeroizing::new(Vec::with_capacity(meta.len() as usize));
    file.take(MAX_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| QrError::Unreadable(e.to_string()))?;
    if bytes.len() as u64 > MAX_FILE_BYTES {
        return Err(QrError::TooBig);
    }

    let format = image::guess_format(&bytes).map_err(|_| QrError::Unsupported)?;
    if !matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Bmp | ImageFormat::WebP
    ) {
        return Err(QrError::Unsupported);
    }
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&bytes[..]), format);
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(MAX_PIXELS as u64 * 4 + (64 << 20));
    reader.limits(limits);
    let decoder = reader
        .into_decoder()
        .map_err(|e| QrError::Unreadable(e.to_string()))?;
    let (width, height) = decoder.dimensions();
    check_size(width as usize, height as usize)?;
    let picture = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| QrError::Unreadable(e.to_string()))?;
    if picture.color().has_alpha() {
        let rgba = Zeroizing::new(picture.into_rgba8().into_raw());
        from_rgba(width as usize, height as usize, &rgba)
    } else {
        let gray = Zeroizing::new(picture.into_luma8().into_raw());
        from_gray(width as usize, height as usize, &gray)
    }
}

// ---- Scan QR on screen ----

/// Windows: a GDI copy of every monitor (the "virtual screen"). Nothing is written to disk.
#[cfg(windows)]
pub fn from_screen() -> Result<Option<Found>, QrError> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CAPTUREBLT, CreateCompatibleBitmap,
        CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC,
        SRCCOPY, SelectObject,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
        SM_YVIRTUALSCREEN,
    };

    let fail =
        |what: &str| QrError::Unreadable(format!("Windows could not capture the screen: {what}"));
    // SAFETY: plain GDI calls on handles created here and released below; the pixel buffer is
    // `width * height * 4` bytes, which is what GetDIBits is told it may write.
    unsafe {
        let (left, top) = (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
        );
        let (width, height) = (
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        );
        if width <= 0 || height <= 0 {
            return Err(fail("no screen"));
        }
        let pixels = check_size(width as usize, height as usize)?;

        let screen = GetDC(None);
        if screen.is_invalid() {
            return Err(fail("no device context"));
        }
        let memory = CreateCompatibleDC(Some(screen));
        let bitmap = CreateCompatibleBitmap(screen, width, height);
        let mut buffer: Zeroizing<Vec<u8>> = Zeroizing::new(vec![0; pixels * 4]);
        let mut copied = false;
        if !memory.is_invalid() && !bitmap.is_invalid() {
            let old = SelectObject(memory, bitmap.into());
            if BitBlt(
                memory,
                0,
                0,
                width,
                height,
                Some(screen),
                left,
                top,
                SRCCOPY | CAPTUREBLT,
            )
            .is_ok()
            {
                let mut info = BITMAPINFO {
                    bmiHeader: BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: width,
                        biHeight: -height, // top-down rows
                        biPlanes: 1,
                        biBitCount: 32,
                        biCompression: BI_RGB.0,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let rows = GetDIBits(
                    memory,
                    bitmap,
                    0,
                    height as u32,
                    Some(buffer.as_mut_ptr().cast()),
                    &mut info,
                    DIB_RGB_COLORS,
                );
                copied = rows == height;
            }
            SelectObject(memory, old);
        }
        if !bitmap.is_invalid() {
            let _ = DeleteObject(bitmap.into());
        }
        if !memory.is_invalid() {
            let _ = DeleteDC(memory);
        }
        ReleaseDC(Some(HWND::default()), screen);
        if !copied {
            return Err(fail("the copy failed"));
        }
        // GDI hands out B, G, R, unused: the unused byte is not an alpha value.
        let gray: Zeroizing<Vec<u8>> = Zeroizing::new(
            buffer
                .as_chunks::<4>()
                .0
                .iter()
                .map(|p| luma(p[2], p[1], p[0], 255))
                .collect(),
        );
        from_gray(width as usize, height as usize, &gray).map(Some)
    }
}

/// Linux: the desktop's own screenshot picker (xdg-desktop-portal). `Ok(None)` when the user
/// closes the picker. The portal saves the picture as a file; it is read and deleted at once.
#[cfg(target_os = "linux")]
pub async fn from_screen() -> Result<Option<Found>, QrError> {
    use ashpd::desktop::ResponseError;
    use ashpd::desktop::screenshot::Screenshot;

    let unavailable = |e: ashpd::Error| {
        QrError::Unreadable(format!("the desktop's screenshot service said: {e}"))
    };
    let response = match Screenshot::request()
        .interactive(true)
        .modal(true)
        .send()
        .await
    {
        Ok(request) => request.response(),
        Err(e) => return Err(unavailable(e)),
    };
    let shot = match response {
        Ok(shot) => shot,
        Err(ashpd::Error::Response(ResponseError::Cancelled)) => return Ok(None),
        Err(e) => return Err(unavailable(e)),
    };
    let path = file_uri_path(shot.uri().as_str())
        .ok_or_else(|| QrError::Unreadable("the screenshot is not a local file".into()))?;
    let read = tauri::async_runtime::spawn_blocking({
        let path = path.clone();
        move || from_file(&path)
    })
    .await
    .map_err(|e| QrError::Unreadable(e.to_string()))?;
    // The picture may show the secret: never leave it in ~/Pictures.
    if let Err(e) = std::fs::remove_file(&path) {
        tracing::warn!("could not delete the screenshot {}: {e}", path.display());
    }
    read.map(Some)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn from_screen() -> Result<Option<Found>, QrError> {
    Err(QrError::Unreadable(
        "scanning the screen is not available on this system".into(),
    ))
}

/// `file:///home/me/Pictures/a%20b.png` -> `/home/me/Pictures/a b.png`.
#[cfg(any(target_os = "linux", test))]
fn file_uri_path(uri: &str) -> Option<std::path::PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = std::str::from_utf8(bytes.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(out)))
    }
    #[cfg(not(unix))]
    {
        String::from_utf8(out).ok().map(std::path::PathBuf::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    use qrcode::{Color, QrCode};

    const URI: &str =
        "otpauth://totp/Example:alice@example.com?secret=JBSWY3DPEHPK3PXP&issuer=Example";

    /// A black-on-white QR code as RGBA, `scale` pixels per module, 4 modules of white around it.
    fn qr_rgba(data: &str, scale: usize) -> (usize, usize, Vec<u8>) {
        let code = QrCode::new(data.as_bytes()).unwrap();
        let modules = code.width();
        let side = (modules + 8) * scale;
        let mut rgba = vec![255u8; side * side * 4];
        for (i, color) in code.to_colors().into_iter().enumerate() {
            if color != Color::Dark {
                continue;
            }
            let (mx, my) = (i % modules + 4, i / modules + 4);
            for y in my * scale..(my + 1) * scale {
                for x in mx * scale..(mx + 1) * scale {
                    let at = (y * side + x) * 4;
                    rgba[at..at + 3].fill(0);
                }
            }
        }
        (side, side, rgba)
    }

    /// Pastes `part` into a grey `width` x `height` canvas at (`left`, `top`).
    fn on_canvas(
        width: usize,
        height: usize,
        parts: &[(usize, usize, usize, usize, &[u8])],
    ) -> Vec<u8> {
        let mut canvas = [235u8, 235, 240, 255].repeat(width * height);
        for &(left, top, w, h, rgba) in parts {
            for y in 0..h {
                let to = ((top + y) * width + left) * 4;
                canvas[to..to + w * 4].copy_from_slice(&rgba[y * w * 4..(y + 1) * w * 4]);
            }
        }
        canvas
    }

    fn write_png(
        dir: &Path,
        name: &str,
        width: usize,
        height: usize,
        rgba: &[u8],
    ) -> std::path::PathBuf {
        let path = dir.join(name);
        image::save_buffer(
            &path,
            rgba,
            width as u32,
            height as u32,
            image::ExtendedColorType::Rgba8,
        )
        .unwrap();
        path
    }

    #[test]
    fn reads_a_code_from_rgba_pixels() {
        let (w, h, rgba) = qr_rgba(URI, 4);
        let found = from_rgba(w, h, &rgba).unwrap();
        assert_eq!(found.uri, URI);
        assert_eq!(found.issuer.as_deref(), Some("Example"));
        assert_eq!(found.account.as_deref(), Some("alice@example.com"));
    }

    #[test]
    fn debug_output_hides_the_secret() {
        let (w, h, rgba) = qr_rgba(URI, 4);
        let shown = format!("{:?}", from_rgba(w, h, &rgba).unwrap());
        assert!(!shown.contains("JBSWY3DPEHPK3PXP"), "{shown}");
        assert!(shown.contains("Example"));
    }

    #[test]
    fn reads_a_small_code_in_a_screen_sized_picture() {
        // About what a QR looks like in a full-screen screenshot: 2 pixels per module.
        let (w, h, rgba) = qr_rgba(URI, 2);
        let canvas = on_canvas(1920, 1080, &[(1300, 400, w, h, &rgba)]);
        assert_eq!(from_rgba(1920, 1080, &canvas).unwrap().uri, URI);
    }

    #[test]
    fn a_see_through_background_counts_as_white() {
        let (w, h, mut rgba) = qr_rgba(URI, 4);
        for px in rgba.as_chunks_mut::<4>().0.iter_mut() {
            if px[0] == 255 {
                px.fill(0); // the paper becomes transparent black, as many PNGs store it
            }
        }
        assert_eq!(from_rgba(w, h, &rgba).unwrap().uri, URI);
    }

    #[test]
    fn skips_other_codes_and_picks_the_two_factor_one() {
        let (w1, h1, other) = qr_rgba("https://example.com/not-a-secret", 3);
        let (w2, h2, totp) = qr_rgba(URI, 3);
        let canvas = on_canvas(
            900,
            500,
            &[(20, 20, w1, h1, &other), (450, 150, w2, h2, &totp)],
        );
        assert_eq!(from_rgba(900, 500, &canvas).unwrap().uri, URI);
    }

    #[test]
    fn explains_why_a_code_is_not_used() {
        let (w, h, rgba) = qr_rgba("https://example.com/", 4);
        assert_eq!(from_rgba(w, h, &rgba).unwrap_err(), QrError::NotTwoFactor);

        let (w, h, rgba) = qr_rgba(
            "otpauth-migration://offline?data=CjEKCkhlbGxvId6tvu8SGEV4YW1wbGU6YWxpY2VAZ21haWwuY29tGgdFeGFtcGxlIAEoATACEAEYASAA",
            3,
        );
        assert_eq!(from_rgba(w, h, &rgba).unwrap_err(), QrError::Migration);

        let (w, h, rgba) = qr_rgba(
            "otpauth://hotp/Example:bob?secret=JBSWY3DPEHPK3PXP&counter=1",
            4,
        );
        let err = from_rgba(w, h, &rgba).unwrap_err();
        assert!(
            matches!(&err, QrError::Invalid(why) if why.contains("HOTP")),
            "{err}"
        );

        let (w, h, rgba) = qr_rgba("otpauth://totp/Example:bob?secret=%%%", 4);
        assert!(matches!(
            from_rgba(w, h, &rgba).unwrap_err(),
            QrError::Invalid(_)
        ));
    }

    #[test]
    fn a_picture_without_a_code_says_so() {
        // Deterministic noise and a plain white page.
        let mut state = 0x2545_F491u32;
        let noise: Vec<u8> = (0..300 * 300 * 4)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as u8
            })
            .collect();
        assert_eq!(from_rgba(300, 300, &noise).unwrap_err(), QrError::NoCode);
        assert_eq!(
            from_rgba(64, 64, &[255; 64 * 64 * 4]).unwrap_err(),
            QrError::NoCode
        );
    }

    #[test]
    fn rejects_bad_sizes_before_allocating() {
        assert_eq!(from_rgba(10_000, 10_000, &[]).unwrap_err(), QrError::TooBig);
        assert_eq!(from_rgba(usize::MAX, 2, &[]).unwrap_err(), QrError::TooBig);
        assert!(matches!(
            from_rgba(0, 5, &[]).unwrap_err(),
            QrError::Unreadable(_)
        ));
        assert!(matches!(
            from_rgba(4, 4, &[0; 10]).unwrap_err(),
            QrError::Unreadable(_)
        ));
        assert!(matches!(
            from_gray(4, 4, &[0; 10]).unwrap_err(),
            QrError::Unreadable(_)
        ));
    }

    #[test]
    fn reads_png_jpeg_bmp_and_webp_files() {
        let dir = tempfile::tempdir().unwrap();
        let (w, h, rgba) = qr_rgba(URI, 5);
        let png = write_png(dir.path(), "code.png", w, h, &rgba);
        assert_eq!(from_file(&png).unwrap().uri, URI);

        let picture = image::RgbaImage::from_raw(w as u32, h as u32, rgba.clone()).unwrap();
        let rgb = image::DynamicImage::ImageRgba8(picture).to_rgb8();
        for name in ["code.jpg", "code.bmp", "code.webp"] {
            let path = dir.path().join(name);
            rgb.save(&path).unwrap();
            assert_eq!(from_file(&path).unwrap().uri, URI, "{name}");
        }
        // A file's extension does not decide: the content does.
        let renamed = dir.path().join("looks-like-text.txt");
        std::fs::copy(&png, &renamed).unwrap();
        assert_eq!(from_file(&renamed).unwrap().uri, URI);
    }

    #[test]
    fn a_png_with_a_see_through_background_reads_too() {
        let dir = tempfile::tempdir().unwrap();
        let (w, h, mut rgba) = qr_rgba(URI, 5);
        for px in rgba.as_chunks_mut::<4>().0.iter_mut() {
            if px[0] == 255 {
                px.fill(0);
            }
        }
        let png = write_png(dir.path(), "clear.png", w, h, &rgba);
        assert_eq!(from_file(&png).unwrap().uri, URI);
    }

    #[test]
    fn refuses_files_that_are_not_pictures_or_too_big() {
        let dir = tempfile::tempdir().unwrap();

        let text = dir.path().join("note.png");
        std::fs::write(&text, b"just some text, not a picture").unwrap();
        assert_eq!(from_file(&text).unwrap_err(), QrError::Unsupported);

        // GIF is a real picture type, but not one we read.
        let gif = dir.path().join("a.gif");
        std::fs::write(&gif, b"GIF89a\x01\x00\x01\x00\x00\x00\x00;").unwrap();
        assert_eq!(from_file(&gif).unwrap_err(), QrError::Unsupported);

        let huge = dir.path().join("huge.png");
        let file = std::fs::File::create(&huge).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        assert_eq!(from_file(&huge).unwrap_err(), QrError::TooBig);

        assert!(matches!(
            from_file(dir.path()).unwrap_err(),
            QrError::Unreadable(_)
        ));
        assert!(matches!(
            from_file(&dir.path().join("missing.png")).unwrap_err(),
            QrError::Unreadable(_)
        ));
    }

    #[test]
    fn a_picture_with_too_many_pixels_is_refused_from_its_header() {
        // 8000 x 8000 = 64 MP of white: a few KB as a PNG, but over the pixel limit.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.png");
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                &vec![255u8; 8000 * 8000],
                8000,
                8000,
                image::ExtendedColorType::L8,
            )
            .unwrap();
        std::fs::write(&path, &png).unwrap();
        assert_eq!(from_file(&path).unwrap_err(), QrError::TooBig);
    }

    /// Assisted: `cargo test -p localpass --lib -- --ignored picture_another_program`. A different
    /// program (wl-copy, like GNOME's screenshot tool) puts a PNG on the clipboard, and the paste
    /// path reads it through arboard (XWayland on GNOME), decodes it and empties the clipboard.
    #[test]
    #[ignore = "needs a Wayland session with wl-copy, and replaces the clipboard"]
    fn reads_a_picture_another_program_copied() {
        let dir = tempfile::tempdir().unwrap();
        let (w, h, rgba) = qr_rgba(URI, 4);
        let png = write_png(dir.path(), "copied.png", w, h, &rgba);
        let status = std::process::Command::new("wl-copy")
            .args(["--type", "image/png"])
            .stdin(std::fs::File::open(&png).unwrap())
            .status()
            .unwrap();
        assert!(status.success());
        std::thread::sleep(std::time::Duration::from_millis(300));

        let image = crate::clipboard::read_image().unwrap();
        assert_eq!((image.width, image.height), (w, h));
        assert_eq!(
            from_rgba(image.width, image.height, &image.rgba)
                .unwrap()
                .uri,
            URI
        );
        assert!(
            crate::clipboard::clear_if_image(&image),
            "the picture should have been cleared"
        );
        assert!(
            crate::clipboard::read_image().is_err(),
            "the clipboard should hold no picture now"
        );
        // What other programs see (Wayland), not just our own X11 view of it.
        let types = std::process::Command::new("wl-paste")
            .arg("--list-types")
            .output()
            .unwrap();
        let types = String::from_utf8_lossy(&types.stdout).into_owned();
        assert!(
            !types.contains("image/"),
            "a Wayland program still sees a picture: {types}"
        );
    }

    #[test]
    fn turns_a_portal_file_uri_into_a_path() {
        let p = |s| file_uri_path(s).map(|p| p.to_string_lossy().into_owned());
        assert_eq!(
            p("file:///home/me/Pictures/Screenshot%20from%202026.png").as_deref(),
            Some("/home/me/Pictures/Screenshot from 2026.png")
        );
        assert_eq!(
            p("file://localhost/tmp/a.png").as_deref(),
            Some("/tmp/a.png")
        );
        assert_eq!(p("https://example.com/a.png"), None);
        assert_eq!(p("file://other-host/tmp/a.png"), None);
        assert_eq!(p("file:///bad%2"), None);
        assert_eq!(p("file:///bad%zz"), None);
    }
}
