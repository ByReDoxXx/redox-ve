//! Bellek balonu ve dinamik bellek hedefi.

/// Balonla inilebilecek en küçük bellek (MB).
pub const MIN_BALLOON_MB: u64 = 256;
/// Dinamik bellekte konuğun kullandığının üstüne bırakılan pay (yüzde) ve bu payın alt sınırı (MB).
pub const DYNAMIC_BUFFER_PCT: u64 = 20;
pub const DYNAMIC_BUFFER_MIN_MB: u64 = 256;
/// Hedefler bu adıma yuvarlanır; daha küçük değişiklikler uygulanmaz.
pub const DYNAMIC_STEP_MB: u64 = 64;

/// Dinamik bellek için yeni balon hedefi (MB); değişiklik gerekmiyorsa `None`.
///
/// Hedef, konuğun kullandığı bellek + paydır; `min_mb` ile `total_mb` arasında tutulur. Büyütme hemen yapılır
/// (konuk bellek darlığı çekmesin), küçültme ise yalnızca fark belirginse: hedef sürekli oynamasın.
/// `current_mb` kayıtlı hedeftir (0: belleğin tamamı).
pub fn dynamic_memory_target(total_mb: u64, min_mb: u64, current_mb: u64, stats: crate::driver::BalloonStats) -> Option<u64> {
    if min_mb == 0 || min_mb > total_mb || stats.total_mb == 0 {
        return None;
    }
    let current = if current_mb == 0 { total_mb } else { current_mb.min(total_mb) };
    let used = stats.total_mb.saturating_sub(stats.available_mb);
    let wanted = used + (used * DYNAMIC_BUFFER_PCT / 100).max(DYNAMIC_BUFFER_MIN_MB);
    let wanted = wanted.div_ceil(DYNAMIC_STEP_MB) * DYNAMIC_STEP_MB;
    let target = wanted.clamp(min_mb, total_mb);
    if target > current {
        return Some(target);
    }
    let slack = (total_mb / 20).max(2 * DYNAMIC_STEP_MB);
    (current - target >= slack).then_some(target)
}

/// Çalışan konuğa bellek bu adımın katlarıyla eklenir (MB): konuk işletim sistemleri sonradan takılan belleği
/// bu büyüklükteki bloklar halinde kullanır, artan kısım boşa gider.
pub const MEM_STEP_MB: u64 = 128;
/// Bir açılışta çalışırken kaç kez bellek eklenebilir (konuğun boş bellek yuvası sayısı).
pub const MEM_SLOTS: usize = 8;
