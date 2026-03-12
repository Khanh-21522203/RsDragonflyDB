use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub fn instant_to_unix_timestamp(instant: Instant) -> u64 {
    let now_instant = Instant::now();
    let now_system = SystemTime::now();

    let duration_since_now = instant.saturating_duration_since(now_instant);
    let system_time = now_system + duration_since_now;

    system_time.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs()
}

pub fn unix_timestamp_to_instant(timestamp: u64) -> Instant {
    let now_instant = Instant::now();
    let now_system = SystemTime::now();
    let now_unix = now_system.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs();

    if timestamp > now_unix {
        let diff = timestamp - now_unix;
        now_instant + Duration::from_secs(diff)
    } else {
        // Already expired
        now_instant
    }
}