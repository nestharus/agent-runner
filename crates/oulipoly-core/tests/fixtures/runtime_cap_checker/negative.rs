fn anonymous(reader: &mut impl std::io::Read) {
    std::thread::sleep(std::time::Duration::new(1 * 1, 0));
    let _ = std::sync::mpsc::sync_channel::<u8>(4);
    let mut bytes = Vec::new();
    let _ = reader.take(12).read_to_end(&mut bytes);
    unsafe { libc::poll(std::ptr::null_mut(), 0, 5000) };
    let _ = Limits {
        queue_capacity: 8,
    };
    let _buffer = [0_u8; 4096];
}

struct Limits {
    queue_capacity: usize,
}
