//! Share-nothing runtime helper.
//!
//! Model: satu OS thread = satu `current_thread` Tokio runtime.
//! Tidak ada work-stealing antar thread, tidak ada antrean/channels
//! global. Setiap thread memiliki client, koneksi, dan buffer statistik
//! miliknya sendiri. Hasil digabung di main thread setelah join.

/// Jumlah OS thread untuk benchmark share-nothing.
///
/// `num_thread` berasal dari config (opsional per target). Jika tidak
/// diisi (atau 0), default = jumlah core. Sengaja TIDAK diikat ke
/// max_concurrent: thread boleh lebih sedikit atau lebih banyak dari
/// koneksi. Shard yang kebagian 0 koneksi di-skip oleh pemanggil.
pub fn resolve(num_thread: Option<usize>) -> usize {
    match num_thread {
        Some(n) if n >= 1 => n,
        _ => default_threads(),
    }
}

/// Default: setara jumlah core mesin.
pub fn default_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
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
