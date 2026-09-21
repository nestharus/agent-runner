const POLL_TICKS: u64 = 25;
const ODDLY_NAMED: usize = 7;

fn checked(reader: &mut impl std::io::Read) {
    let _cadence = std::time::Duration::from_millis(POLL_TICKS);
    let mut values = vec![1, 2, 3];
    values.truncate(0);
    let _ordinary_iterator = values.iter().take(1).count();
    let _ = ODDLY_NAMED;
    let _ = reader;
}
