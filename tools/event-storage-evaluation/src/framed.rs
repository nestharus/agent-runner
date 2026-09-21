use crate::model::Event;
use crate::{CandidateObservation, EvalResult, QueryResult, timed};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::thread;

const FRAME_MAGIC: &[u8; 4] = b"OEV1";
const FRAME_VERSION: u16 = 1;
const FRAME_HEADER_LEN: usize = 92;
const INDEX_MAGIC: &[u8; 4] = b"OEI1";
const INDEX_VERSION: u16 = 1;
const INDEX_HEADER_LEN: usize = 16;
const INDEX_ENTRY_LEN: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
struct IndexEntry {
    timestamp_micros: u64,
    offset: u64,
    frame_len: u32,
    event_id: [u8; 16],
    trace_id: [u8; 16],
    family: u8,
    producer: u32,
}

pub fn framed_shards(
    root: &Path,
    events: &[Event],
    producers: usize,
    batch_size: usize,
    time_bounds: (u64, u64),
    trace_id: [u8; 16],
) -> EvalResult<CandidateObservation> {
    fs::create_dir_all(root)?;
    let producer_events = split_by_producer(events, producers);
    let (append_ms, ()) = timed(|| {
        let handles: Vec<_> = producer_events
            .into_iter()
            .enumerate()
            .map(|(producer, events)| {
                let (data_path, index_path) = shard_paths(root, producer);
                thread::spawn(move || -> EvalResult<()> {
                    let mut shard = FramedShard::open(data_path, index_path)?;
                    for batch in events.chunks(batch_size) {
                        shard.append(batch)?;
                    }
                    Ok(())
                })
            })
            .collect();
        for handle in handles {
            handle.join().map_err(|_| "framed producer panicked")??;
        }
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
    })?;

    let (reopen_ms, entries) = timed(|| recover_all(root, producers))?;
    let (full_scan_ms, full_scan) = timed(|| query_entries(root, &entries, |_| true))?;
    let (time_query_ms, time_query) = timed(|| {
        query_entries(root, &entries, |entry| {
            entry.timestamp_micros >= time_bounds.0 && entry.timestamp_micros < time_bounds.1
        })
    })?;
    let (trace_query_ms, trace_query) =
        timed(|| query_entries(root, &entries, |entry| entry.trace_id == trace_id))?;

    Ok(CandidateObservation {
        candidate: "framed_shards_side_index".to_string(),
        append_ms,
        reopen_ms,
        full_scan_ms,
        time_query_ms,
        trace_query_ms,
        full_scan,
        time_query,
        trace_query,
        bytes_on_disk: crate::directory_size(root)?,
        notes: vec![
            "one framed data shard and fixed-width side index per producer".to_string(),
            "CRC32C frames; data sync precedes index sync for each batch".to_string(),
            "reopen measurement scans data and reconciles the side index".to_string(),
        ],
    })
}

struct FramedShard {
    data: File,
    index: File,
    offset: u64,
    known_ids: HashSet<[u8; 16]>,
}

impl FramedShard {
    fn open(data_path: PathBuf, index_path: PathBuf) -> EvalResult<Self> {
        let entries = recover_shard(&data_path, &index_path)?;
        let data = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&data_path)?;
        let index = OpenOptions::new()
            .create(true)
            .read(true)
            .append(true)
            .open(&index_path)?;
        let offset = data.metadata()?.len();
        let known_ids = entries.into_iter().map(|entry| entry.event_id).collect();
        Ok(Self {
            data,
            index,
            offset,
            known_ids,
        })
    }

    fn append(&mut self, events: &[Event]) -> EvalResult<()> {
        let mut frames = Vec::new();
        let mut indexes = Vec::new();
        for event in events {
            if !self.known_ids.insert(event.event_id) {
                continue;
            }
            let body = event.encoded()?;
            let frame = encode_frame(event, &body)?;
            let frame_len = u32::try_from(frame.len()).map_err(|_| "frame too large")?;
            indexes.push(IndexEntry {
                timestamp_micros: event.timestamp_micros,
                offset: self.offset,
                frame_len,
                event_id: event.event_id,
                trace_id: event.trace_id,
                family: event.family,
                producer: event.producer,
            });
            self.offset += u64::from(frame_len);
            frames.push(frame);
        }
        for frame in frames {
            self.data.write_all(&frame)?;
        }
        self.data.sync_data()?;
        for entry in indexes {
            self.index.write_all(&encode_index_entry(&entry))?;
        }
        self.index.sync_data()?;
        Ok(())
    }
}

fn recover_all(root: &Path, producers: usize) -> EvalResult<Vec<Vec<IndexEntry>>> {
    (0..producers)
        .map(|producer| {
            let (data_path, index_path) = shard_paths(root, producer);
            recover_shard(&data_path, &index_path)
        })
        .collect()
}

