//! Gömülü yazı tipleri (assets/fonts). Bu dosya indirme betiğiyle üretildi.

pub const FONTS_CSS: &str = include_str!("../assets/fonts/fonts.css");

pub const FONTS: &[(&str, &[u8])] = &[
    ("chakra-petch-400-latin-ext.woff2", include_bytes!("../assets/fonts/chakra-petch-400-latin-ext.woff2")),
    ("chakra-petch-400-latin.woff2", include_bytes!("../assets/fonts/chakra-petch-400-latin.woff2")),
    ("chakra-petch-500-latin-ext.woff2", include_bytes!("../assets/fonts/chakra-petch-500-latin-ext.woff2")),
    ("chakra-petch-500-latin.woff2", include_bytes!("../assets/fonts/chakra-petch-500-latin.woff2")),
    ("chakra-petch-600-latin-ext.woff2", include_bytes!("../assets/fonts/chakra-petch-600-latin-ext.woff2")),
    ("chakra-petch-600-latin.woff2", include_bytes!("../assets/fonts/chakra-petch-600-latin.woff2")),
    ("chakra-petch-700-latin-ext.woff2", include_bytes!("../assets/fonts/chakra-petch-700-latin-ext.woff2")),
    ("chakra-petch-700-latin.woff2", include_bytes!("../assets/fonts/chakra-petch-700-latin.woff2")),
    ("jetbrains-mono-latin-ext.woff2", include_bytes!("../assets/fonts/jetbrains-mono-latin-ext.woff2")),
    ("jetbrains-mono-latin.woff2", include_bytes!("../assets/fonts/jetbrains-mono-latin.woff2")),
];
