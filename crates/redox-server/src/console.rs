//! JavaScript'siz konsolun saf parçaları: metni QEMU tuş kodlarına çevirme ve
//! QEMU'nun PPM ekran dökümünü tarayıcının gösterebileceği PNG'ye dönüştürme.

/// Tek seferde gönderilebilecek en fazla tuş vuruşu.
pub const MAX_CHORDS: usize = 512;

fn chord(keys: &[&str]) -> Vec<String> {
    keys.iter().map(|k| k.to_string()).collect()
}

/// ABD klavye düzenine göre bir karakterin tuş kodu ve Shift gerekip gerekmediği.
fn key_for(c: char) -> Option<(String, bool)> {
    let plain = |k: &str| Some((k.to_string(), false));
    let shifted = |k: &str| Some((k.to_string(), true));
    match c {
        'a'..='z' | '0'..='9' => plain(&c.to_string()),
        'A'..='Z' => shifted(&c.to_ascii_lowercase().to_string()),
        ' ' => plain("spc"),
        '\n' => plain("ret"),
        '\t' => plain("tab"),
        '-' => plain("minus"),
        '=' => plain("equal"),
        '[' => plain("bracket_left"),
        ']' => plain("bracket_right"),
        ';' => plain("semicolon"),
        '\'' => plain("apostrophe"),
        '`' => plain("grave_accent"),
        '\\' => plain("backslash"),
        ',' => plain("comma"),
        '.' => plain("dot"),
        '/' => plain("slash"),
        '!' => shifted("1"),
        '@' => shifted("2"),
        '#' => shifted("3"),
        '$' => shifted("4"),
        '%' => shifted("5"),
        '^' => shifted("6"),
        '&' => shifted("7"),
        '*' => shifted("8"),
        '(' => shifted("9"),
        ')' => shifted("0"),
        '_' => shifted("minus"),
        '+' => shifted("equal"),
        '{' => shifted("bracket_left"),
        '}' => shifted("bracket_right"),
        ':' => shifted("semicolon"),
        '"' => shifted("apostrophe"),
        '~' => shifted("grave_accent"),
        '|' => shifted("backslash"),
        '<' => shifted("comma"),
        '>' => shifted("dot"),
        '?' => shifted("slash"),
        _ => None,
    }
}

/// Metni tuş vuruşlarına çevirir. Çevrilemeyen ilk karakteri hata olarak döndürür
/// (ör. Türkçe harfler: konuğun klavye düzeni bilinmediği için gönderilemez).
pub fn text_to_chords(text: &str) -> Result<Vec<Vec<String>>, char> {
    text.chars()
        .filter(|c| *c != '\r')
        .map(|c| {
            let (key, shift) = key_for(c).ok_or(c)?;
            Ok(if shift { vec!["shift".to_string(), key] } else { vec![key] })
        })
        .collect()
}

/// Konsoldaki özel tuş düğmeleri: (form değeri, etiket).
pub const SPECIAL_KEYS: [(&str, &str); 14] = [
    ("enter", "Enter"),
    ("esc", "Esc"),
    ("tab", "Tab"),
    ("backspace", "Backspace"),
    ("space", "Boşluk"),
    ("up", "↑"),
    ("down", "↓"),
    ("left", "←"),
    ("right", "→"),
    ("delete", "Delete"),
    ("f2", "F2"),
    ("f10", "F10"),
    ("f12", "F12"),
    ("ctrl-alt-del", "Ctrl+Alt+Del"),
];

pub fn special_key(name: &str) -> Option<Vec<String>> {
    Some(match name {
        "enter" => chord(&["ret"]),
        "space" => chord(&["spc"]),
        "ctrl-alt-del" => chord(&["ctrl", "alt", "delete"]),
        "esc" | "tab" | "backspace" | "delete" | "up" | "down" | "left" | "right" | "home" | "end" | "pgup" | "pgdn" => {
            chord(&[name])
        }
        f if f.len() <= 3 && f.starts_with('f') && f[1..].parse::<u8>().is_ok_and(|n| (1..=12).contains(&n)) => chord(&[f]),
        _ => return None,
    })
}

