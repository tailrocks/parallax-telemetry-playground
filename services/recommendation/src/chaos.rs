use crate::config::{MAX_CHAOS_LEAK_KB_PER_REQUEST, MAX_CHAOS_LEAK_KB_TOTAL};
use std::sync::{Mutex, OnceLock};

fn leak_store() -> &'static Mutex<Vec<Vec<u8>>> {
    static STORE: OnceLock<Mutex<Vec<Vec<u8>>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) fn retain_bounded_memory(requested_kb: usize, flagd_enabled: bool) -> usize {
    if requested_kb == 0 {
        return 0;
    }
    let requested_kb = if flagd_enabled {
        requested_kb.max(256)
    } else {
        requested_kb
    }
    .min(MAX_CHAOS_LEAK_KB_PER_REQUEST);

    let mut store = leak_store().lock().expect("chaos leak store lock");
    let held_kb = store
        .iter()
        .map(|buffer| buffer.len() / 1024)
        .sum::<usize>();
    let alloc_kb = requested_kb.min(MAX_CHAOS_LEAK_KB_TOTAL.saturating_sub(held_kb));
    if alloc_kb == 0 {
        return 0;
    }

    let mut bytes = vec![0_u8; alloc_kb * 1024];
    for index in (0..bytes.len()).step_by(4096) {
        bytes[index] = (index % 251) as u8;
    }
    store.push(bytes);
    tracing::warn!(
        requested_kb,
        alloc_kb,
        held_buffers = store.len(),
        flagd_enabled,
        "bounded recommendation chaos memory retained"
    );
    alloc_kb
}