fn recover_shard(data_path: &Path, index_path: &Path) -> EvalResult<Vec<IndexEntry>> {
    if let Some(parent) = data_path.parent() {
        fs::create_dir_all(parent)?;
    }
    if !data_path.exists() {
        File::create(data_path)?.sync_all()?;
    }
    let entries = scan_frames(data_path, true)?;
    let current = read_index(index_path);
    if current.as_ref().ok() != Some(&entries) {
        publish_index(index_path, &entries)?;
    }
    Ok(entries)
}

fn scan_frames(path: &Path, repair_torn_tail: bool) -> EvalResult<Vec<IndexEntry>> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(repair_torn_tail)
        .open(path)?;
    let file_len = file.metadata()?.len();
    let mut entries = Vec::new();
    let mut offset = 0_u64;
    while offset < file_len {
        let remaining = file_len - offset;
        if remaining < FRAME_HEADER_LEN as u64 {
            if repair_torn_tail {
                file.set_len(offset)?;
                file.sync_all()?;
                break;
            }
            return Err(format!("truncated frame header at offset {offset}").into());
        }
        file.seek(SeekFrom::Start(offset))?;
        let mut header = [0_u8; FRAME_HEADER_LEN];
        file.read_exact(&mut header)?;
        let decoded = decode_header(&header)
            .map_err(|error| format!("corrupt frame header at offset {offset}: {error}"))?;
        let body_len = decoded.body_len as u64;
        if remaining < FRAME_HEADER_LEN as u64 + body_len {
            if repair_torn_tail {
                file.set_len(offset)?;
                file.sync_all()?;
                break;
            }
            return Err(format!("truncated frame body at offset {offset}").into());
        }
        let mut body = vec![0_u8; decoded.body_len as usize];
        file.read_exact(&mut body)?;
        if crc32c(&body) != decoded.body_crc {
            return Err(format!("corrupt frame body at offset {offset}").into());
        }
        let frame_len = FRAME_HEADER_LEN as u64 + body_len;
        entries.push(IndexEntry {
            timestamp_micros: decoded.timestamp_micros,
            offset,
            frame_len: u32::try_from(frame_len).map_err(|_| "frame too large")?,
            event_id: decoded.event_id,
            trace_id: decoded.trace_id,
            family: decoded.family,
            producer: decoded.producer,
        });
        offset += frame_len;
    }
    Ok(entries)
}

struct DecodedHeader {
    body_len: u32,
    body_crc: u32,
    event_id: [u8; 16],
    trace_id: [u8; 16],
    timestamp_micros: u64,
    producer: u32,
    family: u8,
}

fn encode_frame(event: &Event, body: &[u8]) -> EvalResult<Vec<u8>> {
    let body_len = u32::try_from(body.len()).map_err(|_| "body too large")?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + body.len());
    frame.extend_from_slice(FRAME_MAGIC);
    frame.extend_from_slice(&FRAME_VERSION.to_le_bytes());
    frame.extend_from_slice(&(FRAME_HEADER_LEN as u16).to_le_bytes());
    frame.extend_from_slice(&body_len.to_le_bytes());
    frame.extend_from_slice(&crc32c(body).to_le_bytes());
    frame.extend_from_slice(&event.event_id);
    frame.extend_from_slice(&event.trace_id);
    frame.extend_from_slice(&event.span_id);
    frame.extend_from_slice(&event.parent_span_id);
    frame.extend_from_slice(&event.timestamp_micros.to_le_bytes());
    frame.extend_from_slice(&event.process_root_id);
    frame.extend_from_slice(&event.producer.to_le_bytes());
    frame.push(event.family);
    frame.push(0);
    frame.extend_from_slice(&0_u16.to_le_bytes());
    debug_assert_eq!(frame.len(), FRAME_HEADER_LEN - 4);
    frame.extend_from_slice(&crc32c(&frame).to_le_bytes());
    frame.extend_from_slice(body);
    Ok(frame)
}

fn decode_header(header: &[u8; FRAME_HEADER_LEN]) -> Result<DecodedHeader, &'static str> {
    if &header[..4] != FRAME_MAGIC {
        return Err("bad magic");
    }
    if read_u16(header, 4) != FRAME_VERSION {
        return Err("unsupported version");
    }
    if read_u16(header, 6) as usize != FRAME_HEADER_LEN {
        return Err("bad header length");
    }
    let expected_crc = read_u32(header, FRAME_HEADER_LEN - 4);
    if crc32c(&header[..FRAME_HEADER_LEN - 4]) != expected_crc {
        return Err("header checksum mismatch");
    }
    Ok(DecodedHeader {
        body_len: read_u32(header, 8),
        body_crc: read_u32(header, 12),
        event_id: array_at(header, 16),
        trace_id: array_at(header, 32),
        timestamp_micros: read_u64(header, 64),
        producer: read_u32(header, 80),
        family: header[84],
    })
}