fn crc32(chunks: &[&[u8]]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for bytes in chunks {
        for &b in *bytes {
            crc ^= b as u32;
            for _ in 0..8 {
                crc = if crc & 1 == 1 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    // Taşmadan önce mod almak için 5552 baytlık dilimler (zlib'in kullandığı sınır).
    for chunk in bytes.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65_521;
        b %= 65_521;
    }
    (b << 16) | a
}

fn png_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    out.extend_from_slice(&crc32(&[kind, data]).to_be_bytes());
}

/// İkili PPM (P6, 8 bit) görüntüyü sıkıştırmasız PNG'ye çevirir.
pub fn ppm_to_png(ppm: &[u8]) -> Result<Vec<u8>, String> {
    // Başlık: "P6" genişlik yükseklik azami-değer, boşlukla ayrılmış; ardından tek boşluk ve pikseller.
    let mut pos = 0;
    let mut token = || -> Option<&[u8]> {
        while pos < ppm.len() && ppm[pos].is_ascii_whitespace() {
            pos += 1;
        }
        let start = pos;
        while pos < ppm.len() && !ppm[pos].is_ascii_whitespace() {
            pos += 1;
        }
        (pos > start).then(|| &ppm[start..pos])
    };
    let number = |t: Option<&[u8]>| t.and_then(|t| std::str::from_utf8(t).ok()).and_then(|t| t.parse::<usize>().ok());
    if token() != Some(b"P6") {
        return Err("ekran dökümü PPM (P6) biçiminde değil".to_string());
    }
    let (width, height, max) = (number(token()), number(token()), number(token()));
    let (Some(width), Some(height), Some(255)) = (width, height, max) else {
        return Err("PPM başlığı okunamadı".to_string());
    };
    if width == 0 || height == 0 || width > 8192 || height > 8192 {
        return Err(format!("beklenmeyen ekran boyutu: {width}x{height}"));
    }
    let pixels = ppm.get(pos + 1..pos + 1 + width * height * 3).ok_or("PPM verisi eksik")?;

    // Her satırın başına süzgeç baytı (0 = yok).
    let mut raw = Vec::with_capacity((width * 3 + 1) * height);
    for row in pixels.chunks(width * 3) {
        raw.push(0);
        raw.extend_from_slice(row);
    }
    // zlib sarmalayıcı + "stored" (sıkıştırmasız) deflate blokları.
    let mut z = vec![0x78, 0x01];
    let mut blocks = raw.chunks(65_535).peekable();
    while let Some(block) = blocks.next() {
        z.push(u8::from(blocks.peek().is_none()));
        z.extend_from_slice(&(block.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        z.extend_from_slice(block);
    }
    z.extend_from_slice(&adler32(&raw).to_be_bytes());

    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(width as u32).to_be_bytes());
    ihdr.extend_from_slice(&(height as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8 bit, RGB, sıkıştırma/süzgeç/geçmeli yok
    png_chunk(&mut out, b"IHDR", &ihdr);
    png_chunk(&mut out, b"IDAT", &z);
    png_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_maps_to_us_layout_keys() {
        assert_eq!(text_to_chords("ab"), Ok(vec![chord(&["a"]), chord(&["b"])]));
        assert_eq!(text_to_chords("A1!"), Ok(vec![chord(&["shift", "a"]), chord(&["1"]), chord(&["shift", "1"])]));
        assert_eq!(
            text_to_chords("ls -la /etc\r\n"),
            Ok(["l", "s", "spc", "minus", "l", "a", "spc", "slash", "e", "t", "c", "ret"].map(|k| chord(&[k])).to_vec())
        );
        assert_eq!(text_to_chords("a|b"), Ok(vec![chord(&["a"]), chord(&["shift", "backslash"]), chord(&["b"])]));
        assert_eq!(text_to_chords(""), Ok(vec![]));
        assert_eq!(text_to_chords("şifre"), Err('ş'), "çevrilemeyen karakter bildirilir");
        assert_eq!(text_to_chords("a\u{1b}"), Err('\u{1b}'), "denetim karakterleri kabul edilmez");
        // Yazdırılabilir ASCII'nin tamamı karşılanır.
        let all: String = (32u8..127).map(char::from).collect();
        assert_eq!(text_to_chords(&all).unwrap().len(), 95);
    }

    #[test]
    fn special_keys_are_an_allow_list() {
        assert_eq!(special_key("enter"), Some(chord(&["ret"])));
        assert_eq!(special_key("ctrl-alt-del"), Some(chord(&["ctrl", "alt", "delete"])));
        assert_eq!(special_key("f12"), Some(chord(&["f12"])));
        for (name, _) in SPECIAL_KEYS {
            assert!(special_key(name).is_some(), "{name}");
        }
        for bad in ["", "f0", "f13", "f1x", "power", "sysrq", "ret\"}", "ffff"] {
            assert_eq!(special_key(bad), None, "{bad}");
        }
    }

    #[test]
    fn checksums_match_reference_values() {
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
        assert_eq!(crc32(&[b"1234", b"56789"]), 0xCBF4_3926, "parçalı girdi aynı sonucu verir");
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(&[]), 1);
        assert_eq!(adler32(&vec![0xFF; 100_000]), {
            let (mut a, mut b) = (1u64, 0u64);
            for _ in 0..100_000 {
                a = (a + 255) % 65_521;
                b = (b + a) % 65_521;
            }
            ((b << 16) | a) as u32
        });
    }

    /// PNG'yi çözer: (genişlik, yükseklik, süzgeç baytları atılmış RGB).
    fn decode(png: &[u8]) -> (u32, u32, Vec<u8>) {
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let be = |b: &[u8]| u32::from_be_bytes(b.try_into().unwrap());
        let (mut pos, mut size, mut idat, mut kinds) = (8, (0, 0), Vec::new(), Vec::new());
        while pos < png.len() {
            let len = be(&png[pos..pos + 4]) as usize;
            let kind = &png[pos + 4..pos + 8];
            let data = &png[pos + 8..pos + 8 + len];
            assert_eq!(be(&png[pos + 8 + len..pos + 12 + len]), crc32(&[kind, data]), "CRC");
            kinds.push(String::from_utf8_lossy(kind).to_string());
            match kind {
                b"IHDR" => {
                    size = (be(&data[..4]), be(&data[4..8]));
                    assert_eq!(&data[8..], &[8, 2, 0, 0, 0]);
                }
                b"IDAT" => idat.extend_from_slice(data),
                _ => {}
            }
            pos += 12 + len;
        }
        assert_eq!(kinds, ["IHDR", "IDAT", "IEND"]);
        assert_eq!(&png[png.len() - 4..], &[0xAE, 0x42, 0x60, 0x82], "IEND CRC'si sabittir");

        assert_eq!(&idat[..2], &[0x78, 0x01]);
        let (mut p, mut raw) = (2, Vec::new());
        loop {
            let last = idat[p];
            let len = u16::from_le_bytes([idat[p + 1], idat[p + 2]]);
            assert_eq!(!len, u16::from_le_bytes([idat[p + 3], idat[p + 4]]));
            raw.extend_from_slice(&idat[p + 5..p + 5 + len as usize]);
            p += 5 + len as usize;
            if last == 1 {
                break;
            }
            assert_eq!(last, 0);
        }
        assert_eq!(be(&idat[p..p + 4]), adler32(&raw));
        assert_eq!(p + 4, idat.len());
        let row = size.0 as usize * 3 + 1;
        let rgb = raw.chunks(row).flat_map(|r| {
            assert_eq!(r[0], 0, "süzgeç baytı");
            r[1..].to_vec()
        });
        (size.0, size.1, rgb.collect())
    }

    #[test]
    fn ppm_round_trips_through_png() {
        let pixels: Vec<u8> = (0..2 * 3 * 3).map(|i| (i * 13) as u8).collect();
        let mut ppm = b"P6\n2 3\n255\n".to_vec();
        ppm.extend_from_slice(&pixels);
        assert_eq!(decode(&ppm_to_png(&ppm).unwrap()), (2, 3, pixels));
    }

    #[test]
    fn large_screens_span_multiple_deflate_blocks() {
        // 800x600 RGB = 1,44 MB > 65 535: birden çok blok gerekir.
        let pixels: Vec<u8> = (0..800 * 600 * 3).map(|i| (i % 251) as u8).collect();
        let mut ppm = b"P6 800 600 255\n".to_vec();
        ppm.extend_from_slice(&pixels);
        let (w, h, rgb) = decode(&ppm_to_png(&ppm).unwrap());
        assert_eq!((w, h), (800, 600));
        assert!(rgb == pixels);
    }

    #[test]
    fn malformed_ppm_is_rejected() {
        for bad in [&b""[..], b"P5 1 1 255\n\0", b"P6 1 1 65535\n\0\0\0", b"P6 0 1 255\n", b"P6 2 2 255\n\0\0\0", b"P6 x y 255\n", b"P6 99999 1 255\n"] {
            assert!(ppm_to_png(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
    }
}
