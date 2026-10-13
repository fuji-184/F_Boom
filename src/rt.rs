//! Share-nothing runtime helper.
//!
//! Model: satu OS thread = satu `current_thread` Tokio runtime.
//! Tidak ada work-stealing antar thread, tidak ada antrean/channels
//! global. Setiap thread memiliki client, koneksi, dan buffer statistik
//! miliknya sendiri. Hasil digabung di main thread setelah join.

/// Jumlah OS thread untuk benchmark share-nothing.
///
/// `total_conc` adalah total konkurensi yang diminta. Kita tidak pernah
/// membuat thread lebih banyak dari konkurensi (agar tidak ada thread
/// menganggur), dan minimal 1.
pub fn num_threads(total_conc: usize) -> usize {
    if total_conc == 0 {
        return 1;
    }
    let cpus = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    total_conc.min(cpus).max(1)
}

/// Bagi `total` konkurensi ke `threads` shard sep merata mungkin.
///
/// Contoh: total=100, threads=8 -> [13,13,13,13,12,12,12,12].
pub fn shard(total: usize, threads: usize) -> Vec<usize> {
    let threads = threads.max(1);
    let base = total / threads;
    let rem = total % threads;
    (0..threads)
        .map(|i| base + usize::from(i < rem))
        .collect()
}

/// Bangun single-threaded Tokio runtime untuk thread saat ini.
///
/// Harus dipanggil dari dalam OS thread yang menjadi pemilik shard.
/// Semua task yang di-spawn di runtime ini TIDAK PERNAH migrasi ke
/// thread lain (tidak ada work-stealing).
pub fn build_current_thread_rt() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build current_thread tokio runtime")
}