fn publish_index(path: &Path, entries: &[IndexEntry]) -> EvalResult<()> {
    let temporary = path.with_extension("idx.tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(&index_header())?;
    for entry in entries {
        file.write_all(&encode_index_entry(entry))?;
    }
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    sync_parent(path)?;
    Ok(())
}

fn index_header() -> [u8; INDEX_HEADER_LEN] {
    let mut header = [0_u8; INDEX_HEADER_LEN];
    header[..4].copy_from_slice(INDEX_MAGIC);
    header[4..6].copy_from_slice(&INDEX_VERSION.to_le_bytes());
    header[6..8].copy_from_slice(&(INDEX_ENTRY_LEN as u16).to_le_bytes());
    let checksum = crc32c(&header[..12]);
    header[12..].copy_from_slice(&checksum.to_le_bytes());
    header
}

fn read_index(path: &Path) -> EvalResult<Vec<IndexEntry>> {
    let mut file = File::open(path)?;
    let mut header = [0_u8; INDEX_HEADER_LEN];
    file.read_exact(&mut header)?;
    if header != index_header() {
        return Err("invalid index header".into());
    }
    let mut entries = Vec::new();
    loop {
        let mut encoded = [0_u8; INDEX_ENTRY_LEN];
        match file.read_exact(&mut encoded) {
            Ok(()) => entries.push(decode_index_entry(&encoded)?),
            Err(error) if error.kind() == ErrorKind::UnexpectedEof => {
                if file.stream_position()? == file.metadata()?.len() {
                    break;
                }
                return Err("truncated index entry".into());
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(entries)
}

fn encode_index_entry(entry: &IndexEntry) -> [u8; INDEX_ENTRY_LEN] {
    let mut encoded = [0_u8; INDEX_ENTRY_LEN];
    encoded[..8].copy_from_slice(&entry.timestamp_micros.to_le_bytes());
    encoded[8..16].copy_from_slice(&entry.offset.to_le_bytes());
    encoded[16..20].copy_from_slice(&entry.frame_len.to_le_bytes());
    encoded[20..36].copy_from_slice(&entry.event_id);
    encoded[36..52].copy_from_slice(&entry.trace_id);
    encoded[52] = entry.family;
    encoded[53..57].copy_from_slice(&entry.producer.to_le_bytes());
    let checksum = crc32c(&encoded[..INDEX_ENTRY_LEN - 4]);
    encoded[INDEX_ENTRY_LEN - 4..].copy_from_slice(&checksum.to_le_bytes());
    encoded
}

fn decode_index_entry(encoded: &[u8; INDEX_ENTRY_LEN]) -> EvalResult<IndexEntry> {
    let expected = read_u32(encoded, INDEX_ENTRY_LEN - 4);
    if crc32c(&encoded[..INDEX_ENTRY_LEN - 4]) != expected {
        return Err("index checksum mismatch".into());
    }
    Ok(IndexEntry {
        timestamp_micros: read_u64(encoded, 0),
        offset: read_u64(encoded, 8),
        frame_len: read_u32(encoded, 16),
        event_id: array_at(encoded, 20),
        trace_id: array_at(encoded, 36),
        family: encoded[52],
        producer: read_u32(encoded, 53),
    })
}

fn query_entries(
    root: &Path,
    entries: &[Vec<IndexEntry>],
    predicate: impl Fn(&IndexEntry) -> bool,
) -> EvalResult<QueryResult> {
    let mut result = QueryResult::default();
    for (producer, shard_entries) in entries.iter().enumerate() {
        let (data_path, _) = shard_paths(root, producer);
        let mut data = File::open(data_path)?;
        for entry in shard_entries.iter().filter(|entry| predicate(entry)) {
            let body = read_body(&mut data, entry)?;
            result.observe(&body);
        }
    }
    Ok(result)
}

fn read_body(data: &mut File, entry: &IndexEntry) -> EvalResult<Vec<u8>> {
    data.seek(SeekFrom::Start(entry.offset))?;
    let mut header = [0_u8; FRAME_HEADER_LEN];
    data.read_exact(&mut header)?;
    let decoded =
        decode_header(&header).map_err(|error| format!("invalid indexed frame: {error}"))?;
    if decoded.event_id != entry.event_id
        || decoded.trace_id != entry.trace_id
        || decoded.timestamp_micros != entry.timestamp_micros
    {
        return Err("index/data identity mismatch".into());
    }
    let mut body = vec![0_u8; decoded.body_len as usize];
    data.read_exact(&mut body)?;
    if crc32c(&body) != decoded.body_crc {
        return Err("indexed body checksum mismatch".into());
    }
    Ok(body)
}

fn shard_paths(root: &Path, producer: usize) -> (PathBuf, PathBuf) {
    (
        root.join(format!("events-{producer:04}.oev")),
        root.join(format!("events-{producer:04}.idx")),
    )
}

fn split_by_producer(events: &[Event], producers: usize) -> Vec<Vec<Event>> {
    let mut split = vec![Vec::new(); producers];
    for event in events {
        split[event.producer as usize].push(event.clone());
    }
    split
}

fn sync_parent(path: &Path) -> EvalResult<()> {
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn crc32c(bytes: &[u8]) -> u32 {
    let mut crc = !0_u32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0x82f6_3b78 & (0_u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().expect("fixed offset"))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("fixed offset"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("fixed offset"))
}

fn array_at<const N: usize>(bytes: &[u8], offset: usize) -> [u8; N] {
    bytes[offset..offset + N].try_into().expect("fixed offset")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn torn_tail_is_removed_without_losing_acknowledged_frames() {
        let directory = tempfile::tempdir().unwrap();
        let (data_path, index_path) = shard_paths(directory.path(), 0);
        let events = crate::model::fixture_events(2, 1, 32);
        let mut shard = FramedShard::open(data_path.clone(), index_path.clone()).unwrap();
        shard.append(&events).unwrap();
        drop(shard);
        let acknowledged_len = fs::metadata(&data_path).unwrap().len();
        let mut file = OpenOptions::new().append(true).open(&data_path).unwrap();
        file.write_all(&encode_frame(&events[0], &events[0].encoded().unwrap()).unwrap()[..17])
            .unwrap();
        file.sync_all().unwrap();
        drop(file);

        let entries = recover_shard(&data_path, &index_path).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(fs::metadata(data_path).unwrap().len(), acknowledged_len);
    }

    #[test]
    fn missing_and_partial_indexes_rebuild_from_durable_data() {
        let directory = tempfile::tempdir().unwrap();
        let (data_path, index_path) = shard_paths(directory.path(), 0);
        let events = crate::model::fixture_events(3, 1, 32);
        let mut shard = FramedShard::open(data_path.clone(), index_path.clone()).unwrap();
        shard.append(&events).unwrap();
        drop(shard);

        fs::remove_file(&index_path).unwrap();
        assert_eq!(recover_shard(&data_path, &index_path).unwrap().len(), 3);
        let index_len = fs::metadata(&index_path).unwrap().len();
        OpenOptions::new()
            .write(true)
            .open(&index_path)
            .unwrap()
            .set_len(index_len - 7)
            .unwrap();
        assert_eq!(recover_shard(&data_path, &index_path).unwrap().len(), 3);
        assert_eq!(read_index(&index_path).unwrap().len(), 3);
    }

    #[test]
    fn duplicate_retry_is_physically_idempotent_within_the_routed_shard() {
        let directory = tempfile::tempdir().unwrap();
        let (data_path, index_path) = shard_paths(directory.path(), 0);
        let events = crate::model::fixture_events(1, 1, 32);
        let mut shard = FramedShard::open(data_path.clone(), index_path.clone()).unwrap();
        shard.append(&events).unwrap();
        let first_len = fs::metadata(&data_path).unwrap().len();
        shard.append(&events).unwrap();
        drop(shard);
        assert_eq!(fs::metadata(&data_path).unwrap().len(), first_len);
        assert_eq!(recover_shard(&data_path, &index_path).unwrap().len(), 1);
    }

    #[test]
    fn corrupt_shard_is_detected_independently() {
        let directory = tempfile::tempdir().unwrap();
        let events = crate::model::fixture_events(2, 2, 32);
        for producer in 0..2 {
            let (data_path, index_path) = shard_paths(directory.path(), producer);
            let mut shard = FramedShard::open(data_path, index_path).unwrap();
            shard
                .append(
                    &events
                        .iter()
                        .filter(|event| event.producer as usize == producer)
                        .cloned()
                        .collect::<Vec<_>>(),
                )
                .unwrap();
        }
        let (corrupt_path, corrupt_index) = shard_paths(directory.path(), 1);
        let mut bytes = fs::read(&corrupt_path).unwrap();
        *bytes.last_mut().unwrap() ^= 0xff;
        fs::write(&corrupt_path, bytes).unwrap();
        assert!(recover_shard(&corrupt_path, &corrupt_index).is_err());
        let (healthy_path, healthy_index) = shard_paths(directory.path(), 0);
        assert_eq!(
            recover_shard(&healthy_path, &healthy_index).unwrap().len(),
            1
        );
    }
}
